use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use persona_terminal::Error;
use persona_terminal::registry::SessionRegistration;
use persona_terminal::registry::SessionResolveRequest;
use persona_terminal::tables::{
    DeliveryAttemptState, SessionArchiveState, StoreLocation, StoredDeliveryAttempt,
    StoredSessionArchive, StoredSessionHealth, StoredTerminalEvent, StoredTerminalSession,
    StoredViewerAttachment, TerminalSessionState, TerminalTables, ViewerAttachmentState,
};
use signal_persona_terminal::{
    TerminalEvent, TerminalGeneration, TerminalName, TerminalOperationKind, TerminalReady,
};

struct RegistryFixture {
    root: PathBuf,
    store: StoreLocation,
}

impl RegistryFixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "persona-terminal-registry-{name}-{}-{}",
            std::process::id(),
            Self::stamp()
        ));
        fs::create_dir_all(&root).expect("registry fixture directory is created");
        let store = StoreLocation::new(root.join("terminal.redb"));
        Self { root, store }
    }

    fn stamp() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time is after epoch")
            .as_nanos()
    }

    fn tables(&self) -> TerminalTables {
        TerminalTables::open(&self.store).expect("terminal tables open")
    }

    fn store(&self) -> StoreLocation {
        self.store.clone()
    }
}

impl Drop for RegistryFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn terminal_sessions_are_component_sema_records() {
    let fixture = RegistryFixture::new("component-sema-records");
    let tables = fixture.tables();
    let terminal = TerminalName::new("operator");
    let session = StoredTerminalSession::ready(terminal.clone(), "/tmp/operator.sock");

    tables.put_session(&session).expect("session is written");

    let stored = tables
        .session(&terminal)
        .expect("session is readable")
        .expect("session exists");
    assert_eq!(stored.terminal(), &terminal);
    assert_eq!(stored.socket_path_text(), "/tmp/operator.sock");
    assert_eq!(stored.state(), TerminalSessionState::Ready);
}

#[test]
fn terminal_daemon_registration_writes_named_session() {
    let fixture = RegistryFixture::new("daemon-registration");
    let terminal = TerminalName::new("assistant");

    SessionRegistration::ready(fixture.store(), terminal.clone(), "/tmp/assistant.sock")
        .record()
        .expect("session registration is written");

    let rows = fixture.tables().sessions().expect("sessions are readable");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].terminal(), &terminal);
    assert_eq!(rows[0].socket_path_text(), "/tmp/assistant.sock");
}

#[test]
fn terminal_resolve_reports_missing_session() {
    let fixture = RegistryFixture::new("missing-session");
    let request = SessionResolveRequest::new(fixture.store(), TerminalName::new("missing"));

    let error = request
        .run(Vec::new())
        .expect_err("missing session is not successful");
    assert!(matches!(
        error,
        Error::UnknownTerminalSession { ref terminal } if terminal == "missing"
    ));
}

#[test]
fn terminal_tables_cover_t6_state_records() {
    let fixture = RegistryFixture::new("t6-state-records");
    let tables = fixture.tables();
    let terminal = TerminalName::new("operator");

    tables
        .put_delivery_attempt(&StoredDeliveryAttempt::started(
            1,
            terminal.clone(),
            TerminalOperationKind::TerminalConnection,
        ))
        .expect("delivery attempt is written");
    tables
        .put_terminal_event(&StoredTerminalEvent::new(
            1,
            terminal.clone(),
            TerminalEvent::from(TerminalReady {
                terminal: terminal.clone(),
                generation: TerminalGeneration::new(1),
            }),
        ))
        .expect("terminal event is written");
    tables
        .put_viewer_attachment(&StoredViewerAttachment::new(
            1,
            terminal.clone(),
            "visible-window",
            ViewerAttachmentState::Attached,
        ))
        .expect("viewer attachment is written");
    tables
        .put_session_health(&StoredSessionHealth::new(
            terminal.clone(),
            TerminalSessionState::Ready,
            TerminalGeneration::new(1),
        ))
        .expect("session health is written");
    tables
        .put_session_archive(&StoredSessionArchive::archived(
            terminal.clone(),
            "session rotated",
        ))
        .expect("session archive is written");

    let attempts = tables
        .delivery_attempt_records()
        .expect("delivery attempts are readable");
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].terminal(), &terminal);
    assert_eq!(
        attempts[0].operation(),
        TerminalOperationKind::TerminalConnection
    );
    assert_eq!(attempts[0].state(), DeliveryAttemptState::Started);

    let events = tables
        .terminal_event_records()
        .expect("terminal events are readable");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].terminal(), &terminal);

    let attachments = tables
        .viewer_attachment_records()
        .expect("viewer attachments are readable");
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].viewer(), "visible-window");
    assert_eq!(attachments[0].state(), ViewerAttachmentState::Attached);

    let health = tables
        .session_health_records()
        .expect("session health records are readable");
    assert_eq!(health.len(), 1);
    assert_eq!(health[0].state(), TerminalSessionState::Ready);

    let archive = tables
        .session_archive_records()
        .expect("session archive records are readable");
    assert_eq!(archive.len(), 1);
    assert_eq!(archive[0].terminal(), &terminal);
    assert_eq!(archive[0].reason(), "session rotated");
    assert_eq!(archive[0].state(), SessionArchiveState::Archived);
}
