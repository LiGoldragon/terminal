use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;

use signal_terminal::{
    AcquireInputGateRequest, InputGateLease, ListPromptPatternsRequest, PromptPattern, PromptState,
    Query, RegisterPromptPatternRequest, ReleaseInputGateRequest, Response,
    SubscribeTerminalWorkerLifecycleRequest, TerminalCaptureRequest, TerminalConnectionRequest,
    TerminalInputRequest, TerminalName, TerminalResizeRequest, UnregisterPromptPatternRequest,
    WriteInjectionRequest,
};

use crate::contract::widen_bytes;
use crate::pty::TerminalSocket;
use crate::{Error, Result, frame};

const DEFAULT_CONTROL_SOCKET: &str = "/tmp/terminal.control.sock";
const DEFAULT_TERMINAL: &str = "operator";

#[derive(Debug, Clone, PartialEq)]
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

    /// Build the query, round-trip it through a `signal-terminal` Signal
    /// frame, cross the socket, round-trip the reply through a frame, and
    /// print one event line.
    ///
    /// The round-trips are the witness this CLI exists to be: they prove the
    /// value the CLI built survives the archive the wire carries.
    pub fn run(self, mut output: impl Write) -> Result<()> {
        let query = self.operation.into_query(self.terminal);
        let query = round_trip_query(&query)?;
        let event = TerminalSocket::from_control_socket(self.control_socket).signal(query)?;
        let event = round_trip_response(&event)?;
        TerminalEventLine::new(event).write_to(&mut output)?;
        output.flush()?;
        Ok(())
    }
}

/// Archive one query into a Signal frame and restore it.
fn round_trip_query(query: &Query) -> Result<Query> {
    let mut bytes = Vec::new();
    frame::terminal::write_query(&mut bytes, query)?;
    frame::terminal::read_query(&mut bytes.as_slice())
}

/// Archive one response into a Signal frame and restore it.
fn round_trip_response(response: &Response) -> Result<Response> {
    let mut bytes = Vec::new();
    frame::terminal::write_response(&mut bytes, response)?;
    frame::terminal::read_response(&mut bytes.as_slice())
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
    ReleaseGate { lease_id: i64 },
    Inject { lease_id: i64, bytes: Vec<u8> },
    InjectPrompt { lease_id: i64, text: String },
    WorkerLifecycleSnapshot,
}

impl TerminalSignalOperation {
    fn into_query(self, terminal: TerminalName) -> Query {
        match self {
            Self::Connect => Query::TerminalConnection(TerminalConnectionRequest { terminal }),
            Self::Input { bytes } => Query::TerminalInput(TerminalInputRequest {
                terminal,
                input_bytes: widen_bytes(&bytes),
            }),
            Self::Prompt { text } => {
                let mut bytes = text.into_bytes();
                bytes.push(b'\r');
                Query::TerminalInput(TerminalInputRequest {
                    terminal,
                    input_bytes: widen_bytes(&bytes),
                })
            }
            Self::Capture => Query::TerminalCapture(TerminalCaptureRequest { terminal }),
            Self::Resize { rows, columns } => Query::TerminalResize(TerminalResizeRequest {
                terminal,
                rows: i64::from(rows),
                columns: i64::from(columns),
            }),
            Self::RegisterLiteralPrompt { suffix } => {
                Query::RegisterPromptPattern(RegisterPromptPatternRequest {
                    terminal,
                    pattern: PromptPattern::LiteralSuffix(widen_bytes(&suffix)),
                })
            }
            Self::RegisterRegexPrompt { pattern } => {
                Query::RegisterPromptPattern(RegisterPromptPatternRequest {
                    terminal,
                    pattern: PromptPattern::RegexSuffix(widen_bytes(&pattern)),
                })
            }
            Self::UnregisterPrompt { pattern_id } => {
                Query::UnregisterPromptPattern(UnregisterPromptPatternRequest {
                    terminal,
                    pattern_identifier: pattern_id,
                })
            }
            Self::ListPrompts => {
                Query::ListPromptPatterns(ListPromptPatternsRequest { terminal })
            }
            Self::AcquireGate { pattern_id } => {
                Query::AcquireInputGate(AcquireInputGateRequest {
                    terminal,
                    input_gate_reason: "terminal signal cli".to_string(),
                    prompt_pattern_identifier_selection: pattern_id,
                })
            }
            Self::ReleaseGate { lease_id } => Query::ReleaseInputGate(ReleaseInputGateRequest {
                terminal,
                lease: lease(lease_id),
            }),
            Self::Inject { lease_id, bytes } => Query::WriteInjection(WriteInjectionRequest {
                terminal,
                lease: lease(lease_id),
                input_bytes: widen_bytes(&bytes),
            }),
            Self::InjectPrompt { lease_id, text } => {
                let mut bytes = text.into_bytes();
                bytes.push(b'\r');
                Query::WriteInjection(WriteInjectionRequest {
                    terminal,
                    lease: lease(lease_id),
                    input_bytes: widen_bytes(&bytes),
                })
            }
            Self::WorkerLifecycleSnapshot => Query::SubscribeTerminalWorkerLifecycle(
                SubscribeTerminalWorkerLifecycleRequest { terminal },
            ),
        }
    }
}

