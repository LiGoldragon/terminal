use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;

use signal_core::{
    ExchangeIdentifier, ExchangeLane, LaneSequence, NonEmpty, Reply, Request, SessionEpoch,
    SignalVerb, SubReply,
};
use signal_persona_terminal::{
    AcquireInputGate, GateAcquired, GateBusy, GateReleased, InjectionAck, InjectionRejected,
    InputGateLease, InputGateLeaseId, InputGateReason, ListPromptPatterns, PromptPattern,
    PromptPatternBytes, PromptPatternList, PromptPatternRegistered, PromptPatternUnregistered,
    PromptState, RegisterPromptPattern, ReleaseInputGate, SubscribeTerminalWorkerLifecycle,
    SubscriptionRetracted, TerminalCapture, TerminalCaptured, TerminalColumns, TerminalConnection,
    TerminalDetached, TerminalFrame as Frame, TerminalFrameBody as FrameBody, TerminalInput,
    TerminalInputAccepted, TerminalInputBytes, TerminalName, TerminalReady, TerminalRejected,
    TerminalReply, TerminalRequest, TerminalResize, TerminalResized, TerminalRows,
    TerminalWorkerLifecycleSnapshot, TranscriptDelta, UnregisterPromptPattern, WriteInjection,
};

use crate::pty::TerminalSocket;
use crate::{Error, Result};

const DEFAULT_SOCKET: &str = "/tmp/persona-terminal.sock";
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
    socket: PathBuf,
    terminal: TerminalName,
    operation: TerminalSignalOperation,
}

impl TerminalSignalRequest {
    pub fn from_environment() -> Result<Self> {
        Ok(TerminalSignalArguments::from_environment()?.into_request())
    }

    pub fn new(
        socket: impl Into<PathBuf>,
        terminal: TerminalName,
        operation: TerminalSignalOperation,
    ) -> Self {
        Self {
            socket: socket.into(),
            terminal,
            operation,
        }
    }

    pub fn run(self, mut output: impl Write) -> Result<()> {
        let request = self.operation.into_request(self.terminal);
        let framed_request = TerminalSignalRequestFrame::new(request).into_request()?;
        let event = TerminalSocket::from_path(self.socket).signal(framed_request)?;
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
    fn into_request(self, terminal: TerminalName) -> TerminalRequest {
        match self {
            Self::Connect => TerminalConnection { terminal }.into(),
            Self::Input { bytes } => TerminalInput {
                terminal,
                bytes: TerminalInputBytes::new(bytes),
            }
            .into(),
            Self::Prompt { text } => {
                let mut bytes = text.into_bytes();
                bytes.push(b'\r');
                TerminalInput {
                    terminal,
                    bytes: TerminalInputBytes::new(bytes),
                }
                .into()
            }
            Self::Capture => TerminalCapture { terminal }.into(),
            Self::Resize { rows, columns } => TerminalResize {
                terminal,
                rows: TerminalRows::new(rows),
                columns: TerminalColumns::new(columns),
            }
            .into(),
            Self::RegisterLiteralPrompt { suffix } => RegisterPromptPattern {
                terminal,
                pattern: PromptPattern::LiteralSuffix(PromptPatternBytes::new(suffix)),
            }
            .into(),
            Self::RegisterRegexPrompt { pattern } => RegisterPromptPattern {
                terminal,
                pattern: PromptPattern::RegexSuffix {
                    pattern: PromptPatternBytes::new(pattern),
                },
            }
            .into(),
            Self::UnregisterPrompt { pattern_id } => UnregisterPromptPattern {
                terminal,
                pattern_id: signal_persona_terminal::PromptPatternId::new(pattern_id),
            }
            .into(),
            Self::ListPrompts => ListPromptPatterns { terminal }.into(),
            Self::AcquireGate { pattern_id } => AcquireInputGate {
                terminal,
                reason: InputGateReason::new("persona-terminal signal cli"),
                prompt_pattern_id: pattern_id.map(signal_persona_terminal::PromptPatternId::new),
            }
            .into(),
            Self::ReleaseGate { lease_id } => ReleaseInputGate {
                terminal,
                lease: InputGateLease {
                    id: InputGateLeaseId::new(lease_id),
                },
            }
            .into(),
            Self::Inject { lease_id, bytes } => WriteInjection {
                terminal,
                lease: InputGateLease {
                    id: InputGateLeaseId::new(lease_id),
                },
                bytes: TerminalInputBytes::new(bytes),
            }
            .into(),
            Self::InjectPrompt { lease_id, text } => {
                let mut bytes = text.into_bytes();
                bytes.push(b'\r');
                WriteInjection {
                    terminal,
                    lease: InputGateLease {
                        id: InputGateLeaseId::new(lease_id),
                    },
                    bytes: TerminalInputBytes::new(bytes),
                }
                .into()
            }
            Self::WorkerLifecycleSnapshot => SubscribeTerminalWorkerLifecycle { terminal }.into(),
        }
    }
}

struct TerminalSignalArguments {
    socket: PathBuf,
    terminal: TerminalName,
    operation: TerminalSignalOperation,
}

impl TerminalSignalArguments {
    fn from_environment() -> Result<Self> {
        let mut arguments = std::env::args_os().skip(1);
        let mut socket = None;
        let mut terminal = None;
        let mut operation = None;

        while let Some(argument) = arguments.next() {
            match argument.to_string_lossy().as_ref() {
                "--socket" => socket = arguments.next().map(PathBuf::from),
                "--terminal" | "--name" => {
                    terminal = arguments
                        .next()
                        .map(|value| TerminalName::new(value.to_string_lossy()))
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
                value if socket.is_none() => socket = Some(PathBuf::from(value)),
                value if terminal.is_none() => terminal = Some(TerminalName::new(value)),
                _ => {}
            }
        }

        Ok(Self {
            socket: socket.unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET)),
            terminal: terminal.unwrap_or_else(|| TerminalName::new(DEFAULT_TERMINAL)),
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
        TerminalSignalRequest::new(self.socket, self.terminal, self.operation)
    }
}

struct TerminalSignalRequestFrame {
    request: TerminalRequest,
}

impl TerminalSignalRequestFrame {
    fn new(request: TerminalRequest) -> Self {
        Self { request }
    }

