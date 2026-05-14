use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use persona_terminal::SocketMode;
use persona_terminal::registry::SessionRegistration;
use persona_terminal::supervisor::{
    TerminalSupervisorCommandLine, TerminalSupervisorDaemon, TerminalSupervisorFrameCodec,
};
use persona_terminal::tables::{StoreLocation, TerminalTables};
use signal_persona_terminal::{
    PromptPattern, PromptPatternBytes, PromptPatternId, PromptPatternRegistered,
    RegisterPromptPattern, SubscribeTerminalWorkerLifecycle, TerminalDeliveryAttemptState,
    TerminalEvent, TerminalName, TerminalWorkerKind, TerminalWorkerLifecycle,
    TerminalWorkerLifecycleEvent, TerminalWorkerLifecycleSnapshot, TerminalWorkerStopReason,
};

static ENVIRONMENT_LOCK: Mutex<()> = Mutex::new(());

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

struct EnvironmentRestore {
    name: &'static str,
    value: Option<std::ffi::OsString>,
}

impl EnvironmentRestore {
    fn capture(name: &'static str) -> Self {
        Self {
            name,
            value: std::env::var_os(name),
        }
    }

    fn set(&self, value: impl AsRef<std::ffi::OsStr>) {
        unsafe {
            std::env::set_var(self.name, value);
        }
    }

    fn remove(&self) {
        unsafe {
            std::env::remove_var(self.name);
        }
    }
}

impl Drop for EnvironmentRestore {
    fn drop(&mut self) {
        unsafe {
            match &self.value {
                Some(value) => std::env::set_var(self.name, value),
                None => std::env::remove_var(self.name),
            }
        }
    }
}

#[test]
fn terminal_supervisor_daemon_applies_spawn_envelope_socket_mode() {
    let fixture = SupervisorFixture::new("socket-mode");
    let supervisor = TerminalSupervisorDaemon::from_socket(fixture.supervisor_socket())
        .with_store(fixture.store())
        .with_socket_mode(SocketMode::from_octal(0o600))
        .bind()
        .expect("supervisor binds before client connects");

    let mode = fs::metadata(supervisor.socket())
        .expect("supervisor socket metadata is readable")
        .permissions()
        .mode()
        & 0o777;

    assert_eq!(mode, 0o600);
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
    assert_eq!(attempts[0].state(), TerminalDeliveryAttemptState::Started);

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

#[test]
fn terminal_supervisor_command_line_uses_spawn_envelope_environment() {
    let _lock = ENVIRONMENT_LOCK
        .lock()
        .expect("environment lock is available");
    let fixture = SupervisorFixture::new("spawn-envelope-environment");
    let socket = fixture.root.join("run").join("terminal.sock");
    let state = fixture.root.join("state").join("terminal.redb");
    let terminal_store = EnvironmentRestore::capture("PERSONA_TERMINAL_STORE");
    let state_path = EnvironmentRestore::capture("PERSONA_STATE_PATH");
    let socket_path = EnvironmentRestore::capture("PERSONA_SOCKET_PATH");

    terminal_store.remove();
    state_path.set(&state);
    socket_path.set(&socket);

    let daemon = TerminalSupervisorCommandLine::from_arguments(Vec::<String>::new())
        .daemon()
        .expect("supervisor daemon resolves from spawn envelope environment");
    assert_eq!(daemon.socket(), &socket);
    assert_eq!(daemon.store().as_path(), state.as_path());
}

#[test]
fn terminal_supervisor_subscription_streams_initial_state_then_delta() {
    let fixture = SupervisorFixture::new("streams-lifecycle");
    let terminal = TerminalName::new("responder");
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
                .expect("supervisor writes subscription request");
            assert_eq!(
                request,
                SubscribeTerminalWorkerLifecycle {
                    terminal: terminal.clone(),
                }
                .into()
            );
            let stream: &mut UnixStream = stream.get_mut();
            codec
                .write_event(
                    stream,
                    TerminalEvent::from(TerminalWorkerLifecycleSnapshot {
                        terminal: terminal.clone(),
                        observations: vec![TerminalWorkerLifecycle::Started(
                            TerminalWorkerKind::OutputReader,
                        )],
                    }),
                )
                .expect("fake cell writes lifecycle snapshot");
            codec
                .write_event(
                    stream,
                    TerminalEvent::from(TerminalWorkerLifecycleEvent {
                        terminal,
                        observation: TerminalWorkerLifecycle::Stopped {
                            worker: TerminalWorkerKind::OutputReader,
                            reason: TerminalWorkerStopReason::OutputReaderFinished,
                        },
                    }),
                )
                .expect("fake cell writes lifecycle delta");
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
            .expect("supervisor handles subscription")
    });

    let mut stream =
        UnixStream::connect(supervisor_socket).expect("client connects to supervisor socket");
    let codec = TerminalSupervisorFrameCodec::default();
    codec
        .write_request(
            &mut stream,
            SubscribeTerminalWorkerLifecycle {
                terminal: terminal.clone(),
            }
            .into(),
        )
        .expect("client writes subscription request");
    let snapshot = codec
        .read_event(&mut stream)
        .expect("client reads initial lifecycle state");
    let delta = codec
        .read_event(&mut stream)
        .expect("client reads lifecycle delta");

    assert_eq!(
        snapshot,
        TerminalEvent::from(TerminalWorkerLifecycleSnapshot {
            terminal: TerminalName::new("responder"),
            observations: vec![TerminalWorkerLifecycle::Started(
                TerminalWorkerKind::OutputReader,
            )],
        })
    );
    assert_eq!(
        delta,
        TerminalEvent::from(TerminalWorkerLifecycleEvent {
            terminal: TerminalName::new("responder"),
            observation: TerminalWorkerLifecycle::Stopped {
                worker: TerminalWorkerKind::OutputReader,
                reason: TerminalWorkerStopReason::OutputReaderFinished,
            },
        })
    );
    assert_eq!(served.join().expect("supervisor server joins"), snapshot);

    let tables = TerminalTables::open(&fixture.store()).expect("terminal tables open");
    let attempts = tables
        .delivery_attempt_records()
        .expect("delivery attempts are readable");
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        attempts[0].operation(),
        signal_persona_terminal::TerminalOperationKind::SubscribeTerminalWorkerLifecycle
    );

    let events = tables
        .terminal_event_records()
        .expect("terminal events are readable");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event(), &snapshot);
    assert_eq!(events[1].event(), &delta);
    cell.join().expect("fake cell joins");
}
