use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use meta_signal_terminal::{
    MetaTerminalFrame, MetaTerminalFrameBody, MetaTerminalReply, MetaTerminalRequest,
};
use nota_next::{NotaEncode, NotaSource};
use signal_frame::{ExchangeIdentifier, ExchangeLane, LaneSequence, Reply, SessionEpoch, SubReply};
use triad_runtime::{ComponentCommand, FrameBody as RuntimeFrameBody, LengthPrefixedCodec};

use crate::cli_argument::NotaCommandText;
use crate::{Error, Result};

const DEFAULT_META_TERMINAL_SOCKET: &str = "/tmp/meta-terminal.sock";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaTerminalEndpoint {
    socket: PathBuf,
}

impl MetaTerminalEndpoint {
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
pub struct MetaTerminalClient {
    endpoint: MetaTerminalEndpoint,
    codec: LengthPrefixedCodec,
}

impl MetaTerminalClient {
    pub fn new(endpoint: MetaTerminalEndpoint) -> Self {
        Self {
            endpoint,
            codec: LengthPrefixedCodec::default(),
        }
    }

    pub fn submit(&self, request: MetaTerminalRequest) -> Result<MetaTerminalReply> {
        let exchange = self.exchange();
        let frame = MetaTerminalFrame::new(MetaTerminalFrameBody::Request {
            exchange,
            request: signal_frame::Request::from_payload(request),
        });
        let mut stream = UnixStream::connect(self.endpoint.as_path())?;
        self.codec
            .write_body(&mut stream, &RuntimeFrameBody::new(frame.encode()?))?;
        let body = self.codec.read_body(&mut stream)?;
        self.reply_from_frame(MetaTerminalFrame::decode(body.bytes())?)
    }

    fn exchange(&self) -> ExchangeIdentifier {
        let _endpoint = &self.endpoint;
        ExchangeIdentifier::new(
            SessionEpoch::new(0),
            ExchangeLane::Connector,
            LaneSequence::first(),
        )
    }

    fn reply_from_frame(&self, frame: MetaTerminalFrame) -> Result<MetaTerminalReply> {
        match frame.into_body() {
            MetaTerminalFrameBody::Reply { reply, .. } => self.reply_output(reply),
            other => Err(Error::UnexpectedSignalFrame {
                got: format!("{other:?}"),
            }),
        }
    }

    fn reply_output(&self, reply: Reply<MetaTerminalReply>) -> Result<MetaTerminalReply> {
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
pub struct MetaTerminalCommandLine {
    command: ComponentCommand,
    environment: MetaTerminalCommandEnvironment,
}

impl MetaTerminalCommandLine {
    pub fn from_env() -> Self {
        Self {
            command: ComponentCommand::from_environment(),
            environment: MetaTerminalCommandEnvironment::from_process(),
        }
    }

    pub fn from_arguments<Arguments, Argument>(arguments: Arguments) -> Self
    where
        Arguments: IntoIterator<Item = Argument>,
        Argument: Into<String>,
    {
        Self::from_arguments_with_environment(
            arguments,
            MetaTerminalCommandEnvironment::from_process(),
        )
    }

    pub fn from_arguments_with_environment<Arguments, Argument>(
        arguments: Arguments,
        environment: MetaTerminalCommandEnvironment,
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
        let request = MetaTerminalRequestText::from_command(self.command)?.into_request()?;
        let reply = MetaTerminalClient::new(self.environment.endpoint()).submit(request)?;
        writeln!(output, "{}", reply.to_nota())?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaTerminalCommandEnvironment {
    socket: String,
}

impl MetaTerminalCommandEnvironment {
    pub fn new(socket: impl Into<String>) -> Self {
        Self {
            socket: socket.into(),
        }
    }

    pub fn from_process() -> Self {
        Self::new(
            std::env::var("TERMINAL_META_SOCKET")
                .unwrap_or(DEFAULT_META_TERMINAL_SOCKET.to_string()),
        )
    }

    pub fn endpoint(&self) -> MetaTerminalEndpoint {
        MetaTerminalEndpoint::new(&self.socket)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MetaTerminalRequestText {
    text: NotaCommandText,
}

impl MetaTerminalRequestText {
    fn from_command(command: ComponentCommand) -> Result<Self> {
        Ok(Self {
            text: NotaCommandText::from_command(command)?,
        })
    }

    fn into_request(self) -> Result<MetaTerminalRequest> {
        Ok(NotaSource::new(self.text.as_str()).parse::<MetaTerminalRequest>()?)
    }
}
