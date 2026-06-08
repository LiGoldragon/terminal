use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;

use signal_frame::{
    ExchangeIdentifier, ExchangeLane, LaneSequence, NonEmpty, Reply, Request, SessionEpoch,
    SubReply,
};
use signal_terminal::{
    AcquireInputGate, Frame, FrameBody, GateAcquired, GateBusy, GateReleased, InjectionAck,
    InjectionRejected, Input, InputGateLease, InputGateLeaseIdentifier, InputGateReason,
    ListPromptPatterns, Output, PromptPattern, PromptPatternBytes, PromptPatternList,
    PromptPatternRegistered, PromptPatternUnregistered, PromptState, RegisterPromptPattern,
    ReleaseInputGate, SessionList, SessionResolved, SubscribeTerminalWorkerLifecycle,
    SubscriptionRetracted, TerminalCapture, TerminalCaptured, TerminalColumns, TerminalConnection,
    TerminalDetached, TerminalInput, TerminalInputAccepted, TerminalInputBytes, TerminalName,
    TerminalReady, TerminalRejected, TerminalResize, TerminalResized, TerminalRows,
    TerminalWorkerLifecycleSnapshot, TranscriptDelta, UnregisterPromptPattern, WriteInjection,
};

use crate::pty::TerminalSocket;
use crate::{Error, Result};

const DEFAULT_CONTROL_SOCKET: &str = "/tmp/terminal.control.sock";
const DEFAULT_TERMINAL: &str = "operator";

