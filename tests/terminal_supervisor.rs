use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use meta_signal_terminal::{
    CreateSession, MetaTerminalOperationKind, MetaTerminalRequestUnimplemented,
    MetaTerminalUnimplementedReason, Query as MetaQuery, Response as MetaResponse, TerminalCommand,
};
use signal_persona::{
    ComponentHealth, ComponentKind, LifecycleQuery, Presence, Query as SupervisionQuery,
    Response as SupervisionResponse,
};
use signal_terminal::{
    ListSessionsRequest, PromptPattern, PromptPatternRegisteredReply, Query,
    RegisterPromptPatternRequest, ResolveSessionRequest, Response, SessionEntry,
    SessionResolvedReply, SubscribeTerminalWorkerLifecycleRequest, TerminalEvent, TerminalName,
    TerminalWorkerKind, TerminalWorkerLifecycle, TerminalWorkerLifecycleEventPayload,
    TerminalWorkerLifecycleSnapshotReply, TerminalWorkerStop, TerminalWorkerStopReason,
};
use terminal::contract::widen_bytes;
use terminal::records::TerminalDeliveryAttemptState;
use terminal::registry::SessionRegistration;
use terminal::supervisor::{
    TerminalSupervisor, TerminalSupervisorCommandLine, TerminalSupervisorDaemon,
    TerminalSupervisorEnvironment, TerminalSupervisorFrameCodec, TerminalSupervisorMetaRequest,
};
use terminal::tables::{StoreLocation, TerminalTables};
use terminal::{
    Configuration, SocketMode, SupervisionFrameCodec, TerminalDaemonConfigurationFile,
    TerminalSupervisorDaemonCommand, frame,
};
use triad_runtime::BindingSurface;

fn literal_pattern(bytes: &[u8]) -> PromptPattern {
    PromptPattern::LiteralSuffix(widen_bytes(bytes))
}

fn register_pattern_request(terminal: TerminalName, suffix: &[u8]) -> Query {
    Query::RegisterPromptPattern(RegisterPromptPatternRequest {
        terminal,
        pattern: literal_pattern(suffix),
    })
}

fn prompt_pattern_registered(terminal: TerminalName) -> Response {
    Response::PromptPatternRegistered(PromptPatternRegisteredReply {
        terminal,
        pattern_identifier: "from-cell".to_string(),
    })
}

