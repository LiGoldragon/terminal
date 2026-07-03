use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use nota::NotaSource;
use signal_frame::{ExchangeIdentifier, ExchangeLane, LaneSequence, Reply, SessionEpoch, SubReply};
use signal_terminal::{Frame, FrameBody, Input, Output};
use triad_runtime::{ComponentCommand, FrameBody as RuntimeFrameBody, LengthPrefixedCodec};

use crate::cli_argument::NotaCommandText;
use crate::{Error, Result};

const DEFAULT_TERMINAL_SOCKET: &str = "/tmp/terminal.sock";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalEndpoint {
    socket: PathBuf,
}

impl TerminalEndpoint {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
        }
    }

    pub fn as_path(&self) -> &Path {
        &self.socket
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalClient {
    endpoint: TerminalEndpoint,
    codec: LengthPrefixedCodec,
}

impl TerminalClient {
    pub fn new(endpoint: TerminalEndpoint) -> Self {
        Self {
            endpoint,
            codec: LengthPrefixedCodec::default(),
        }
    }

    pub fn submit(&self, input: Input) -> Result<Output> {
        let exchange = self.exchange();
        let frame = Frame::new(FrameBody::Request {
            exchange,
            request: signal_frame::Request::from_payload(input),
        });
        let mut stream = UnixStream::connect(self.endpoint.as_path())?;
        self.codec
            .write_body(&mut stream, &RuntimeFrameBody::new(frame.encode()?))?;
        let body = self.codec.read_body(&mut stream)?;
        self.reply_from_frame(Frame::decode(body.bytes())?)
    }

    fn exchange(&self) -> ExchangeIdentifier {
        let _endpoint = &self.endpoint;
        ExchangeIdentifier::new(
            SessionEpoch::new(0),
            ExchangeLane::Connector,
            LaneSequence::first(),
        )
    }

    fn reply_from_frame(&self, frame: Frame) -> Result<Output> {
        match frame.into_body() {
            FrameBody::Reply { reply, .. } => self.reply_output(reply),
            other => Err(Error::UnexpectedSignalFrame {
                got: format!("{other:?}"),
            }),
        }
    }

    fn reply_output(&self, reply: Reply<Output>) -> Result<Output> {
        let _endpoint = &self.endpoint;
        match reply {
            Reply::Accepted { per_operation, .. } => match per_operation.into_head() {
                SubReply::Ok(output) => Ok(output),
                other => Err(Error::UnexpectedSignalFrame {
                    got: format!("{other:?}"),
                }),
            },
            Reply::Rejected { reason } => Err(Error::UnexpectedSignalFrame {
                got: reason.to_string(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalCommandLine {
    command: ComponentCommand,
    environment: TerminalCommandEnvironment,
}

impl TerminalCommandLine {
    pub fn from_env() -> Self {
        Self {
            command: ComponentCommand::from_environment(),
            environment: TerminalCommandEnvironment::from_process(),
        }
    }

    pub fn from_arguments<Arguments, Argument>(arguments: Arguments) -> Self
    where
        Arguments: IntoIterator<Item = Argument>,
        Argument: Into<String>,
    {
        Self::from_arguments_with_environment(arguments, TerminalCommandEnvironment::from_process())
    }

    pub fn from_arguments_with_environment<Arguments, Argument>(
        arguments: Arguments,
        environment: TerminalCommandEnvironment,
    ) -> Self
    where
        Arguments: IntoIterator<Item = Argument>,
        Argument: Into<String>,
    {
        Self {
            command: ComponentCommand::from_arguments(arguments),
            environment,
        }
    }

    pub fn run(self, mut output: impl Write) -> Result<()> {
        let input = TerminalInputText::from_command(self.command)?.into_input()?;
        let reply = TerminalClient::new(self.environment.endpoint()).submit(input)?;
        writeln!(output, "{reply}")?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalCommandEnvironment {
    socket: String,
}

impl TerminalCommandEnvironment {
    pub fn new(socket: impl Into<String>) -> Self {
        Self {
            socket: socket.into(),
        }
    }

    pub fn from_process() -> Self {
        Self::new(std::env::var("TERMINAL_SOCKET").unwrap_or(DEFAULT_TERMINAL_SOCKET.to_string()))
    }

    pub fn endpoint(&self) -> TerminalEndpoint {
        TerminalEndpoint::new(&self.socket)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalInputText {
    text: NotaCommandText,
}

impl TerminalInputText {
    fn from_command(command: ComponentCommand) -> Result<Self> {
        Ok(Self {
            text: NotaCommandText::from_command(command)?,
        })
    }

    fn into_input(self) -> Result<Input> {
        Ok(NotaSource::new(self.text.as_str()).parse::<Input>()?)
    }
}