fn lease(identifier: i64) -> InputGateLease {
    InputGateLease {
        input_gate_lease_identifier: identifier,
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
                        .map(|value| value.to_string_lossy().into_owned())
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
                        lease_id: Self::required_i64(arguments.next(), "lease-id")?,
                    });
                    break;
                }
                "inject" => {
                    operation = Some(TerminalSignalOperation::Inject {
                        lease_id: Self::required_i64(arguments.next(), "lease-id")?,
                        bytes: Self::required_text(arguments.next(), "bytes")?.into_bytes(),
                    });
                    break;
                }
                "inject-prompt" => {
                    operation = Some(TerminalSignalOperation::InjectPrompt {
                        lease_id: Self::required_i64(arguments.next(), "lease-id")?,
                        text: Self::required_text(arguments.next(), "text")?,
                    });
                    break;
                }
                "worker-lifecycle" => {
                    operation = Some(TerminalSignalOperation::WorkerLifecycleSnapshot);
                    break;
                }
                value if control_socket.is_none() => control_socket = Some(PathBuf::from(value)),
                value if terminal.is_none() => terminal = Some(value.to_string()),
                _ => {}
            }
        }

        Ok(Self {
            control_socket: control_socket.unwrap_or_else(|| PathBuf::from(DEFAULT_CONTROL_SOCKET)),
            terminal: terminal.unwrap_or_else(|| DEFAULT_TERMINAL.to_string()),
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

    fn required_i64(value: Option<OsString>, field: &str) -> Result<i64> {
        Self::required_text(value, field)?
            .parse::<i64>()
            .map_err(|_| Error::InvalidArgument {
                detail: format!("invalid {field}"),
            })
    }

    fn into_request(self) -> TerminalSignalRequest {
        TerminalSignalRequest::new(self.control_socket, self.terminal, self.operation)
    }
}

/// One reply, rendered as one tab-separated line.
///
/// The line is this CLI's own product — the shape `terminal-validate-capture`
/// and the witnesses read — not a contract text form. A contract value's text
/// form is Datom, and the `terminal` CLI prints that.
struct TerminalEventLine {
    event: Response,
}

impl TerminalEventLine {
    fn new(event: Response) -> Self {
        Self { event }
    }

    fn write_to(&self, output: &mut impl Write) -> Result<()> {
        match &self.event {
            Response::TerminalReady(ready) => writeln!(
                output,
                "TerminalReady\t{}\t{}",
                ready.terminal, ready.generation
            )?,
            Response::TerminalInputAccepted(accepted) => writeln!(
                output,
                "TerminalInputAccepted\t{}\t{}",
                accepted.terminal, accepted.generation
            )?,
            Response::TranscriptDelta(delta) => writeln!(
                output,
                "TranscriptDelta\t{}\t{}\t{}",
                delta.terminal,
                delta.sequence,
                HexBytes::new(&delta.transcript_bytes)
            )?,
            Response::TerminalResized(resized) => writeln!(
                output,
                "TerminalResized\t{}\t{}\t{}\t{}",
                resized.terminal, resized.rows, resized.columns, resized.generation
            )?,
            Response::TerminalCaptured(captured) => writeln!(
                output,
                "TerminalCaptured\t{}\t{}\t{}",
                captured.terminal,
                captured.generation,
                HexBytes::new(&captured.transcript_bytes)
            )?,
            Response::TerminalDetached(detached) => writeln!(
                output,
                "TerminalDetached\t{}\t{}\t{:?}",
                detached.terminal, detached.generation, detached.terminal_detachment_reason
            )?,
            Response::TerminalExited(exited) => writeln!(
                output,
                "TerminalExited\t{}\t{}\t{:?}",
                exited.terminal, exited.generation, exited.terminal_exit_status
            )?,
            Response::TerminalRejected(rejected) => writeln!(
                output,
                "TerminalRejected\t{}\t{:?}",
                rejected.terminal, rejected.terminal_rejection_reason
            )?,
            Response::PromptPatternRegistered(registered) => writeln!(
                output,
                "PromptPatternRegistered\t{}\t{}",
                registered.terminal, registered.pattern_identifier
            )?,
            Response::PromptPatternUnregistered(unregistered) => writeln!(
                output,
                "PromptPatternUnregistered\t{}\t{}",
                unregistered.terminal, unregistered.pattern_identifier
            )?,
            Response::PromptPatternList(list) => writeln!(
                output,
                "PromptPatternList\t{}\t{}",
                list.terminal,
                list.entries.len()
            )?,
            Response::GateAcquired(acquired) => writeln!(
                output,
                "GateAcquired\t{}\t{}\t{}",
                acquired.terminal,
                acquired.lease.input_gate_lease_identifier,
                PromptStateText::new(&acquired.prompt_state)
            )?,
            Response::GateBusy(busy) => writeln!(
                output,
                "GateBusy\t{}\t{}",
                busy.terminal, busy.current_holder
            )?,
            Response::GateReleased(released) => writeln!(
                output,
                "GateReleased\t{}\t{}\t{}",
                released.terminal,
                released.lease.input_gate_lease_identifier,
                released.cached_human_bytes
            )?,
            Response::InjectionAck(ack) => writeln!(
                output,
                "InjectionAck\t{}\t{}\t{}",
                ack.terminal, ack.generation, ack.sequence
            )?,
            Response::InjectionRejected(rejected) => writeln!(
                output,
                "InjectionRejected\t{}\t{:?}",
                rejected.terminal, rejected.injection_rejection_reason
            )?,
            Response::TerminalWorkerLifecycleSnapshot(snapshot) => writeln!(
                output,
                "TerminalWorkerLifecycleSnapshot\t{}\t{}",
                snapshot.terminal,
                snapshot.observations.len()
            )?,
            Response::SubscriptionRetracted(retracted) => {
                writeln!(output, "SubscriptionRetracted\t{}", retracted.token.terminal)?
            }
            Response::SessionList(list) => {
                writeln!(output, "SessionList\t{}", list.session_entries.len())?
            }
            Response::SessionResolved(resolved) => writeln!(
                output,
                "SessionResolved\t{}\t{}",
                resolved.name, resolved.data_socket_path
            )?,
            Response::Event(event) => writeln!(output, "Event\t{event:?}")?,
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
            PromptState::Dirty(trailing_count) => write!(formatter, "Dirty:{trailing_count}"),
        }
    }
}

struct HexBytes<'bytes> {
    bytes: &'bytes [i64],
}

impl<'bytes> HexBytes<'bytes> {
    fn new(bytes: &'bytes [i64]) -> Self {
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
