use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use persona_terminal::registry::SessionRegistration;
use persona_terminal::supervisor::{
    TerminalSupervisorCommandLine, TerminalSupervisorDaemon, TerminalSupervisorFrameCodec,
};
use persona_terminal::tables::{StoreLocation, TerminalTables};
use persona_terminal::{SocketMode, SupervisionFrameCodec};
use signal_core::{
    ExchangeIdentifier, ExchangeLane, LaneSequence, NonEmpty, Operation, Request,
    RequestRejectionReason, SessionEpoch, SignalVerb,
};
use signal_persona::{
    ComponentHealth, ComponentHealthQuery, ComponentHello, ComponentKind, ComponentName,
    ComponentReadinessQuery, SupervisionFrame, SupervisionFrameBody, SupervisionProtocolVersion,
    SupervisionReply, SupervisionRequest, WirePath,
};
use signal_persona_terminal::{
    ListSessions, PromptPattern, PromptPatternBytes, PromptPatternId, PromptPatternRegistered,
    RegisterPromptPattern, ResolveSession, SessionEntry, SessionList, SessionResolved,
    SubscribeTerminalWorkerLifecycle, TerminalDeliveryAttemptState, TerminalEvent, TerminalFrame,
    TerminalFrameBody as FrameBody, TerminalName, TerminalReply, TerminalWorkerKind,
    TerminalWorkerLifecycle, TerminalWorkerLifecycleEvent, TerminalWorkerLifecycleSnapshot,
    TerminalWorkerStopReason,
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
        self.root.join("cell.control.sock")
    }

    fn cell_data_socket(&self) -> PathBuf {
        self.root.join("cell.data.sock")
    }

    fn supervisor_socket(&self) -> PathBuf {
        self.root.join("supervisor.sock")
    }

    fn supervision_socket(&self) -> PathBuf {
        self.root.join("supervision.sock")
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
fn terminal_supervisor_frame_codec_rejects_mismatched_signal_verb() {
    let request = Request::from_operations(NonEmpty::single(Operation::new(
        SignalVerb::Match,
        RegisterPromptPattern {
            terminal: TerminalName::new("operator"),
            pattern: PromptPattern::LiteralSuffix(PromptPatternBytes::new(b"ready> ".to_vec())),
        }
        .into(),
    )));
    let frame = TerminalFrame::new(FrameBody::Request {
        exchange: test_exchange(),
        request,
    });
    let bytes = frame.encode_length_prefixed().expect("frame encodes");
    let mut input = bytes.as_slice();
    let error = TerminalSupervisorFrameCodec::default()
        .read_request(&mut input)
        .expect_err("mismatched verb is rejected");

    match error {
        persona_terminal::Error::InvalidSignalRequest { reason } => {
            assert_eq!(
                reason,
                RequestRejectionReason::VerbPayloadMismatch { index: 0 }
            );
        }
        other => panic!("expected typed signal request rejection, got {other:?}"),
    }
}

#[test]
fn terminal_supervisor_socket_routes_through_component_sema() {
    let fixture = SupervisorFixture::new("routes-through-sema");
    let terminal = TerminalName::new("operator");
    SessionRegistration::ready(
        fixture.store(),
        terminal.clone(),
        fixture.cell_socket(),
        fixture.cell_data_socket(),
    )
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
                    TerminalReply::from(PromptPatternRegistered {
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
        TerminalReply::from(PromptPatternRegistered {
            terminal,
            pattern_id: PromptPatternId::new("from-cell"),
        })
    );
    assert_eq!(
        served.join().expect("supervisor server joins"),
        TerminalReply::from(PromptPatternRegistered {
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
        &TerminalReply::from(PromptPatternRegistered {
            terminal: TerminalName::new("operator"),
            pattern_id: PromptPatternId::new("from-cell"),
        })
    );
    cell.join().expect("fake cell joins");
}

#[test]
fn terminal_supervisor_resolves_session_without_contacting_cell() {
    let fixture = SupervisorFixture::new("resolve-session");
    let terminal = TerminalName::new("operator");
    SessionRegistration::ready(
        fixture.store(),
        terminal.clone(),
        fixture.cell_socket(),
        fixture.cell_data_socket(),
    )
    .record()
    .expect("session registration is written");

    let supervisor = TerminalSupervisorDaemon::from_socket(fixture.supervisor_socket())
        .with_store(fixture.store())
        .bind()
        .expect("supervisor binds before client connects");
    let supervisor_socket = supervisor.socket().clone();
    let served = thread::spawn(move || {
        supervisor
            .serve_one()
            .expect("supervisor handles resolve request")
    });

    let mut stream =
        UnixStream::connect(supervisor_socket).expect("client connects to supervisor socket");
    let codec = TerminalSupervisorFrameCodec::default();
    codec
        .write_request(
            &mut stream,
            ResolveSession {
                name: terminal.clone(),
            }
            .into(),
        )
        .expect("client writes supervisor request");
    let event = codec
        .read_event(&mut stream)
        .expect("client reads supervisor event");
    let expected = TerminalReply::from(SessionResolved {
        name: terminal,
        data_socket_path: WirePath::new(fixture.cell_data_socket().display().to_string()),
    });

    assert_eq!(event, expected);
    assert_eq!(served.join().expect("supervisor server joins"), expected);
}

#[test]
fn terminal_supervisor_lists_sessions_without_contacting_cells() {
    let fixture = SupervisorFixture::new("list-sessions");
    let operator = TerminalName::new("operator");
    let designer = TerminalName::new("designer");
    let operator_data_socket = fixture.root.join("operator.data.sock");
    let designer_data_socket = fixture.root.join("designer.data.sock");
    SessionRegistration::ready(
        fixture.store(),
        operator.clone(),
        fixture.root.join("operator.control.sock"),
        operator_data_socket.clone(),
    )
    .record()
    .expect("operator session registration is written");
    SessionRegistration::ready(
        fixture.store(),
        designer.clone(),
        fixture.root.join("designer.control.sock"),
        designer_data_socket.clone(),
    )
    .record()
    .expect("designer session registration is written");

    let supervisor = TerminalSupervisorDaemon::from_socket(fixture.supervisor_socket())
        .with_store(fixture.store())
        .bind()
        .expect("supervisor binds before client connects");
    let supervisor_socket = supervisor.socket().clone();
    let served = thread::spawn(move || {
        supervisor
            .serve_one()
            .expect("supervisor handles list request")
    });

    let mut stream =
        UnixStream::connect(supervisor_socket).expect("client connects to supervisor socket");
    let codec = TerminalSupervisorFrameCodec::default();
    codec
        .write_request(&mut stream, ListSessions {}.into())
        .expect("client writes supervisor request");
    let event = codec
        .read_event(&mut stream)
        .expect("client reads supervisor event");
    let TerminalReply::SessionList(SessionList { mut entries }) = event.clone() else {
        panic!("expected session list reply, got {event:?}");
    };
    entries.sort_by(|left, right| left.name.as_str().cmp(right.name.as_str()));
    let expected_entries = vec![
        SessionEntry {
            name: designer,
            data_socket_path: WirePath::new(designer_data_socket.display().to_string()),
        },
        SessionEntry {
            name: operator,
            data_socket_path: WirePath::new(operator_data_socket.display().to_string()),
        },
    ];

    assert_eq!(entries, expected_entries);
    assert_eq!(served.join().expect("supervisor server joins"), event);
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
fn terminal_supervisor_answers_component_supervision_relation() {
    use nota_codec::{Encoder, NotaEncode};
    use signal_persona::{SocketMode as WireSocketMode, WirePath};
    use signal_persona_auth::{OwnerIdentity, UnixUserId};
    use signal_persona_terminal::TerminalDaemonConfiguration;

    let fixture = SupervisorFixture::new("supervision");
    let supervision_socket = fixture.supervision_socket();
    let configuration_path = fixture.root.join("terminal-daemon.nota");
    let configuration = TerminalDaemonConfiguration {
        terminal_socket_path: WirePath::new(fixture.supervisor_socket().display().to_string()),
        terminal_socket_mode: WireSocketMode::new(0o600),
        supervision_socket_path: WirePath::new(supervision_socket.display().to_string()),
        supervision_socket_mode: WireSocketMode::new(0o600),
        store_path: WirePath::new(fixture.store().as_path().display().to_string()),
        owner_identity: OwnerIdentity::UnixUser(UnixUserId::new(1000)),
    };
    let mut encoder = Encoder::new();
    configuration
        .encode(&mut encoder)
        .expect("encode terminal config");
    let mut text = encoder.into_string();
    text.push('\n');
    fs::write(&configuration_path, text).expect("write terminal config");

    let mut child = Command::new(env!("CARGO_BIN_EXE_persona-terminal-supervisor"))
        .arg(&configuration_path)
        .spawn()
        .expect("persona-terminal-supervisor starts");

    wait_for_socket(&supervision_socket);
    let mode = fs::metadata(&supervision_socket)
        .expect("supervision socket metadata is readable")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);

    // The primary supervisor socket — the engine-facing one that
    // resolves named terminals from Sema and forwards Signal control
    // frames to terminal-cell — also honors PERSONA_SOCKET_MODE when
    // the binary is spawned in the engine envelope. Per /189 §9, the
    // ARCH constraint "Engine-spawned terminal sockets apply the
    // managed PERSONA_SOCKET_MODE before accepting client traffic"
    // needs a binary-spawn witness, not only a library-level one.
    wait_for_socket(&fixture.supervisor_socket());
    let supervisor_mode = fs::metadata(fixture.supervisor_socket())
        .expect("supervisor socket metadata is readable")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        supervisor_mode, 0o600,
        "spawned persona-terminal-supervisor applies PERSONA_SOCKET_MODE to its primary socket"
    );

    let mut stream = UnixStream::connect(&supervision_socket).expect("client connects");
    let codec = SupervisionFrameCodec::new(1024 * 1024);

    write_supervision_request(
        &mut stream,
        SupervisionRequest::ComponentHello(ComponentHello {
            expected_component: ComponentName::new("persona-terminal"),
            expected_kind: ComponentKind::Terminal,
            supervision_protocol_version: SupervisionProtocolVersion::new(1),
        }),
    );
    assert!(matches!(
        codec.read_reply(&mut stream).expect("identity reply"),
        SupervisionReply::ComponentIdentity(identity)
            if identity.name.as_str() == "persona-terminal"
                && identity.kind == ComponentKind::Terminal
    ));

    write_supervision_request(
        &mut stream,
        SupervisionRequest::ComponentReadinessQuery(ComponentReadinessQuery {
            component: ComponentName::new("persona-terminal"),
        }),
    );
    assert!(matches!(
        codec.read_reply(&mut stream).expect("readiness reply"),
        SupervisionReply::ComponentReady(_)
    ));

    write_supervision_request(
        &mut stream,
        SupervisionRequest::ComponentHealthQuery(ComponentHealthQuery {
            component: ComponentName::new("persona-terminal"),
        }),
    );
    assert!(matches!(
        codec.read_reply(&mut stream).expect("health reply"),
        SupervisionReply::ComponentHealthReport(report)
            if report.health == ComponentHealth::Running
    ));

    stop_child(&mut child);
}

#[test]
fn terminal_supervisor_subscription_streams_initial_state_then_delta() {
    let fixture = SupervisorFixture::new("streams-lifecycle");
    let terminal = TerminalName::new("responder");
    SessionRegistration::ready(
        fixture.store(),
        terminal.clone(),
        fixture.cell_socket(),
        fixture.cell_data_socket(),
    )
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
                    TerminalReply::from(TerminalWorkerLifecycleSnapshot {
                        terminal: terminal.clone(),
                        observations: vec![TerminalWorkerLifecycle::Started(
                            TerminalWorkerKind::OutputReader,
                        )],
                    }),
                )
                .expect("fake cell writes lifecycle snapshot");
            codec
                .write_stream_event(
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
        .read_stream_event(&mut stream)
        .expect("client reads lifecycle delta");

    assert_eq!(
        snapshot,
        TerminalReply::from(TerminalWorkerLifecycleSnapshot {
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
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event(), &snapshot);
    cell.join().expect("fake cell joins");
}

fn write_supervision_request(stream: &mut UnixStream, request: SupervisionRequest) {
    let frame = SupervisionFrame::new(SupervisionFrameBody::Request {
        exchange: test_exchange(),
        request: Request::from_payload(request),
    });
    let bytes = frame
        .encode_length_prefixed()
        .expect("supervision request encodes");
    stream
        .write_all(bytes.as_slice())
        .expect("supervision request writes");
    stream.flush().expect("supervision request flushes");
}

fn test_exchange() -> ExchangeIdentifier {
    ExchangeIdentifier::new(
        SessionEpoch::new(0),
        ExchangeLane::Connector,
        LaneSequence::first(),
    )
}

fn wait_for_socket(socket: &Path) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        if socket.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("socket was not created: {}", socket.display());
}

fn stop_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}
