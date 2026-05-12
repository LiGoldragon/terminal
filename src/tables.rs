use std::path::{Path, PathBuf};

use sema::{Schema, SchemaVersion, Sema, Table};
use signal_persona_terminal::{
    TerminalEvent, TerminalGeneration, TerminalName, TerminalOperationKind, TerminalSequence,
};

use crate::Result;

const TERMINAL_SCHEMA: Schema = Schema {
    version: SchemaVersion::new(1),
};

const SESSIONS: Table<&'static str, StoredTerminalSession> = Table::new("sessions");
const DELIVERY_ATTEMPTS: Table<u64, StoredDeliveryAttempt> = Table::new("delivery_attempts");
const TERMINAL_EVENTS: Table<u64, StoredTerminalEvent> = Table::new("terminal_events");
const VIEWER_ATTACHMENTS: Table<u64, StoredViewerAttachment> = Table::new("viewer_attachments");
const SESSION_HEALTH: Table<&'static str, StoredSessionHealth> = Table::new("session_health");
const SESSION_ARCHIVE: Table<&'static str, StoredSessionArchive> = Table::new("session_archive");

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
            DELIVERY_ATTEMPTS.ensure(transaction)?;
            TERMINAL_EVENTS.ensure(transaction)?;
            VIEWER_ATTACHMENTS.ensure(transaction)?;
            SESSION_HEALTH.ensure(transaction)?;
            SESSION_ARCHIVE.ensure(transaction)?;
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

    pub fn put_delivery_attempt(&self, attempt: &StoredDeliveryAttempt) -> Result<()> {
        self.database.write(|transaction| {
            DELIVERY_ATTEMPTS.insert(transaction, attempt.sequence(), attempt)?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn delivery_attempt_records(&self) -> Result<Vec<StoredDeliveryAttempt>> {
        Ok(self.database.read(|transaction| {
            Ok(DELIVERY_ATTEMPTS
                .iter(transaction)?
                .into_iter()
                .map(|(_sequence, attempt)| attempt)
                .collect())
        })?)
    }

    pub fn put_terminal_event(&self, event: &StoredTerminalEvent) -> Result<()> {
        self.database.write(|transaction| {
            TERMINAL_EVENTS.insert(transaction, event.sequence(), event)?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn terminal_event_records(&self) -> Result<Vec<StoredTerminalEvent>> {
        Ok(self.database.read(|transaction| {
            Ok(TERMINAL_EVENTS
                .iter(transaction)?
                .into_iter()
                .map(|(_sequence, event)| event)
                .collect())
        })?)
    }

    pub fn put_viewer_attachment(&self, attachment: &StoredViewerAttachment) -> Result<()> {
        self.database.write(|transaction| {
            VIEWER_ATTACHMENTS.insert(transaction, attachment.sequence(), attachment)?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn viewer_attachment_records(&self) -> Result<Vec<StoredViewerAttachment>> {
        Ok(self.database.read(|transaction| {
            Ok(VIEWER_ATTACHMENTS
                .iter(transaction)?
                .into_iter()
                .map(|(_sequence, attachment)| attachment)
                .collect())
        })?)
    }

    pub fn put_session_health(&self, health: &StoredSessionHealth) -> Result<()> {
        self.database.write(|transaction| {
            SESSION_HEALTH.insert(transaction, health.terminal().as_str(), health)?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn session_health_records(&self) -> Result<Vec<StoredSessionHealth>> {
        Ok(self.database.read(|transaction| {
            Ok(SESSION_HEALTH
                .iter(transaction)?
                .into_iter()
                .map(|(_terminal, health)| health)
                .collect())
        })?)
    }

    pub fn put_session_archive(&self, archive: &StoredSessionArchive) -> Result<()> {
        self.database.write(|transaction| {
            SESSION_ARCHIVE.insert(transaction, archive.terminal().as_str(), archive)?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn session_archive_records(&self) -> Result<Vec<StoredSessionArchive>> {
        Ok(self.database.read(|transaction| {
            Ok(SESSION_ARCHIVE
                .iter(transaction)?
                .into_iter()
                .map(|(_terminal, archive)| archive)
                .collect())
        })?)
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct StoredDeliveryAttempt {
    sequence: u64,
    terminal: TerminalName,
    operation: TerminalOperationKind,
    state: DeliveryAttemptState,
}

impl StoredDeliveryAttempt {
    pub fn started(
        sequence: u64,
        terminal: TerminalName,
        operation: TerminalOperationKind,
    ) -> Self {
        Self {
            sequence,
            terminal,
            operation,
            state: DeliveryAttemptState::Started,
        }
    }

    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn terminal(&self) -> &TerminalName {
        &self.terminal
    }

    pub const fn operation(&self) -> TerminalOperationKind {
        self.operation
    }

    pub const fn state(&self) -> DeliveryAttemptState {
        self.state
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryAttemptState {
    Started,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct StoredTerminalEvent {
    sequence: u64,
    terminal: TerminalName,
    event: TerminalEvent,
}

impl StoredTerminalEvent {
    pub fn new(sequence: u64, terminal: TerminalName, event: TerminalEvent) -> Self {
        Self {
            sequence,
            terminal,
            event,
        }
    }

    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn terminal(&self) -> &TerminalName {
        &self.terminal
    }

    pub fn event(&self) -> &TerminalEvent {
        &self.event
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct StoredViewerAttachment {
    sequence: u64,
    terminal: TerminalName,
    viewer: String,
    state: ViewerAttachmentState,
}

impl StoredViewerAttachment {
    pub fn new(
        sequence: u64,
        terminal: TerminalName,
        viewer: impl Into<String>,
        state: ViewerAttachmentState,
    ) -> Self {
        Self {
            sequence,
            terminal,
            viewer: viewer.into(),
            state,
        }
    }

    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn terminal(&self) -> &TerminalName {
        &self.terminal
    }

    pub fn viewer(&self) -> &str {
        self.viewer.as_str()
    }

    pub const fn state(&self) -> ViewerAttachmentState {
        self.state
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerAttachmentState {
    Attached,
    Detached,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct StoredSessionHealth {
    terminal: TerminalName,
    state: TerminalSessionState,
    generation: TerminalGeneration,
}

impl StoredSessionHealth {
    pub fn new(
        terminal: TerminalName,
        state: TerminalSessionState,
        generation: TerminalGeneration,
    ) -> Self {
        Self {
            terminal,
            state,
            generation,
        }
    }

    pub fn terminal(&self) -> &TerminalName {
        &self.terminal
    }

    pub const fn state(&self) -> TerminalSessionState {
        self.state
    }

    pub const fn generation(&self) -> TerminalGeneration {
        self.generation
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct StoredSessionArchive {
    terminal: TerminalName,
    reason: String,
    state: SessionArchiveState,
}

impl StoredSessionArchive {
    pub fn archived(terminal: TerminalName, reason: impl Into<String>) -> Self {
        Self {
            terminal,
            reason: reason.into(),
            state: SessionArchiveState::Archived,
        }
    }

    pub fn terminal(&self) -> &TerminalName {
        &self.terminal
    }

    pub fn reason(&self) -> &str {
        self.reason.as_str()
    }

    pub const fn state(&self) -> SessionArchiveState {
        self.state
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionArchiveState {
    Archived,
}
