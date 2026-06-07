use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use signal_terminal::{
    TerminalDeliveryAttemptObservation, TerminalDeliveryAttemptState, TerminalEventObservation,
    TerminalGeneration, TerminalName, TerminalObservationSequence, TerminalOperationKind,
    TerminalReady, TerminalReply, TerminalSessionArchiveObservation, TerminalSessionArchiveState,
    TerminalSessionHealthObservation, TerminalSessionObservation, TerminalSessionState,
    TerminalViewerAttachmentObservation, TerminalViewerAttachmentState,
};
use terminal::Error;
use terminal::registry::SessionRegistration;
use terminal::registry::SessionResolveRequest;
use terminal::tables::{StoreLocation, TerminalTables};

struct RegistryFixture {
    root: PathBuf,
    store: StoreLocation,
}

impl RegistryFixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "terminal-registry-{name}-{}-{}",
            std::process::id(),
            Self::stamp()
        ));
        fs::create_dir_all(&root).expect("registry fixture directory is created");
        let store = StoreLocation::new(root.join("terminal.sema"));
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
    let session = TerminalSessionObservation::ready(
        terminal.clone(),
        "/tmp/operator.control.sock",
        "/tmp/operator.data.sock",
    );

    tables.put_session(&session).expect("session is written");

    let stored = tables
        .session(&terminal)
        .expect("session is readable")
        .expect("session exists");
    assert_eq!(stored.terminal(), &terminal);
    assert_eq!(
        stored.control_socket_path().as_str(),
        "/tmp/operator.control.sock"
    );
    assert_eq!(
        stored.data_socket_path().as_str(),
        "/tmp/operator.data.sock"
    );
    assert_eq!(stored.state(), TerminalSessionState::Ready);
}

#[test]
fn terminal_daemon_registration_writes_named_session_with_typed_control_and_data_paths() {
    let fixture = RegistryFixture::new("daemon-registration");
    let terminal = TerminalName::new("assistant");

    SessionRegistration::ready(
        fixture.store(),
        terminal.clone(),
        "/tmp/assistant.control.sock",
        "/tmp/assistant.data.sock",
    )
    .record()
    .expect("session registration is written");

    let rows = fixture.tables().sessions().expect("sessions are readable");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].terminal(), &terminal);
    assert_eq!(
        rows[0].control_socket_path().as_str(),
        "/tmp/assistant.control.sock"
    );
    assert_eq!(
        rows[0].data_socket_path().as_str(),
        "/tmp/assistant.data.sock"
    );

    let health = fixture
        .tables()
        .session_health_records()
        .expect("session health records are readable");
    assert_eq!(health.len(), 1);
    assert_eq!(health[0].terminal(), &terminal);
    assert_eq!(health[0].state(), TerminalSessionState::Ready);
    assert_eq!(health[0].generation(), TerminalGeneration::new(1));
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
        .put_delivery_attempt(&TerminalDeliveryAttemptObservation::started(
            TerminalObservationSequence::new(1),
            terminal.clone(),
            TerminalOperationKind::TerminalConnection,
        ))
        .expect("delivery attempt is written");
    tables
        .put_terminal_event(&TerminalEventObservation::new(
            TerminalObservationSequence::new(1),
            terminal.clone(),
            TerminalReply::from(TerminalReady {
                terminal: terminal.clone(),
                generation: TerminalGeneration::new(1),
            }),
        ))
        .expect("terminal event is written");
    tables
        .put_viewer_attachment(&TerminalViewerAttachmentObservation::new(
            TerminalObservationSequence::new(1),
            terminal.clone(),
            "visible-window",
            TerminalViewerAttachmentState::Attached,
        ))
        .expect("viewer attachment is written");
    tables
        .put_session_health(&TerminalSessionHealthObservation::new(
            terminal.clone(),
            TerminalSessionState::Ready,
            TerminalGeneration::new(1),
        ))
        .expect("session health is written");
    tables
        .put_session_archive(&TerminalSessionArchiveObservation::archived(
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
    assert_eq!(attempts[0].state(), TerminalDeliveryAttemptState::Started);

    let events = tables
        .terminal_event_records()
        .expect("terminal events are readable");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].terminal(), &terminal);

    let attachments = tables
        .viewer_attachment_records()
        .expect("viewer attachments are readable");
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].viewer().as_str(), "visible-window");
    assert_eq!(
        attachments[0].state(),
        TerminalViewerAttachmentState::Attached
    );

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
    assert_eq!(archive[0].reason().as_str(), "session rotated");
    assert_eq!(archive[0].state(), TerminalSessionArchiveState::Archived);
}
