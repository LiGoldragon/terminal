use std::path::{Path, PathBuf};

use rkyv::Deserialize as RkyvDeserialize;
use rkyv::api::high::HighDeserializer;
use rkyv::bytecheck::CheckBytes;
use rkyv::rancor::{self, Strategy};
use rkyv::validation::Validator;
use rkyv::validation::archive::ArchiveValidator;
use rkyv::validation::shared::SharedValidator;
use sema_engine::{
    Engine, EngineOpen, EngineStoredValue, KeyedAssertion, KeyedMutation, QueryPlan, RecordKey,
    SchemaVersion, TableDescriptor, TableName, TableReference,
};
use signal_terminal::{
    TerminalDeliveryAttemptObservation, TerminalEventObservation, TerminalName,
    TerminalSessionArchiveObservation, TerminalSessionHealthObservation,
    TerminalSessionObservation, TerminalViewerAttachmentObservation,
};

use crate::Result;

const TERMINAL_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(1);
const SESSIONS: TableName = TableName::new("sessions");
const DELIVERY_ATTEMPTS: TableName = TableName::new("delivery_attempts");
const TERMINAL_EVENTS: TableName = TableName::new("terminal_events");
const VIEWER_ATTACHMENTS: TableName = TableName::new("viewer_attachments");
const SESSION_HEALTH: TableName = TableName::new("session_health");
const SESSION_ARCHIVE: TableName = TableName::new("session_archive");

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
                None => Self::new("/tmp/terminal.sema"),
            },
        }
    }

    pub fn as_path(&self) -> &Path {
        self.path.as_path()
    }
}

pub struct TerminalTables {
    engine: Engine,
    sessions: TableReference<TerminalSessionObservation>,
    delivery_attempts: TableReference<TerminalDeliveryAttemptObservation>,
    terminal_events: TableReference<TerminalEventObservation>,
    viewer_attachments: TableReference<TerminalViewerAttachmentObservation>,
    session_health: TableReference<TerminalSessionHealthObservation>,
    session_archive: TableReference<TerminalSessionArchiveObservation>,
}

impl TerminalTables {
    pub fn open(store: &StoreLocation) -> Result<Self> {
        let mut engine = Engine::open(EngineOpen::new(
            store.as_path().to_path_buf(),
            TERMINAL_SCHEMA_VERSION,
        ))?;
        let sessions = engine.register_table(TableDescriptor::new(SESSIONS))?;
        let delivery_attempts = engine.register_table(TableDescriptor::new(DELIVERY_ATTEMPTS))?;
        let terminal_events = engine.register_table(TableDescriptor::new(TERMINAL_EVENTS))?;
        let viewer_attachments = engine.register_table(TableDescriptor::new(VIEWER_ATTACHMENTS))?;
        let session_health = engine.register_table(TableDescriptor::new(SESSION_HEALTH))?;
        let session_archive = engine.register_table(TableDescriptor::new(SESSION_ARCHIVE))?;
        Ok(Self {
            engine,
            sessions,
            delivery_attempts,
            terminal_events,
            viewer_attachments,
            session_health,
            session_archive,
        })
    }

    pub fn put_session(&self, session: &TerminalSessionObservation) -> Result<()> {
        self.put_record(
            self.sessions,
            RecordKey::new(session.terminal().as_str()),
            session,
        )
    }

    pub fn session(&self, terminal: &TerminalName) -> Result<Option<TerminalSessionObservation>> {
        Ok(self
            .engine
            .match_records(QueryPlan::key(
                self.sessions,
                RecordKey::new(terminal.as_str()),
            ))?
            .records()
            .first()
            .cloned())
    }

    pub fn sessions(&self) -> Result<Vec<TerminalSessionObservation>> {
        self.records(self.sessions)
    }

    pub fn put_delivery_attempt(&self, attempt: &TerminalDeliveryAttemptObservation) -> Result<()> {
        self.put_record(
            self.delivery_attempts,
            RecordKey::new(attempt.sequence().into_u64().to_string()),
            attempt,
        )
    }

