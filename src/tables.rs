use std::path::{Path, PathBuf};

use sema::{Schema, SchemaVersion, Sema, Table};
use signal_terminal::{
    TerminalDeliveryAttemptObservation, TerminalEventObservation, TerminalName,
    TerminalSessionArchiveObservation, TerminalSessionHealthObservation,
    TerminalSessionObservation, TerminalViewerAttachmentObservation,
};

use crate::Result;

const TERMINAL_SCHEMA: Schema = Schema {
    version: SchemaVersion::new(1),
};

const SESSIONS: Table<&'static str, TerminalSessionObservation> = Table::new("sessions");
const DELIVERY_ATTEMPTS: Table<u64, TerminalDeliveryAttemptObservation> =
    Table::new("delivery_attempts");
const TERMINAL_EVENTS: Table<u64, TerminalEventObservation> = Table::new("terminal_events");
const VIEWER_ATTACHMENTS: Table<u64, TerminalViewerAttachmentObservation> =
    Table::new("viewer_attachments");
const SESSION_HEALTH: Table<&'static str, TerminalSessionHealthObservation> =
    Table::new("session_health");
const SESSION_ARCHIVE: Table<&'static str, TerminalSessionArchiveObservation> =
    Table::new("session_archive");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreLocation {
    path: PathBuf,
}

impl StoreLocation {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn from_environment() -> Self {
        match std::env::var_os("TERMINAL_STORE") {
            Some(path) => Self::new(path),
            None => match std::env::var_os("PERSONA_STATE_PATH") {
                Some(path) => Self::new(path),
                None => Self::new("/tmp/terminal.redb"),
            },
        }
    }

    pub fn as_path(&self) -> &Path {
        self.path.as_path()
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

    pub fn put_session(&self, session: &TerminalSessionObservation) -> Result<()> {
        self.database.write(|transaction| {
            SESSIONS.insert(transaction, session.terminal().as_str(), session)?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn session(&self, terminal: &TerminalName) -> Result<Option<TerminalSessionObservation>> {
        Ok(self
            .database
            .read(|transaction| SESSIONS.get(transaction, terminal.as_str()))?)
    }

    pub fn sessions(&self) -> Result<Vec<TerminalSessionObservation>> {
        Ok(self.database.read(|transaction| {
            Ok(SESSIONS
                .iter(transaction)?
                .into_iter()
                .map(|(_terminal, session)| session)
                .collect())
        })?)
    }

    pub fn put_delivery_attempt(&self, attempt: &TerminalDeliveryAttemptObservation) -> Result<()> {
        self.database.write(|transaction| {
            DELIVERY_ATTEMPTS.insert(transaction, attempt.sequence().into_u64(), attempt)?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn delivery_attempt_records(&self) -> Result<Vec<TerminalDeliveryAttemptObservation>> {
        Ok(self.database.read(|transaction| {
            Ok(DELIVERY_ATTEMPTS
                .iter(transaction)?
                .into_iter()
                .map(|(_sequence, attempt)| attempt)
                .collect())
        })?)
    }

    pub fn put_terminal_event(&self, event: &TerminalEventObservation) -> Result<()> {
        self.database.write(|transaction| {
            TERMINAL_EVENTS.insert(transaction, event.sequence().into_u64(), event)?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn terminal_event_records(&self) -> Result<Vec<TerminalEventObservation>> {
        Ok(self.database.read(|transaction| {
            Ok(TERMINAL_EVENTS
                .iter(transaction)?
                .into_iter()
                .map(|(_sequence, event)| event)
                .collect())
        })?)
    }

    pub fn put_viewer_attachment(
        &self,
        attachment: &TerminalViewerAttachmentObservation,
    ) -> Result<()> {
        self.database.write(|transaction| {
            VIEWER_ATTACHMENTS.insert(transaction, attachment.sequence().into_u64(), attachment)?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn viewer_attachment_records(&self) -> Result<Vec<TerminalViewerAttachmentObservation>> {
        Ok(self.database.read(|transaction| {
            Ok(VIEWER_ATTACHMENTS
                .iter(transaction)?
                .into_iter()
                .map(|(_sequence, attachment)| attachment)
                .collect())
        })?)
    }

    pub fn put_session_health(&self, health: &TerminalSessionHealthObservation) -> Result<()> {
        self.database.write(|transaction| {
            SESSION_HEALTH.insert(transaction, health.terminal().as_str(), health)?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn session_health_records(&self) -> Result<Vec<TerminalSessionHealthObservation>> {
        Ok(self.database.read(|transaction| {
            Ok(SESSION_HEALTH
                .iter(transaction)?
                .into_iter()
                .map(|(_terminal, health)| health)
                .collect())
        })?)
    }

    pub fn put_session_archive(&self, archive: &TerminalSessionArchiveObservation) -> Result<()> {
        self.database.write(|transaction| {
            SESSION_ARCHIVE.insert(transaction, archive.terminal().as_str(), archive)?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn session_archive_records(&self) -> Result<Vec<TerminalSessionArchiveObservation>> {
        Ok(self.database.read(|transaction| {
            Ok(SESSION_ARCHIVE
                .iter(transaction)?
                .into_iter()
                .map(|(_terminal, archive)| archive)
                .collect())
        })?)
    }
}
