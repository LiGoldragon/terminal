use std::fs;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use persona_terminal::registry::SessionRegistration;
use persona_terminal::supervisor::{TerminalSupervisorDaemon, TerminalSupervisorFrameCodec};
use persona_terminal::tables::{DeliveryAttemptState, StoreLocation, TerminalTables};
use signal_persona_terminal::{
    PromptPattern, PromptPatternBytes, PromptPatternId, PromptPatternRegistered,
    RegisterPromptPattern, TerminalEvent, TerminalName,
};

struct SupervisorFixture {
    root: PathBuf,
    store: StoreLocation,
}

impl SupervisorFixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pt-sup-{name}-{}-{}",
            std::process::id(),
            Self::stamp()
        ));
        fs::create_dir_all(&root).expect("supervisor fixture directory is created");
        let store = StoreLocation::new(root.join("terminal.redb"));
        Self { root, store }
    }

    fn stamp() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time is after epoch")
            .as_nanos()
    }

    fn store(&self) -> StoreLocation {
        self.store.clone()
    }

    fn cell_socket(&self) -> PathBuf {
        self.root.join("cell.sock")
    }

    fn supervisor_socket(&self) -> PathBuf {
        self.root.join("supervisor.sock")
    }
}

impl Drop for SupervisorFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn terminal_supervisor_socket_routes_through_component_sema() {
    let fixture = SupervisorFixture::new("routes-through-sema");
    let terminal = TerminalName::new("operator");
    SessionRegistration::ready(fixture.store(), terminal.clone(), fixture.cell_socket())
        .record()
        .expect("session registration is written");

    let cell_listener = UnixListener::bind(fixture.cell_socket()).expect("fake cell socket binds");
    let cell = thread::spawn({
        let terminal = terminal.clone();
        move || {
            let (stream, _address) = cell_listener.accept().expect("supervisor connects");
            let mut stream = std::io::BufReader::new(stream);
            let codec = TerminalSupervisorFrameCodec::default();
            let request = codec
                .read_request(&mut stream)
                .expect("supervisor writes terminal signal request");
            assert_eq!(
                request,
                RegisterPromptPattern {
                    terminal: terminal.clone(),
                    pattern: PromptPattern::LiteralSuffix(PromptPatternBytes::new(
                        b"ready> ".to_vec(),
                    )),
                }
                .into()
            );
            let stream: &mut UnixStream = stream.get_mut();
            codec
                .write_event(
                    stream,
                    TerminalEvent::from(PromptPatternRegistered {
                        terminal,
                        pattern_id: PromptPatternId::new("from-cell"),
                    }),
                )
                .expect("fake cell writes terminal signal event");
        }
    });

    let supervisor = TerminalSupervisorDaemon::from_socket(fixture.supervisor_socket())
        .with_store(fixture.store())
        .bind()
        .expect("supervisor binds before client connects");
    let supervisor_socket = supervisor.socket().clone();
    let served = thread::spawn(move || {
        supervisor
            .serve_one()
            .expect("supervisor handles one signal request")
    });

    let mut stream =
        UnixStream::connect(supervisor_socket).expect("client connects to supervisor socket");
    let codec = TerminalSupervisorFrameCodec::default();
    codec
        .write_request(
            &mut stream,
            RegisterPromptPattern {
                terminal: terminal.clone(),
                pattern: PromptPattern::LiteralSuffix(PromptPatternBytes::new(b"ready> ".to_vec())),
            }
            .into(),
        )
        .expect("client writes supervisor request");
    let event = codec
        .read_event(&mut stream)
        .expect("client reads supervisor event");

    assert_eq!(
        event,
        TerminalEvent::from(PromptPatternRegistered {
            terminal,
            pattern_id: PromptPatternId::new("from-cell"),
        })
    );
    assert_eq!(
        served.join().expect("supervisor server joins"),
        TerminalEvent::from(PromptPatternRegistered {
            terminal: TerminalName::new("operator"),
            pattern_id: PromptPatternId::new("from-cell"),
        })
    );
    let tables = TerminalTables::open(&fixture.store()).expect("terminal tables open");
    let attempts = tables
        .delivery_attempt_records()
        .expect("delivery attempts are readable");
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].terminal(), &TerminalName::new("operator"));
    assert_eq!(attempts[0].state(), DeliveryAttemptState::Started);

    let events = tables
        .terminal_event_records()
        .expect("terminal events are readable");
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].event(),
        &TerminalEvent::from(PromptPatternRegistered {
            terminal: TerminalName::new("operator"),
            pattern_id: PromptPatternId::new("from-cell"),
        })
    );
    cell.join().expect("fake cell joins");
}
