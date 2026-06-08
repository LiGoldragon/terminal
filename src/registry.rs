use std::io::Write;
use std::path::PathBuf;

use signal_terminal::{TerminalName, TerminalSessionHealthObservation, TerminalSessionObservation};

use crate::Error;
use crate::Result;
use crate::tables::{StoreLocation, TerminalTables};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionListRequest {
    store: StoreLocation,
}

impl SessionListRequest {
    pub fn from_environment() -> Self {
        Self {
            store: SessionArguments::from_environment().store(),
        }
    }

    pub fn run(self, mut output: impl Write) -> Result<()> {
        for session in TerminalTables::open(&self.store)?.sessions()? {
            SessionLine::new(session).write_to(&mut output)?;
        }
        output.flush()?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionResolveRequest {
    store: StoreLocation,
    terminal: TerminalName,
}

impl SessionResolveRequest {
    pub fn new(store: StoreLocation, terminal: TerminalName) -> Self {
        Self { store, terminal }
    }

    pub fn from_environment() -> Self {
        let arguments = SessionArguments::from_environment();
        Self::new(arguments.store(), arguments.terminal())
    }

    pub fn run(self, mut output: impl Write) -> Result<()> {
        if let Some(session) = TerminalTables::open(&self.store)?.session(&self.terminal)? {
            writeln!(output, "{}", session.control_socket_path().as_str())?;
        } else {
            return Err(Error::UnknownTerminalSession {
                terminal: self.terminal.as_str().to_string(),
            });
        }
        output.flush()?;
        Ok(())
    }
}

struct SessionLine {
    session: TerminalSessionObservation,
}

impl SessionLine {
    fn new(session: TerminalSessionObservation) -> Self {
        Self { session }
    }

    fn write_to(&self, output: &mut impl Write) -> Result<()> {
        writeln!(
            output,
            "{}\t{}\t{}\t{}\t{}\t{}",
            self.session.terminal().as_str(),
            self.session.control_socket_path().as_str(),
            self.session.data_socket_path().as_str(),
            self.session.state().as_str(),
            self.session.generation().clone().into_u64(),
            self.session.transcript_sequence().clone().into_u64()
        )?;
        Ok(())
    }
}

struct SessionArguments {
    store: StoreLocation,
    terminal: Option<TerminalName>,
}

impl SessionArguments {
    fn from_environment() -> Self {
        let mut arguments = std::env::args_os().skip(1);
        let mut store = None;
        let mut terminal = None;

        while let Some(argument) = arguments.next() {
            match argument.to_string_lossy().as_ref() {
                "--store" => store = arguments.next().map(StoreLocation::new),
                "--terminal" | "--name" => {
                    terminal = arguments
                        .next()
                        .map(|value| TerminalName::new(value.to_string_lossy().into_owned()))
                }
                value if terminal.is_none() => {
                    terminal = Some(TerminalName::new(value.to_string()))
                }
                _ => {}
            }
        }

        Self {
            store: store.unwrap_or_else(StoreLocation::from_environment),
            terminal,
        }
    }

    fn store(&self) -> StoreLocation {
        self.store.clone()
    }

    fn terminal(&self) -> TerminalName {
        self.terminal
            .clone()
            .unwrap_or_else(|| TerminalName::new("default".to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRegistration {
    store: StoreLocation,
    session: TerminalSessionObservation,
}

impl SessionRegistration {
    pub fn ready(
        store: StoreLocation,
        terminal: TerminalName,
        control_socket_path: impl Into<PathBuf>,
        data_socket_path: impl Into<PathBuf>,
    ) -> Self {
        let control_socket_path = control_socket_path.into().to_string_lossy().into_owned();
        let data_socket_path = data_socket_path.into().to_string_lossy().into_owned();
        Self {
            store,
            session: TerminalSessionObservation::ready(
                terminal,
                control_socket_path,
                data_socket_path,
            ),
        }
    }

    pub fn record(&self) -> Result<()> {
        let tables = TerminalTables::open(&self.store)?;
        tables.put_session(&self.session)?;
        tables.put_session_health(&TerminalSessionHealthObservation::new(
            self.session.terminal().clone(),
            self.session.state(),
            self.session.generation().clone(),
        ))
    }
}