fn worker_stopped(
    kind: TerminalWorkerKind,
    reason: TerminalWorkerStopReason,
) -> TerminalWorkerStop {
    TerminalWorkerStop {
        terminal_worker_kind: kind,
        terminal_worker_stop_reason: reason,
    }
}

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
        let store = StoreLocation::new(root.join("terminal.sema"));
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

    fn meta_supervisor_socket(&self) -> PathBuf {
        self.root.join("meta-supervisor.sock")
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

fn daemon_configuration(
    fixture: &SupervisorFixture,
) -> signal_terminal::TerminalDaemonConfiguration {
    use signal_terminal::{OwnerIdentity, TerminalDaemonConfiguration};

    TerminalDaemonConfiguration {
        terminal_socket_path: fixture.supervisor_socket().display().to_string(),
        terminal_socket_mode: 0o600,
        meta_terminal_socket_path: fixture.meta_supervisor_socket().display().to_string(),
        meta_terminal_socket_mode: 0o600,
        supervision_socket_path: fixture.supervision_socket().display().to_string(),
        supervision_socket_mode: 0o600,
        store_path: fixture.store().as_path().display().to_string(),
        owner_identity: OwnerIdentity::UnixUser(1000),
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

/// The retired envelope let one request frame carry many payloads, so the
/// codec had to refuse the multi-payload case. `signal-terminal` 2.0.1
/// carries one value per frame and cannot express more, so what remains to
/// witness is that bytes which are not a valid archive are refused rather
/// than misread.
#[test]
fn terminal_supervisor_frame_codec_rejects_a_malformed_archive() {
    let mut framed = Vec::new();
    framed.extend_from_slice(&3_u32.to_be_bytes());
    framed.extend_from_slice(&[1, 2, 3]);
    let mut input = framed.as_slice();

    let error = TerminalSupervisorFrameCodec::new()
        .read_request(&mut input)
        .expect_err("a malformed archive is rejected");

    assert!(
        matches!(error, terminal::Error::UnexpectedSignalFrame { .. }),
        "malformed archive is rejected as a frame mismatch: {error:?}"
    );
}

#[test]
fn terminal_supervisor_socket_routes_through_component_sema() {
    let fixture = SupervisorFixture::new("routes-through-sema");
    let terminal = "operator".to_string();
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
            let codec = TerminalSupervisorFrameCodec::new();
            let request = codec
                .read_request(&mut stream)
                .expect("supervisor writes terminal signal request");
            assert_eq!(
                request,
                register_pattern_request(terminal.clone(), b"ready> ")
            );
            let stream: &mut UnixStream = stream.get_mut();
            codec
                .write_reply(stream, &prompt_pattern_registered(terminal))
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
    let codec = TerminalSupervisorFrameCodec::new();
    codec
        .write_request(
            &mut stream,
            &register_pattern_request(terminal.clone(), b"ready> "),
        )
        .expect("client writes supervisor request");
    let event = codec
        .read_reply(&mut stream)
        .expect("client reads supervisor event");

    assert_eq!(event, prompt_pattern_registered(terminal.clone()));
    assert_eq!(
        served.join().expect("supervisor server joins"),
        prompt_pattern_registered(terminal.clone())
    );
    let tables = TerminalTables::open(&fixture.store()).expect("terminal tables open");
    let attempts = tables
        .delivery_attempt_records()
        .expect("delivery attempts are readable");
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].terminal(), &terminal);
    assert_eq!(attempts[0].state(), TerminalDeliveryAttemptState::Started);

    let events = tables
        .terminal_event_records()
        .expect("terminal events are readable");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event(), &prompt_pattern_registered(terminal));
    cell.join().expect("fake cell joins");
}

#[test]
fn terminal_supervisor_resolves_session_without_contacting_cell() {
    let fixture = SupervisorFixture::new("resolve-session");
    let terminal = "operator".to_string();
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
    let codec = TerminalSupervisorFrameCodec::new();
    codec
        .write_request(
            &mut stream,
            &Query::ResolveSession(ResolveSessionRequest {
                name: terminal.clone(),
            }),
        )
        .expect("client writes supervisor request");
    let event = codec
        .read_reply(&mut stream)
        .expect("client reads supervisor event");
    let expected = Response::SessionResolved(SessionResolvedReply {
        name: terminal,
        data_socket_path: fixture.cell_data_socket().display().to_string(),
    });

    assert_eq!(event, expected);
    assert_eq!(served.join().expect("supervisor server joins"), expected);
}

#[test]
fn terminal_supervisor_lists_sessions_without_contacting_cells() {
    let fixture = SupervisorFixture::new("list-sessions");
    let operator = "operator".to_string();
    let designer = "designer".to_string();
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
    let codec = TerminalSupervisorFrameCodec::new();
    codec
        .write_request(&mut stream, &Query::ListSessions(ListSessionsRequest {}))
        .expect("client writes supervisor request");
    let event = codec
        .read_reply(&mut stream)
        .expect("client reads supervisor event");
    let Response::SessionList(list) = event.clone() else {
        panic!("expected session list reply, got {event:?}");
    };
    let mut entries = list.session_entries.clone();
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    let expected_entries = vec![
        SessionEntry {
            name: designer,
            data_socket_path: designer_data_socket.display().to_string(),
        },
        SessionEntry {
            name: operator,
            data_socket_path: operator_data_socket.display().to_string(),
        },
    ];

    assert_eq!(entries, expected_entries);
    assert_eq!(served.join().expect("supervisor server joins"), event);
}

