use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;

use signal_core::{Reply, Request, SemaVerb};
use signal_persona_terminal::{
    Frame, FrameBody, TerminalCapture, TerminalCaptured, TerminalColumns, TerminalConnection,
    TerminalDetached, TerminalEvent, TerminalInput, TerminalInputAccepted, TerminalInputBytes,
    TerminalName, TerminalReady, TerminalRejected, TerminalRequest, TerminalResize,
    TerminalResized, TerminalRows, TranscriptDelta,
};

use crate::contract::TerminalTransportBinding;
use crate::{Error, Result};

const DEFAULT_SOCKET: &str = "/tmp/persona-terminal.sock";
const DEFAULT_TERMINAL: &str = "operator";

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
        let mut binding =
            TerminalTransportBinding::from_socket_path(self.terminal.clone(), self.socket);
        let request = self.operation.into_request(self.terminal);
        let framed_request = TerminalSignalRequestFrame::new(request).into_request()?;
        let event = binding.handle_request(framed_request)?;
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
        let frame = Frame::new(FrameBody::Request(Request::assert(self.request)));
        let bytes = frame.encode_length_prefixed()?;
        let decoded = Frame::decode_length_prefixed(&bytes)?;
        match decoded.into_body() {
            FrameBody::Request(Request::Operation {
                verb: SemaVerb::Assert,
                payload,
            }) => Ok(payload),
            other => Err(Error::InvalidArgument {
                detail: format!("unexpected signal request frame: {other:?}"),
            }),
        }
    }
}

struct TerminalSignalEventFrame {
    event: TerminalEvent,
}

impl TerminalSignalEventFrame {
    fn new(event: TerminalEvent) -> Self {
        Self { event }
    }

    fn into_event(self) -> Result<TerminalEvent> {
        let frame = Frame::new(FrameBody::Reply(Reply::operation(self.event)));
        let bytes = frame.encode_length_prefixed()?;
        let decoded = Frame::decode_length_prefixed(&bytes)?;
        match decoded.into_body() {
            FrameBody::Reply(Reply::Operation(event)) => Ok(event),
            other => Err(Error::InvalidArgument {
                detail: format!("unexpected signal reply frame: {other:?}"),
            }),
        }
    }
}

struct TerminalEventLine {
    event: TerminalEvent,
}

impl TerminalEventLine {
    fn new(event: TerminalEvent) -> Self {
        Self { event }
    }

    fn write_to(&self, output: &mut impl Write) -> Result<()> {
        match &self.event {
            TerminalEvent::TerminalReady(TerminalReady {
                terminal,
                generation,
            }) => writeln!(
                output,
                "TerminalReady\t{}\t{}",
                terminal.as_str(),
                generation.into_u64()
            )?,
            TerminalEvent::TerminalInputAccepted(TerminalInputAccepted {
                terminal,
                generation,
            }) => writeln!(
                output,
                "TerminalInputAccepted\t{}\t{}",
                terminal.as_str(),
                generation.into_u64()
            )?,
            TerminalEvent::TranscriptDelta(TranscriptDelta {
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
            TerminalEvent::TerminalResized(TerminalResized {
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
            TerminalEvent::TerminalCaptured(TerminalCaptured {
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
            TerminalEvent::TerminalDetached(TerminalDetached {
                terminal,
                generation,
                reason,
            }) => writeln!(
                output,
                "TerminalDetached\t{}\t{}\t{reason:?}",
                terminal.as_str(),
                generation.into_u64()
            )?,
            TerminalEvent::TerminalExited(exited) => writeln!(
                output,
                "TerminalExited\t{}\t{}\t{:?}",
                exited.terminal.as_str(),
                exited.generation.into_u64(),
                exited.status
            )?,
            TerminalEvent::TerminalRejected(TerminalRejected { terminal, reason }) => writeln!(
                output,
                "TerminalRejected\t{}\t{reason:?}",
                terminal.as_str()
            )?,
        }
        Ok(())
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