fn synthetic_exchange() -> ExchangeIdentifier {
    ExchangeIdentifier::new(
        SessionEpoch::new(0),
        ExchangeLane::Connector,
        LaneSequence::first(),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSignalRequest {
    control_socket: PathBuf,
    terminal: TerminalName,
    operation: TerminalSignalOperation,
}

impl TerminalSignalRequest {
    pub fn from_environment() -> Result<Self> {
        Ok(TerminalSignalArguments::from_environment()?.into_request())
    }

    pub fn new(
        control_socket: impl Into<PathBuf>,
        terminal: TerminalName,
        operation: TerminalSignalOperation,
    ) -> Self {
        Self {
            control_socket: control_socket.into(),
            terminal,
            operation,
        }
    }

    pub fn run(self, mut output: impl Write) -> Result<()> {
        let request = self.operation.into_request(self.terminal);
        let framed_request = TerminalSignalRequestFrame::new(request).into_request()?;
        let event =
            TerminalSocket::from_control_socket(self.control_socket).signal(framed_request)?;
        let framed_event = TerminalSignalEventFrame::new(event).into_event()?;
        TerminalEventLine::new(framed_event).write_to(&mut output)?;
        output.flush()?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalSignalOperation {
    Connect,
    Input { bytes: Vec<u8> },
    Prompt { text: String },
    Capture,
    Resize { rows: u16, columns: u16 },
    RegisterLiteralPrompt { suffix: Vec<u8> },
    RegisterRegexPrompt { pattern: Vec<u8> },
    UnregisterPrompt { pattern_id: String },
    ListPrompts,
    AcquireGate { pattern_id: Option<String> },
    ReleaseGate { lease_id: u64 },
    Inject { lease_id: u64, bytes: Vec<u8> },
    InjectPrompt { lease_id: u64, text: String },
    WorkerLifecycleSnapshot,
}

impl TerminalSignalOperation {
    fn into_request(self, terminal: TerminalName) -> Input {
        match self {
            Self::Connect => TerminalConnection(terminal).into(),
            Self::Input { bytes } => TerminalInput {
                terminal,
                bytes: TerminalInputBytes::new(Self::signal_bytes(&bytes)),
            }
            .into(),
            Self::Prompt { text } => {
                let mut bytes = text.into_bytes();
                bytes.push(b'\r');
                TerminalInput {
                    terminal,
                    bytes: TerminalInputBytes::new(Self::signal_bytes(&bytes)),
                }
                .into()
            }
            Self::Capture => TerminalCapture(terminal).into(),
            Self::Resize { rows, columns } => TerminalResize {
                terminal,
                rows: TerminalRows::new(u64::from(rows)),
                columns: TerminalColumns::new(u64::from(columns)),
            }
            .into(),
            Self::RegisterLiteralPrompt { suffix } => RegisterPromptPattern {
                terminal,
                pattern: PromptPattern::LiteralSuffix(PromptPatternBytes::new(Self::signal_bytes(
                    &suffix,
                ))),
            }
            .into(),
            Self::RegisterRegexPrompt { pattern } => RegisterPromptPattern {
                terminal,
                pattern: PromptPattern::RegexSuffix(PromptPatternBytes::new(Self::signal_bytes(
                    &pattern,
                ))),
            }
            .into(),
            Self::UnregisterPrompt { pattern_id } => UnregisterPromptPattern {
                terminal,
                pattern_id: signal_terminal::PromptPatternIdentifier::new(pattern_id),
            }
            .into(),
            Self::ListPrompts => ListPromptPatterns(terminal).into(),
            Self::AcquireGate { pattern_id } => AcquireInputGate {
                terminal,
                reason: InputGateReason::new("terminal signal cli".to_string()),
                prompt_pattern_identifier: pattern_id
                    .map(signal_terminal::PromptPatternIdentifier::new),
            }
            .into(),
            Self::ReleaseGate { lease_id } => ReleaseInputGate {
                terminal,
                lease: InputGateLease(InputGateLeaseIdentifier::new(lease_id)),
            }
            .into(),
            Self::Inject { lease_id, bytes } => WriteInjection {
                terminal,
                lease: InputGateLease(InputGateLeaseIdentifier::new(lease_id)),
                bytes: TerminalInputBytes::new(Self::signal_bytes(&bytes)),
            }
            .into(),
            Self::InjectPrompt { lease_id, text } => {
                let mut bytes = text.into_bytes();
                bytes.push(b'\r');
                WriteInjection {
                    terminal,
                    lease: InputGateLease(InputGateLeaseIdentifier::new(lease_id)),
                    bytes: TerminalInputBytes::new(Self::signal_bytes(&bytes)),
                }
                .into()
            }
            Self::WorkerLifecycleSnapshot => SubscribeTerminalWorkerLifecycle(terminal).into(),
        }
    }

    /// Widen a `u8` argument byte buffer into the schema-emitted `Integer`
    /// (`u64`) byte vector the signal-terminal contract carries.
    fn signal_bytes(bytes: &[u8]) -> Vec<u64> {
        bytes.iter().map(|byte| u64::from(*byte)).collect()
    }
}

struct TerminalSignalArguments {
    control_socket: PathBuf,
    terminal: TerminalName,
    operation: TerminalSignalOperation,
}

impl TerminalSignalArguments {
    fn from_environment() -> Result<Self> {
        let mut arguments = std::env::args_os().skip(1);
        let mut control_socket = None;
        let mut terminal = None;
        let mut operation = None;

        while let Some(argument) = arguments.next() {
            match argument.to_string_lossy().as_ref() {
                "--control-socket" => control_socket = arguments.next().map(PathBuf::from),
                "--terminal" | "--name" => {
                    terminal = arguments
                        .next()
                        .map(|value| TerminalName::new(value.to_string_lossy().into_owned()))
                }
                "connect" => operation = Some(TerminalSignalOperation::Connect),
                "input" => {
                    operation = Some(TerminalSignalOperation::Input {
                        bytes: Self::required_text(arguments.next(), "input")?.into_bytes(),
                    });
                    break;
                }
                "prompt" => {
                    operation = Some(TerminalSignalOperation::Prompt {
                        text: Self::required_text(arguments.next(), "prompt")?,
                    });
                    break;
                }
                "capture" => operation = Some(TerminalSignalOperation::Capture),
                "resize" => {
                    operation = Some(TerminalSignalOperation::Resize {
                        rows: Self::required_u16(arguments.next(), "rows")?,
                        columns: Self::required_u16(arguments.next(), "columns")?,
                    });
                    break;
                }
                "register-literal-prompt" | "register-literal" => {
                    operation = Some(TerminalSignalOperation::RegisterLiteralPrompt {
                        suffix: Self::required_text(arguments.next(), "suffix")?.into_bytes(),
                    });
                    break;
                }
                "register-regex-prompt" | "register-regex" => {
                    operation = Some(TerminalSignalOperation::RegisterRegexPrompt {
                        pattern: Self::required_text(arguments.next(), "pattern")?.into_bytes(),
                    });
                    break;
                }
                "unregister-prompt" => {
                    operation = Some(TerminalSignalOperation::UnregisterPrompt {
                        pattern_id: Self::required_text(arguments.next(), "pattern-id")?,
                    });
                    break;
                }
                "list-prompts" => operation = Some(TerminalSignalOperation::ListPrompts),
                "acquire-gate" => {
                    operation = Some(TerminalSignalOperation::AcquireGate {
                        pattern_id: arguments
                            .next()
                            .map(|value| value.to_string_lossy().into_owned()),
                    });
                    break;
                }
                "release-gate" => {
                    operation = Some(TerminalSignalOperation::ReleaseGate {
                        lease_id: Self::required_u64(arguments.next(), "lease-id")?,
                    });
                    break;
                }
                "inject" => {
                    operation = Some(TerminalSignalOperation::Inject {
                        lease_id: Self::required_u64(arguments.next(), "lease-id")?,
                        bytes: Self::required_text(arguments.next(), "bytes")?.into_bytes(),
                    });
                    break;
                }
                "inject-prompt" => {
                    operation = Some(TerminalSignalOperation::InjectPrompt {
                        lease_id: Self::required_u64(arguments.next(), "lease-id")?,
                        text: Self::required_text(arguments.next(), "text")?,
                    });
                    break;
                }
                "worker-lifecycle" => {
                    operation = Some(TerminalSignalOperation::WorkerLifecycleSnapshot);
                    break;
                }
                value if control_socket.is_none() => control_socket = Some(PathBuf::from(value)),
                value if terminal.is_none() => {
                    terminal = Some(TerminalName::new(value.to_string()))
                }
                _ => {}
            }
        }

        Ok(Self {
            control_socket: control_socket.unwrap_or_else(|| PathBuf::from(DEFAULT_CONTROL_SOCKET)),
            terminal: terminal.unwrap_or_else(|| TerminalName::new(DEFAULT_TERMINAL.to_string())),
            operation: operation.unwrap_or(TerminalSignalOperation::Connect),
        })
    }

    fn required_text(value: Option<OsString>, field: &str) -> Result<String> {
        value
            .map(|value| value.to_string_lossy().into_owned())
            .ok_or_else(|| Error::InvalidArgument {
                detail: format!("missing {field}"),
            })
    }

    fn required_u16(value: Option<OsString>, field: &str) -> Result<u16> {
        Self::required_text(value, field)?
            .parse::<u16>()
            .map_err(|_| Error::InvalidArgument {
                detail: format!("invalid {field}"),
            })
    }

    fn required_u64(value: Option<OsString>, field: &str) -> Result<u64> {
        Self::required_text(value, field)?
            .parse::<u64>()
            .map_err(|_| Error::InvalidArgument {
                detail: format!("invalid {field}"),
            })
    }

    fn into_request(self) -> TerminalSignalRequest {
        TerminalSignalRequest::new(self.control_socket, self.terminal, self.operation)
    }
}

struct TerminalSignalRequestFrame {
    request: Input,
}

impl TerminalSignalRequestFrame {
    fn new(request: Input) -> Self {
        Self { request }
    }

    fn into_request(self) -> Result<Input> {
        let frame = Frame::new(FrameBody::Request {
            exchange: synthetic_exchange(),
            request: Request::from_payload(self.request),
        });
        let bytes = frame.encode_length_prefixed()?;
        let decoded = Frame::decode_length_prefixed(&bytes)?;
        match decoded.into_body() {
            FrameBody::Request { request, .. } => {
                let (payload, tail) = request.payloads.into_head_and_tail();
                if tail.is_empty() {
                    Ok(payload)
                } else {
                    Err(Error::InvalidArgument {
                        detail: format!(
                            "expected one signal request payload, got {}",
                            tail.len() + 1
                        ),
                    })
                }
            }
            other => Err(Error::InvalidArgument {
                detail: format!("unexpected signal request frame: {other:?}"),
            }),
        }
    }
}

struct TerminalSignalEventFrame {
    event: Output,
}

impl TerminalSignalEventFrame {
    fn new(event: Output) -> Self {
        Self { event }
    }

    fn into_event(self) -> Result<Output> {
        let frame = Frame::new(FrameBody::Reply {
            exchange: synthetic_exchange(),
            reply: Reply::committed(NonEmpty::single(SubReply::Ok(self.event))),
        });
        let bytes = frame.encode_length_prefixed()?;
        let decoded = Frame::decode_length_prefixed(&bytes)?;
        match decoded.into_body() {
            FrameBody::Reply { reply, .. } => match reply {
                Reply::Accepted { per_operation, .. } => match per_operation.into_head() {
                    SubReply::Ok(payload) => Ok(payload),
                    other => Err(Error::InvalidArgument {
                        detail: format!("unexpected signal sub-reply: {other:?}"),
                    }),
                },
                Reply::Rejected { reason } => Err(Error::InvalidArgument {
                    detail: format!("signal reply rejected: {reason:?}"),
                }),
            },
            other => Err(Error::InvalidArgument {
                detail: format!("unexpected signal reply frame: {other:?}"),
            }),
        }
    }
}

struct TerminalEventLine {
    event: Output,
}

impl TerminalEventLine {
    fn new(event: Output) -> Self {
        Self { event }
    }

    fn write_to(&self, output: &mut impl Write) -> Result<()> {
        match &self.event {
            Output::TerminalReady(TerminalReady {
                terminal,
                generation,
            }) => writeln!(
                output,
                "TerminalReady\t{}\t{}",
                terminal.as_str(),
                generation.clone().into_u64()
            )?,
            Output::TerminalInputAccepted(TerminalInputAccepted {
                terminal,
                generation,
            }) => writeln!(
                output,
                "TerminalInputAccepted\t{}\t{}",
                terminal.as_str(),
                generation.clone().into_u64()
            )?,
            Output::TranscriptDelta(TranscriptDelta {
                terminal,
                sequence,
                bytes,
            }) => writeln!(
                output,
                "TranscriptDelta\t{}\t{}\t{}",
                terminal.as_str(),
                sequence.clone().into_u64(),
                HexBytes::new(bytes.as_slice())
            )?,
            Output::TerminalResized(TerminalResized {
                terminal,
                rows,
                columns,
                generation,
            }) => writeln!(
                output,
                "TerminalResized\t{}\t{}\t{}\t{}",
                terminal.as_str(),
                rows.clone().into_u16(),
                columns.clone().into_u16(),
                generation.clone().into_u64()
            )?,
            Output::TerminalCaptured(TerminalCaptured {
                terminal,
                generation,
                bytes,
            }) => writeln!(
                output,
                "TerminalCaptured\t{}\t{}\t{}",
                terminal.as_str(),
                generation.clone().into_u64(),
                HexBytes::new(bytes.as_slice())
            )?,
            Output::TerminalDetached(TerminalDetached {
                terminal,
                generation,
                reason,
            }) => writeln!(
                output,
                "TerminalDetached\t{}\t{}\t{reason:?}",
                terminal.as_str(),
                generation.clone().into_u64()
            )?,
            Output::TerminalExited(exited) => writeln!(
                output,
                "TerminalExited\t{}\t{}\t{:?}",
                exited.terminal.as_str(),
                exited.generation.clone().into_u64(),
                exited.status
            )?,
            Output::TerminalRejected(TerminalRejected { terminal, reason }) => writeln!(
                output,
                "TerminalRejected\t{}\t{reason:?}",
                terminal.as_str()
            )?,
            Output::PromptPatternRegistered(PromptPatternRegistered {
                terminal,
                pattern_id,
            }) => writeln!(
                output,
                "PromptPatternRegistered\t{}\t{}",
                terminal.as_str(),
                pattern_id.as_str()
            )?,
            Output::PromptPatternUnregistered(PromptPatternUnregistered {
                terminal,
                pattern_id,
            }) => writeln!(
                output,
                "PromptPatternUnregistered\t{}\t{}",
                terminal.as_str(),
                pattern_id.as_str()
            )?,
            Output::PromptPatternList(PromptPatternList { terminal, entries }) => writeln!(
                output,
                "PromptPatternList\t{}\t{}",
                terminal.as_str(),
                entries.len()
            )?,
            Output::GateAcquired(GateAcquired {
                terminal,
                lease,
                prompt_state,
            }) => writeln!(
                output,
                "GateAcquired\t{}\t{}\t{}",
                terminal.as_str(),
                lease.0.clone().into_u64(),
                PromptStateText::new(prompt_state)
            )?,
            Output::GateBusy(GateBusy {
                terminal,
                current_holder,
            }) => writeln!(
                output,
                "GateBusy\t{}\t{}",
                terminal.as_str(),
                current_holder.clone().into_u64()
            )?,
            Output::GateReleased(GateReleased {
                terminal,
                lease,
                cached_human_bytes,
            }) => writeln!(
                output,
                "GateReleased\t{}\t{}\t{}",
                terminal.as_str(),
                lease.0.clone().into_u64(),
                cached_human_bytes.clone().into_u64()
            )?,
            Output::InjectionAck(InjectionAck {
                terminal,
                generation,
                sequence,
            }) => writeln!(
                output,
                "InjectionAck\t{}\t{}\t{}",
                terminal.as_str(),
                generation.clone().into_u64(),
                sequence.clone().into_u64()
            )?,
            Output::InjectionRejected(InjectionRejected { terminal, reason }) => writeln!(
                output,
                "InjectionRejected\t{}\t{reason:?}",
                terminal.as_str()
            )?,
            Output::TerminalWorkerLifecycleSnapshot(TerminalWorkerLifecycleSnapshot {
                terminal,
                observations,
            }) => writeln!(
                output,
                "TerminalWorkerLifecycleSnapshot\t{}\t{}",
                terminal.as_str(),
                observations.len()
            )?,
            Output::SubscriptionRetracted(SubscriptionRetracted(token)) => {
                writeln!(output, "SubscriptionRetracted\t{}", token.0.as_str())?
            } // Per /176 §1 + /177 §3, TerminalWorkerLifecycleEvent now
            // belongs to the streaming TerminalEvent enum — it arrives
            // via StreamingFrameBody::SubscriptionEvent, not as a
            // reply. The CLI's reply-reading path no longer receives
            // it; a separate subscription-event reader handles those.
            Output::SessionList(SessionList(entries)) => {
                writeln!(output, "SessionList\t{}", entries.len())?
            }
            Output::SessionResolved(SessionResolved {
                name,
                data_socket_path,
            }) => writeln!(
                output,
                "SessionResolved\t{}\t{}",
                name.as_str(),
                data_socket_path.as_str()
            )?,
            // Per /176 §1 + /177 §3, a streaming TerminalEvent rides the
            // SubscriptionEvent frame path, not the direct-reply path; the
            // CLI's reply reader never receives one. Render defensively in
            // case the daemon wraps an event into a reply.
            Output::Event(event) => writeln!(output, "Event\t{event:?}")?,
        }
        Ok(())
    }
}

struct PromptStateText<'state> {
    state: &'state PromptState,
}

impl<'state> PromptStateText<'state> {
    fn new(state: &'state PromptState) -> Self {
        Self { state }
    }
}

impl std::fmt::Display for PromptStateText<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.state {
            PromptState::NotChecked => formatter.write_str("NotChecked"),
            PromptState::Clean => formatter.write_str("Clean"),
            PromptState::Dirty(trailing_count) => {
                write!(formatter, "Dirty:{}", trailing_count.clone().into_u64())
            }
        }
    }
}

struct HexBytes<'bytes> {
    bytes: &'bytes [u64],
}

impl<'bytes> HexBytes<'bytes> {
    fn new(bytes: &'bytes [u64]) -> Self {
        Self { bytes }
    }
}

impl std::fmt::Display for HexBytes<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in self.bytes {
            write!(formatter, "{:02x}", *byte as u8)?;
        }
        Ok(())
    }
}