#[test]
fn terminal_supervisor_meta_request_reaches_meta_surface_without_ordinary_variant() {
    let fixture = SupervisorFixture::new("meta-session-unimplemented");
    let runtime = tokio::runtime::Runtime::new().expect("runtime starts");
    let supervisor = runtime.block_on(TerminalSupervisor::start(fixture.store()));
    let create = CreateSession {
        terminal_name: "operator".to_string(),
        terminal_command: TerminalCommand {
            terminal_command_executable: "pi".to_string(),
            terminal_command_arguments: Vec::new(),
        },
        terminal_environment: Vec::new(),
        selected_working_directory: None,
    };

    let reply = runtime.block_on(async {
        supervisor
            .ask(TerminalSupervisorMetaRequest::new(
                MetaQuery::CreateSession(create.clone()),
            ))
            .await
            .expect("meta request reaches supervisor actor")
    });

    assert_eq!(
        reply.into_reply(),
        MetaResponse::MetaTerminalRequestUnimplemented(MetaTerminalRequestUnimplemented {
            terminal_name: "operator".to_string(),
            meta_terminal_operation_kind: MetaTerminalOperationKind::CreateSession(create),
            meta_terminal_unimplemented_reason: MetaTerminalUnimplementedReason::NotBuiltYet,
        })
    );
    runtime
        .block_on(TerminalSupervisor::stop(supervisor))
        .expect("supervisor stops");
}

/// With no arguments, the supervisor takes its socket and store from the
/// spawn envelope it is handed.
#[test]
fn terminal_supervisor_command_line_uses_spawn_envelope_environment() {
    let fixture = SupervisorFixture::new("spawn-envelope-environment");
    let socket = fixture.root.join("run").join("terminal.sock");
    let state = fixture.root.join("state").join("terminal.sema");

    let daemon = TerminalSupervisorCommandLine::from_arguments_with_environment(
        Vec::<String>::new(),
        TerminalSupervisorEnvironment::new(
            Some(socket.clone()),
            Some(StoreLocation::new(state.clone())),
        ),
    )
    .daemon()
    .expect("supervisor daemon resolves from the spawn envelope it is handed");
    assert_eq!(daemon.socket(), &socket);
    assert_eq!(daemon.store().as_path(), state.as_path());
}

/// An argument overrides the envelope, and a missing socket in both is a
/// typed refusal rather than a default.
#[test]
fn terminal_supervisor_command_line_requires_a_socket_from_somewhere() {
    let error = TerminalSupervisorCommandLine::from_arguments_with_environment(
        Vec::<String>::new(),
        TerminalSupervisorEnvironment::default(),
    )
    .daemon()
    .expect_err("no socket in arguments or envelope is a refusal");

    assert!(matches!(
        error,
        terminal::Error::MissingSocket {
            component: "terminal-supervisor"
        }
    ));
}

#[test]
fn terminal_daemon_configuration_raises_working_request_concurrency() {
    let fixture = SupervisorFixture::new("request-concurrency");
    let configuration = Configuration::from_raw(daemon_configuration(&fixture));

    assert_eq!(configuration.request_concurrency_limit().count(), 64);
}

