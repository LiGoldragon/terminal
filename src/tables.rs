use std::path::{Path, PathBuf};

use sema::{Schema, SchemaVersion, Sema, Table};
use signal_persona_terminal::{TerminalGeneration, TerminalName, TerminalSequence};

use crate::Result;

const TERMINAL_SCHEMA: Schema = Schema {
    version: SchemaVersion::new(1),
};

const SESSIONS: Table<&'static str, StoredTerminalSession> = Table::new("sessions");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreLocation {
    path: PathBuf,
}

impl StoreLocation {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn from_environment() -> Self {
        match std::env::var_os("PERSONA_TERMINAL_STORE") {
            Some(path) => Self::new(path),
            None => Self::new("/tmp/persona-terminal.redb"),
        }
    }

    pub fn as_path(&self) -> &Path {
        self.path.as_path()
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct StoredTerminalSession {
    terminal: TerminalName,
    socket_path: String,
    generation: TerminalGeneration,
    transcript_sequence: TerminalSequence,
    state: TerminalSessionState,
}

impl StoredTerminalSession {
    pub fn ready(terminal: TerminalName, socket_path: impl Into<PathBuf>) -> Self {
        Self {
            terminal,
            socket_path: socket_path.into().to_string_lossy().into_owned(),
            generation: TerminalGeneration::new(1),
            transcript_sequence: TerminalSequence::new(0),
            state: TerminalSessionState::Ready,
        }
    }

    pub fn terminal(&self) -> &TerminalName {
        &self.terminal
    }

    pub fn socket_path(&self) -> PathBuf {
        PathBuf::from(&self.socket_path)
    }

    pub fn socket_path_text(&self) -> &str {
        self.socket_path.as_str()
    }

    pub fn generation(&self) -> TerminalGeneration {
        self.generation
    }

    pub fn transcript_sequence(&self) -> TerminalSequence {
        self.transcript_sequence
    }

    pub fn state(&self) -> TerminalSessionState {
        self.state
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalSessionState {
    Ready,
    Exited,
}

impl TerminalSessionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Exited => "exited",
        }
    }
}

pub struct TerminalTables {
    database: Sema,
}

impl TerminalTables {
    pub fn open(store: &StoreLocation) -> Result<Self> {
        let database = Sema::open_with_schema(store.as_path(), &TERMINAL_SCHEMA)?;
        database.write(|transaction| {
            SESSIONS.ensure(transaction)?;
            Ok(())
        })?;
        Ok(Self { database })
    }

    pub fn put_session(&self, session: &StoredTerminalSession) -> Result<()> {
        self.database.write(|transaction| {
            SESSIONS.insert(transaction, session.terminal().as_str(), session)?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn session(&self, terminal: &TerminalName) -> Result<Option<StoredTerminalSession>> {
        Ok(self
            .database
            .read(|transaction| SESSIONS.get(transaction, terminal.as_str()))?)
    }

    pub fn sessions(&self) -> Result<Vec<StoredTerminalSession>> {
        Ok(self.database.read(|transaction| {
            Ok(SESSIONS
                .iter(transaction)?
                .into_iter()
                .map(|(_terminal, session)| session)
                .collect())
        })?)
    }
}