    pub fn delivery_attempt_records(&self) -> Result<Vec<TerminalDeliveryAttemptObservation>> {
        self.records(self.delivery_attempts)
    }

    pub fn put_terminal_event(&self, event: &TerminalEventObservation) -> Result<()> {
        self.put_record(
            self.terminal_events,
            RecordKey::new(event.sequence().into_u64().to_string()),
            event,
        )
    }

    pub fn terminal_event_records(&self) -> Result<Vec<TerminalEventObservation>> {
        self.records(self.terminal_events)
    }

    pub fn put_viewer_attachment(
        &self,
        attachment: &TerminalViewerAttachmentObservation,
    ) -> Result<()> {
        self.put_record(
            self.viewer_attachments,
            RecordKey::new(attachment.sequence().into_u64().to_string()),
            attachment,
        )
    }

    pub fn viewer_attachment_records(&self) -> Result<Vec<TerminalViewerAttachmentObservation>> {
        self.records(self.viewer_attachments)
    }

    pub fn put_session_health(&self, health: &TerminalSessionHealthObservation) -> Result<()> {
        self.put_record(
            self.session_health,
            RecordKey::new(health.terminal().as_str()),
            health,
        )
    }

    pub fn session_health_records(&self) -> Result<Vec<TerminalSessionHealthObservation>> {
        self.records(self.session_health)
    }

    pub fn put_session_archive(&self, archive: &TerminalSessionArchiveObservation) -> Result<()> {
        self.put_record(
            self.session_archive,
            RecordKey::new(archive.terminal().as_str()),
            archive,
        )
    }

    pub fn session_archive_records(&self) -> Result<Vec<TerminalSessionArchiveObservation>> {
        self.records(self.session_archive)
    }

    pub fn registered_table_names(&self) -> Vec<String> {
        self.engine
            .list_tables()
            .into_iter()
            .map(|registration| registration.table_name().to_owned())
            .collect()
    }

    fn records<RecordValue>(&self, table: TableReference<RecordValue>) -> Result<Vec<RecordValue>>
    where
        RecordValue: TerminalEngineRecord,
        RecordValue::Archived: RkyvDeserialize<RecordValue, HighDeserializer<rancor::Error>>
            + for<'validation> CheckBytes<
                Strategy<Validator<ArchiveValidator<'validation>, SharedValidator>, rancor::Error>,
            >,
    {
        Ok(self
            .engine
            .match_records(QueryPlan::all(table))?
            .records()
            .to_vec())
    }

    fn put_record<RecordValue>(
        &self,
        table: TableReference<RecordValue>,
        key: RecordKey,
        record: &RecordValue,
    ) -> Result<()>
    where
        RecordValue: TerminalEngineRecord,
        RecordValue::Archived: RkyvDeserialize<RecordValue, HighDeserializer<rancor::Error>>
            + for<'validation> CheckBytes<
                Strategy<Validator<ArchiveValidator<'validation>, SharedValidator>, rancor::Error>,
            >,
    {
        let exists = !self
            .engine
            .match_records(QueryPlan::key(table, key.clone()))?
            .records()
            .is_empty();
        if exists {
            self.engine
                .mutate_keyed(KeyedMutation::new(table, key, record.clone()))?;
        } else {
            self.engine
                .assert_keyed(KeyedAssertion::new(table, key, record.clone()))?;
        }
        Ok(())
    }
}

trait TerminalEngineRecord: EngineStoredValue + Send + Sync + 'static
where
    Self::Archived: RkyvDeserialize<Self, HighDeserializer<rancor::Error>>
        + for<'validation> CheckBytes<
            Strategy<Validator<ArchiveValidator<'validation>, SharedValidator>, rancor::Error>,
        >,
{
}

impl<RecordValue> TerminalEngineRecord for RecordValue
where
    RecordValue: EngineStoredValue + Send + Sync + 'static,
    RecordValue::Archived: RkyvDeserialize<RecordValue, HighDeserializer<rancor::Error>>
        + for<'validation> CheckBytes<
            Strategy<Validator<ArchiveValidator<'validation>, SharedValidator>, rancor::Error>,
        >,
{
}
