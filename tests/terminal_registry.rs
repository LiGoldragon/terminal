use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use persona_terminal::Error;
use persona_terminal::registry::SessionRegistration;
use persona_terminal::registry::SessionResolveRequest;
use persona_terminal::tables::{
    StoreLocation, StoredTerminalSession, TerminalSessionState, TerminalTables,
};
use signal_persona_terminal::TerminalName;

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