#[test]
fn terminal_supervisor_answers_component_supervision_relation() {
    let fixture = SupervisorFixture::new("supervision");
    let supervision_socket = fixture.supervision_socket();
    let meta_socket = fixture.meta_supervisor_socket();
    let configuration_path = fixture.root.join("terminal-daemon.rkyv");
    TerminalDaemonConfigurationFile::new(&configuration_path)
        .write_configuration(&daemon_configuration(&fixture))
        .expect("write terminal config");

    let mut child = Command::new(env!("CARGO_BIN_EXE_terminal-supervisor"))
        .arg(&configuration_path)
        .spawn()
        .expect("terminal-supervisor starts");

    wait_for_socket(&supervision_socket);
    let mode = fs::metadata(&supervision_socket)
        .expect("supervision socket metadata is readable")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);

    // The primary supervisor socket — the engine-facing one that resolves
    // named terminals from Sema and forwards Signal control frames to
    // terminal-cell — also honors the managed socket mode when the binary
    // is spawned in the engine envelope. That needs a binary-spawn
    // witness, not only a library-level one.
    wait_for_socket(&fixture.supervisor_socket());
    let supervisor_mode = fs::metadata(fixture.supervisor_socket())
        .expect("supervisor socket metadata is readable")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        supervisor_mode, 0o600,
        "spawned terminal-supervisor applies the managed socket mode to its primary socket"
    );

    wait_for_socket(&meta_socket);
    let meta_mode = fs::metadata(&meta_socket)
        .expect("meta socket metadata is readable")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(meta_mode, 0o600);

    let create = CreateSession {
        terminal_name: "operator".to_string(),
        terminal_command: TerminalCommand {
            terminal_command_executable: "pi".to_string(),
            terminal_command_arguments: Vec::new(),
        },
        terminal_environment: Vec::new(),
        selected_working_directory: None,
    };
    let mut meta_stream = UnixStream::connect(&meta_socket).expect("meta client connects");
    frame::meta::write_query(&mut meta_stream, &MetaQuery::CreateSession(create.clone()))
        .expect("meta request writes");
    assert_eq!(
        frame::meta::read_response(&mut meta_stream).expect("meta reply reads"),
        MetaResponse::MetaTerminalRequestUnimplemented(MetaTerminalRequestUnimplemented {
            terminal_name: "operator".to_string(),
            meta_terminal_operation_kind: MetaTerminalOperationKind::CreateSession(create),
            meta_terminal_unimplemented_reason: MetaTerminalUnimplementedReason::NotBuiltYet,
        })
    );

    let mut stream = UnixStream::connect(&supervision_socket).expect("client connects");
    let codec = SupervisionFrameCodec::new();

    codec
        .write_request(
            &mut stream,
            &SupervisionQuery::Announce(Presence {
                expected_component: "terminal".to_string(),
                expected_kind: ComponentKind::Terminal,
                engine_management_protocol_version: 1,
            }),
        )
        .expect("announce writes");
    assert!(matches!(
        codec.read_reply(&mut stream).expect("identity reply"),
        SupervisionResponse::Identified(identity)
            if identity.component_name == "terminal"
                && identity.component_kind == ComponentKind::Terminal
    ));

    codec
        .write_request(
            &mut stream,
            &SupervisionQuery::Query(LifecycleQuery::ReadinessStatus("terminal".to_string())),
        )
        .expect("readiness writes");
    assert!(matches!(
        codec.read_reply(&mut stream).expect("readiness reply"),
        SupervisionResponse::Ready(_)
    ));

    codec
        .write_request(
            &mut stream,
            &SupervisionQuery::Query(LifecycleQuery::HealthStatus("terminal".to_string())),
        )
        .expect("health writes");
    assert!(matches!(
        codec.read_reply(&mut stream).expect("health reply"),
        SupervisionResponse::HealthReport(report) if report == ComponentHealth::Running
    ));

    stop_child(&mut child);
}

/// The daemon takes one binary rkyv configuration file and nothing else.
///
/// Inline text names no file at all, so it is refused while the argument is
/// still being read — before any path is opened.
#[test]
fn terminal_supervisor_configuration_rejects_an_inline_text_argument() {
    let inline = TerminalSupervisorDaemonCommand::from_arguments(["TerminalDaemonConfiguration"])
        .configuration()
        .expect_err("inline text names no configuration file");

    assert!(matches!(inline, terminal::Error::Argument(_)));
}

