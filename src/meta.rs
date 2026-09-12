use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use meta_signal_terminal::{Query as MetaQuery, Response as MetaResponse};
use triad_runtime::ComponentCommand;

use crate::cli_argument::DatomCommandText;
use crate::{Result, datom_text, frame};

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

/// One `meta-signal-terminal` exchange over the owner-only meta socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaTerminalClient {
    endpoint: MetaTerminalEndpoint,
}

impl MetaTerminalClient {
    pub fn new(endpoint: MetaTerminalEndpoint) -> Self {
        Self { endpoint }
    }

    pub fn submit(&self, query: MetaQuery) -> Result<MetaResponse> {
        let mut stream = UnixStream::connect(self.endpoint.as_path())?;
        frame::meta::write_query(&mut stream, &query)?;
        frame::meta::read_response(&mut stream)
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
        let text = DatomCommandText::from_command(self.command)?;
        let query: MetaQuery = datom_text::actualize(text.as_str())?;
        let reply = MetaTerminalClient::new(self.environment.endpoint()).submit(query)?;
        writeln!(output, "{}", datom_text::textualize(&reply))?;
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
