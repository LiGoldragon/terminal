use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use signal_terminal::{Query, Response};
use triad_runtime::ComponentCommand;

use crate::cli_argument::DatomCommandText;
use crate::{Result, datom_text, frame};

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

/// One ordinary `signal-terminal` exchange over the component
/// communication socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalClient {
    endpoint: TerminalEndpoint,
}

impl TerminalClient {
    pub fn new(endpoint: TerminalEndpoint) -> Self {
        Self { endpoint }
    }

    pub fn submit(&self, query: Query) -> Result<Response> {
        let mut stream = UnixStream::connect(self.endpoint.as_path())?;
        frame::terminal::write_query(&mut stream, &query)?;
        frame::terminal::read_response(&mut stream)
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
        let text = DatomCommandText::from_command(self.command)?;
        let query: Query = datom_text::actualize(text.as_str())?;
        let reply = TerminalClient::new(self.environment.endpoint()).submit(query)?;
        writeln!(output, "{}", datom_text::textualize(&reply))?;
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