/// A file that is not a configuration archive is refused at decode, before
/// any socket is bound.
#[test]
fn terminal_supervisor_configuration_rejects_a_file_that_is_not_an_archive() {
    let fixture = SupervisorFixture::new("reject-text-configuration");
    fs::create_dir_all(&fixture.root).expect("fixture directory is created");
    let text_path = fixture.root.join("terminal-daemon.datom");
    fs::write(&text_path, "TerminalDaemonConfiguration").expect("write text fixture");

    let file = TerminalSupervisorDaemonCommand::from_arguments([text_path.display().to_string()])
        .configuration()
        .expect_err("a file that is not a configuration archive is rejected");

    assert!(matches!(file, terminal::Error::ConfigurationArchiveDecode));
}

#[test]
fn terminal_supervisor_subscription_streams_initial_state_then_delta() {
    let fixture = SupervisorFixture::new("streams-lifecycle");
    let terminal = "responder".to_string();
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
            let codec = TerminalSupervisorFrameCodec::new();
            let request = codec
                .read_request(&mut stream)
                .expect("supervisor writes subscription request");
            assert_eq!(
                request,
                Query::SubscribeTerminalWorkerLifecycle(SubscribeTerminalWorkerLifecycleRequest {
                    terminal: terminal.clone(),
                })
            );
            let stream: &mut UnixStream = stream.get_mut();
            codec
                .write_reply(
                    stream,
                    &Response::TerminalWorkerLifecycleSnapshot(
                        TerminalWorkerLifecycleSnapshotReply {
                            terminal: terminal.clone(),
                            observations: vec![TerminalWorkerLifecycle::Started(
                                TerminalWorkerKind::OutputReader,
                            )],
                        },
                    ),
                )
                .expect("fake cell writes lifecycle snapshot");
            codec
                .write_reply(
                    stream,
                    &Response::Event(TerminalEvent::TerminalWorkerLifecycleEvent(
                        TerminalWorkerLifecycleEventPayload {
                            terminal,
                            observation: TerminalWorkerLifecycle::Stopped(worker_stopped(
                                TerminalWorkerKind::OutputReader,
                                TerminalWorkerStopReason::OutputReaderFinished,
                            )),
                        },
                    )),
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
    let codec = TerminalSupervisorFrameCodec::new();
    codec
        .write_request(
            &mut stream,
            &Query::SubscribeTerminalWorkerLifecycle(SubscribeTerminalWorkerLifecycleRequest {
                terminal: terminal.clone(),
            }),
        )
        .expect("client writes subscription request");
    let snapshot = codec
        .read_reply(&mut stream)
        .expect("client reads initial lifecycle state");
    let delta = codec
        .read_reply(&mut stream)
        .expect("client reads lifecycle delta");

    assert_eq!(
        snapshot,
        Response::TerminalWorkerLifecycleSnapshot(TerminalWorkerLifecycleSnapshotReply {
            terminal: terminal.clone(),
            observations: vec![TerminalWorkerLifecycle::Started(
                TerminalWorkerKind::OutputReader,
            )],
        })
    );
    assert_eq!(
        delta,
        Response::Event(TerminalEvent::TerminalWorkerLifecycleEvent(
            TerminalWorkerLifecycleEventPayload {
                terminal: terminal.clone(),
                observation: TerminalWorkerLifecycle::Stopped(worker_stopped(
                    TerminalWorkerKind::OutputReader,
                    TerminalWorkerStopReason::OutputReaderFinished,
                )),
            }
        ))
    );
    assert_eq!(served.join().expect("supervisor server joins"), snapshot);

    let tables = TerminalTables::open(&fixture.store()).expect("terminal tables open");
    let attempts = tables
        .delivery_attempt_records()
        .expect("delivery attempts are readable");
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        attempts[0].operation(),
        &signal_terminal::TerminalOperationKind::SubscribeTerminalWorkerLifecycle
    );

    let events = tables
        .terminal_event_records()
        .expect("terminal events are readable");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event(), &snapshot);
    cell.join().expect("fake cell joins");
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