    fn into_request(self) -> Result<TerminalRequest> {
        let frame = Frame::new(FrameBody::Request {
            exchange: synthetic_exchange(),
            request: Request::from_payload(self.request),
        });
        let bytes = frame.encode_length_prefixed()?;
        let decoded = Frame::decode_length_prefixed(&bytes)?;
        match decoded.into_body() {
            FrameBody::Request { request, .. } => request
                .into_checked()
                .map_err(|(reason, _)| Error::InvalidSignalRequest { reason })
                .map(|checked| checked.operations.into_head().payload),
            other => Err(Error::InvalidArgument {
                detail: format!("unexpected signal request frame: {other:?}"),
            }),
        }
    }
}

struct TerminalSignalEventFrame {
    event: TerminalReply,
}

impl TerminalSignalEventFrame {
    fn new(event: TerminalReply) -> Self {
        Self { event }
    }

    fn into_event(self) -> Result<TerminalReply> {
        let frame = Frame::new(FrameBody::Reply {
            exchange: synthetic_exchange(),
            reply: Reply::completed(NonEmpty::single(SubReply::Ok {
                verb: SignalVerb::Subscribe,
                payload: self.event,
            })),
        });
        let bytes = frame.encode_length_prefixed()?;
        let decoded = Frame::decode_length_prefixed(&bytes)?;
        match decoded.into_body() {
            FrameBody::Reply { reply, .. } => match reply {
                Reply::Accepted { per_operation, .. } => match per_operation.into_head() {
                    SubReply::Ok { payload, .. } => Ok(payload),
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
    event: TerminalReply,
}

impl TerminalEventLine {
    fn new(event: TerminalReply) -> Self {
        Self { event }
    }

    fn write_to(&self, output: &mut impl Write) -> Result<()> {
        match &self.event {
            TerminalReply::TerminalReady(TerminalReady {
                terminal,
                generation,
            }) => writeln!(
                output,
                "TerminalReady\t{}\t{}",
                terminal.as_str(),
                generation.into_u64()
            )?,
            TerminalReply::TerminalInputAccepted(TerminalInputAccepted {
                terminal,
                generation,
            }) => writeln!(
                output,
                "TerminalInputAccepted\t{}\t{}",
                terminal.as_str(),
                generation.into_u64()
            )?,
            TerminalReply::TranscriptDelta(TranscriptDelta {
                terminal,
                sequence,
                bytes,
            }) => writeln!(
                output,
                "TranscriptDelta\t{}\t{}\t{}",
                terminal.as_str(),
                sequence.into_u64(),
                HexBytes::new(bytes.as_slice())
            )?,
            TerminalReply::TerminalResized(TerminalResized {
                terminal,
                rows,
                columns,
                generation,
            }) => writeln!(
                output,
                "TerminalResized\t{}\t{}\t{}\t{}",
                terminal.as_str(),
                rows.into_u16(),
                columns.into_u16(),
                generation.into_u64()
            )?,
            TerminalReply::TerminalCaptured(TerminalCaptured {
                terminal,
                generation,
                bytes,
            }) => writeln!(
                output,
                "TerminalCaptured\t{}\t{}\t{}",
                terminal.as_str(),
                generation.into_u64(),
                HexBytes::new(bytes.as_slice())
            )?,
            TerminalReply::TerminalDetached(TerminalDetached {
                terminal,
                generation,
                reason,
            }) => writeln!(
                output,
                "TerminalDetached\t{}\t{}\t{reason:?}",
                terminal.as_str(),
                generation.into_u64()
            )?,
            TerminalReply::TerminalExited(exited) => writeln!(
                output,
                "TerminalExited\t{}\t{}\t{:?}",
                exited.terminal.as_str(),
                exited.generation.into_u64(),
                exited.status
            )?,
            TerminalReply::TerminalRejected(TerminalRejected { terminal, reason }) => writeln!(
                output,
                "TerminalRejected\t{}\t{reason:?}",
                terminal.as_str()
            )?,
            TerminalReply::PromptPatternRegistered(PromptPatternRegistered {
                terminal,
                pattern_id,
            }) => writeln!(
                output,
                "PromptPatternRegistered\t{}\t{}",
                terminal.as_str(),
                pattern_id.as_str()
            )?,
            TerminalReply::PromptPatternUnregistered(PromptPatternUnregistered {
                terminal,
                pattern_id,
            }) => writeln!(
                output,
                "PromptPatternUnregistered\t{}\t{}",
                terminal.as_str(),
                pattern_id.as_str()
            )?,
            TerminalReply::PromptPatternList(PromptPatternList { terminal, entries }) => writeln!(
                output,
                "PromptPatternList\t{}\t{}",
                terminal.as_str(),
                entries.len()
            )?,
            TerminalReply::GateAcquired(GateAcquired {
                terminal,
                lease,
                prompt_state,
            }) => writeln!(
                output,
                "GateAcquired\t{}\t{}\t{}",
                terminal.as_str(),
                lease.id.into_u64(),
                PromptStateText::new(prompt_state)
            )?,
            TerminalReply::GateBusy(GateBusy {
                terminal,
                current_holder,
            }) => writeln!(
                output,
                "GateBusy\t{}\t{}",
                terminal.as_str(),
                current_holder.into_u64()
            )?,
            TerminalReply::GateReleased(GateReleased {
                terminal,
                lease,
                cached_human_bytes,
            }) => writeln!(
                output,
                "GateReleased\t{}\t{}\t{}",
                terminal.as_str(),
                lease.id.into_u64(),
                cached_human_bytes.into_u64()
            )?,
            TerminalReply::InjectionAck(InjectionAck {
                terminal,
                generation,
                sequence,
            }) => writeln!(
                output,
                "InjectionAck\t{}\t{}\t{}",
                terminal.as_str(),
                generation.into_u64(),
                sequence.into_u64()
            )?,
            TerminalReply::InjectionRejected(InjectionRejected { terminal, reason }) => writeln!(
                output,
                "InjectionRejected\t{}\t{reason:?}",
                terminal.as_str()
            )?,
            TerminalReply::TerminalWorkerLifecycleSnapshot(TerminalWorkerLifecycleSnapshot {
                terminal,
                observations,
            }) => writeln!(
                output,
                "TerminalWorkerLifecycleSnapshot\t{}\t{}",
                terminal.as_str(),
                observations.len()
            )?,
            TerminalReply::SubscriptionRetracted(SubscriptionRetracted { token }) => {
                writeln!(output, "SubscriptionRetracted\t{}", token.terminal.as_str())?
            }
            // Per /176 §1 + /177 §3, TerminalWorkerLifecycleEvent now
            // belongs to the streaming TerminalEvent enum — it arrives
            // via StreamingFrameBody::SubscriptionEvent, not as a
            // reply. The CLI's reply-reading path no longer receives
            // it; a separate subscription-event reader handles those.
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
            PromptState::Dirty { trailing_count } => {
                write!(formatter, "Dirty:{}", trailing_count.into_u64())
            }
        }
    }
}

struct HexBytes<'bytes> {
    bytes: &'bytes [u8],
}

impl<'bytes> HexBytes<'bytes> {
    fn new(bytes: &'bytes [u8]) -> Self {
        Self { bytes }
    }
}

impl std::fmt::Display for HexBytes<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in self.bytes {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}
