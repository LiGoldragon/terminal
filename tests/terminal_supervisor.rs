use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use meta_signal_terminal::{
    CreateSession, MetaTerminalFrame as MetaFrame, MetaTerminalFrameBody as MetaFrameBody,
    MetaTerminalOperationKind, MetaTerminalReply, MetaTerminalRequest,
    MetaTerminalRequestUnimplemented, MetaTerminalUnimplementedReason, TerminalCommand,
    TerminalCommandExecutable,
};
use signal_frame::{
    ExchangeIdentifier, ExchangeLane, LaneSequence, NonEmpty, Request as FrameRequest, SessionEpoch,
};
use signal_persona::{
    ComponentHealth, ComponentKind, ComponentName, EngineManagementProtocolVersion,
    Frame as SupervisionFrame, FrameBody as SupervisionFrameBody, Operation as SupervisionRequest,
    Presence, Query as SupervisionQuery, Reply as SupervisionReply,
};
use signal_terminal::{
    Frame, FrameBody, ListSessions, Output, PromptPattern, PromptPatternBytes,
    PromptPatternIdentifier, PromptPatternRegistered, RegisterPromptPattern, ResolveSession,
    SessionEntry, SessionResolved, SubscribeTerminalWorkerLifecycle, TerminalDeliveryAttemptState,
    TerminalEvent, TerminalName, TerminalWorkerKind, TerminalWorkerLifecycle,
    TerminalWorkerLifecycleEvent, TerminalWorkerLifecycleSnapshot, TerminalWorkerStop,
    TerminalWorkerStopReason, WirePath,
};
use terminal::registry::SessionRegistration;
use terminal::supervisor::{
    TerminalSupervisor, TerminalSupervisorCommandLine, TerminalSupervisorDaemon,
    TerminalSupervisorFrameCodec, TerminalSupervisorMetaRequest,
};
use terminal::tables::{StoreLocation, TerminalTables};
use terminal::{
    Configuration, SocketMode, SupervisionFrameCodec, TerminalDaemonConfigurationFile,
    TerminalSupervisorDaemonCommand,
};
use triad_runtime::BindingSurface;

static ENVIRONMENT_LOCK: Mutex<()> = Mutex::new(());

/// Widen a `u8` byte literal into the schema-emitted `Integer` (`u64`)
/// byte vector the signal-terminal contract carries on its byte-bearing
/// fields.
fn signal_bytes(bytes: &[u8]) -> Vec<u64> {
    bytes.iter().map(|byte| u64::from(*byte)).collect()
}

fn literal_pattern(bytes: &[u8]) -> signal_terminal::Pattern {
    PromptPattern::LiteralSuffix(PromptPatternBytes::new(signal_bytes(bytes))).into()
}

fn register_pattern_request(terminal: TerminalName, suffix: &[u8]) -> RegisterPromptPattern {
    RegisterPromptPattern {
        terminal: terminal.into(),
        pattern: literal_pattern(suffix),
    }
}

fn prompt_pattern_registered(terminal: TerminalName) -> Output {
    PromptPatternRegistered {
        terminal: terminal.into(),
        pattern_identifier: PromptPatternIdentifier::new("from-cell".to_string()).into(),
    }
    .into()
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
fn terminal_supervisor_frame_codec_rejects_multi_payload_request() {
    let request = FrameRequest::from_payloads(NonEmpty::from_head_and_tail(
        register_pattern_request(TerminalName::new("operator".to_string()), b"ready> ").into(),
        vec![
            register_pattern_request(TerminalName::new("operator".to_string()), b"again> ").into(),
        ],
    ));
    let frame = Frame::new(FrameBody::Request {
        exchange: test_exchange(),
        request,
    });
    let bytes = frame.encode_length_prefixed().expect("frame encodes");
    let mut input = bytes.as_slice();
    let error = TerminalSupervisorFrameCodec::default()
        .read_request(&mut input)
        .expect_err("mismatched verb is rejected");

    assert!(
        matches!(error, terminal::Error::UnexpectedSignalFrame { .. }),
        "multi-payload request is rejected as a structural frame mismatch: {error:?}"
    );
}

#[test]
fn terminal_supervisor_socket_routes_through_component_sema() {
    let fixture = SupervisorFixture::new("routes-through-sema");
    let terminal = TerminalName::new("operator".to_string());
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
                register_pattern_request(terminal.clone(), b"ready> ").into()
            );
            let stream: &mut UnixStream = stream.get_mut();
            codec
                .write_event(stream, prompt_pattern_registered(terminal))
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
            register_pattern_request(terminal.clone(), b"ready> ").into(),
        )
        .expect("client writes supervisor request");
    let event = codec
        .read_event(&mut stream)
        .expect("client reads supervisor event");

    assert_eq!(event, prompt_pattern_registered(terminal));
    assert_eq!(
        served.join().expect("supervisor server joins"),
        prompt_pattern_registered(TerminalName::new("operator".to_string()))
    );
    let tables = TerminalTables::open(&fixture.store()).expect("terminal tables open");
    let attempts = tables
        .delivery_attempt_records()
        .expect("delivery attempts are readable");
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        attempts[0].terminal(),
        &TerminalName::new("operator".to_string())
    );
    assert_eq!(attempts[0].state(), TerminalDeliveryAttemptState::Started);

    let events = tables
        .terminal_event_records()
        .expect("terminal events are readable");
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].event(),
        &prompt_pattern_registered(TerminalName::new("operator".to_string()))
    );
    cell.join().expect("fake cell joins");
}

#[test]
fn terminal_supervisor_resolves_session_without_contacting_cell() {
    let fixture = SupervisorFixture::new("resolve-session");
    let terminal = TerminalName::new("operator".to_string());
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
            ResolveSession::new(terminal.clone().into()).into(),
        )
        .expect("client writes supervisor request");
    let event = codec
        .read_event(&mut stream)
        .expect("client reads supervisor event");
    let expected = Output::from(SessionResolved {
        name: terminal.into(),
        data_socket_path: WirePath::new(fixture.cell_data_socket().display().to_string()).into(),
    });

    assert_eq!(event, expected);
    assert_eq!(served.join().expect("supervisor server joins"), expected);
}

#[test]
fn terminal_supervisor_lists_sessions_without_contacting_cells() {
    let fixture = SupervisorFixture::new("list-sessions");
    let operator = TerminalName::new("operator".to_string());
    let designer = TerminalName::new("designer".to_string());
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
    let Output::SessionList(list) = event.clone() else {
        panic!("expected session list reply, got {event:?}");
    };
    let mut entries = list.payload().payload().clone();
    entries.sort_by(|left, right| {
        left.name
            .payload()
            .payload()
            .cmp(right.name.payload().payload())
    });
    let expected_entries = vec![
        SessionEntry {
            name: designer.into(),
            data_socket_path: WirePath::new(designer_data_socket.display().to_string()).into(),
        },
        SessionEntry {
            name: operator.into(),
            data_socket_path: WirePath::new(operator_data_socket.display().to_string()).into(),
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
    let request = MetaTerminalRequest::CreateSession(CreateSession {
        name: TerminalName::new("operator".to_string()),
        command: TerminalCommand {
            executable: TerminalCommandExecutable::new("pi"),
            arguments: Vec::new(),
        },
        environment: Vec::new(),
        working_directory: None,
    });

    let reply = runtime.block_on(async {
        supervisor
            .ask(TerminalSupervisorMetaRequest::new(request))
            .await
            .expect("meta request reaches supervisor actor")
    });

    assert_eq!(
        reply.into_reply(),
        MetaTerminalReply::MetaTerminalRequestUnimplemented(MetaTerminalRequestUnimplemented {
            terminal: TerminalName::new("operator".to_string()),
            operation: MetaTerminalOperationKind::CreateSession,
            reason: MetaTerminalUnimplementedReason::NotBuiltYet,
        })
    );
    runtime
        .block_on(TerminalSupervisor::stop(supervisor))
        .expect("supervisor stops");
}

#[test]
fn terminal_supervisor_command_line_uses_spawn_envelope_environment() {
    let _lock = ENVIRONMENT_LOCK
        .lock()
        .expect("environment lock is available");
    let fixture = SupervisorFixture::new("spawn-envelope-environment");
    let socket = fixture.root.join("run").join("terminal.sock");
    let state = fixture.root.join("state").join("terminal.sema");
    let terminal_store = EnvironmentRestore::capture("TERMINAL_STORE");
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
fn terminal_daemon_configuration_raises_working_request_concurrency() {
    use signal_terminal::SocketMode as WireSocketMode;
    use signal_terminal::TerminalDaemonConfiguration;
    use signal_terminal::{OwnerIdentity, UnixUserIdentifier};

    let fixture = SupervisorFixture::new("request-concurrency");
    let raw = TerminalDaemonConfiguration {
        terminal_socket_path: WirePath::new(fixture.supervisor_socket().display().to_string())
            .into(),
        terminal_socket_mode: WireSocketMode::new(0o600).into(),
        meta_terminal_socket_path: WirePath::new(
            fixture.meta_supervisor_socket().display().to_string(),
        )
        .into(),
        meta_terminal_socket_mode: WireSocketMode::new(0o600).into(),
        supervision_socket_path: WirePath::new(fixture.supervision_socket().display().to_string())
            .into(),
        supervision_socket_mode: WireSocketMode::new(0o600).into(),
        store_path: WirePath::new(fixture.store().as_path().display().to_string()).into(),
        owner_identity: OwnerIdentity::UnixUser(UnixUserIdentifier::new(1000)),
    };
    let configuration = Configuration::from_raw(raw);

    assert_eq!(configuration.request_concurrency_limit().count(), 64);
}

#[test]
fn terminal_supervisor_answers_component_supervision_relation() {
    use signal_terminal::SocketMode as WireSocketMode;
    use signal_terminal::TerminalDaemonConfiguration;
    use signal_terminal::{OwnerIdentity, UnixUserIdentifier};

    let fixture = SupervisorFixture::new("supervision");
    let supervision_socket = fixture.supervision_socket();
    let meta_socket = fixture.meta_supervisor_socket();
    let configuration_path = fixture.root.join("terminal-daemon.rkyv");
    let configuration = TerminalDaemonConfiguration {
        terminal_socket_path: WirePath::new(fixture.supervisor_socket().display().to_string())
            .into(),
        terminal_socket_mode: WireSocketMode::new(0o600).into(),
        meta_terminal_socket_path: WirePath::new(meta_socket.display().to_string()).into(),
        meta_terminal_socket_mode: WireSocketMode::new(0o600).into(),
        supervision_socket_path: WirePath::new(supervision_socket.display().to_string()).into(),
        supervision_socket_mode: WireSocketMode::new(0o600).into(),
        store_path: WirePath::new(fixture.store().as_path().display().to_string()).into(),
        owner_identity: OwnerIdentity::UnixUser(UnixUserIdentifier::new(1000)),
    };
    TerminalDaemonConfigurationFile::new(&configuration_path)
        .write_configuration(&configuration)
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
        "spawned terminal-supervisor applies PERSONA_SOCKET_MODE to its primary socket"
    );

    wait_for_socket(&meta_socket);
    let meta_mode = fs::metadata(&meta_socket)
        .expect("meta socket metadata is readable")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(meta_mode, 0o600);

    let mut meta_stream = UnixStream::connect(&meta_socket).expect("meta client connects");
    write_meta_terminal_request(
        &mut meta_stream,
        MetaTerminalRequest::CreateSession(CreateSession {
            name: TerminalName::new("operator".to_string()),
            command: TerminalCommand {
                executable: TerminalCommandExecutable::new("pi"),
                arguments: Vec::new(),
            },
            environment: Vec::new(),
            working_directory: None,
        }),
    );
    assert_eq!(
        read_meta_terminal_reply(&mut meta_stream),
        MetaTerminalReply::MetaTerminalRequestUnimplemented(MetaTerminalRequestUnimplemented {
            terminal: TerminalName::new("operator".to_string()),
            operation: MetaTerminalOperationKind::CreateSession,
            reason: MetaTerminalUnimplementedReason::NotBuiltYet,
        })
    );

    let mut stream = UnixStream::connect(&supervision_socket).expect("client connects");
    let codec = SupervisionFrameCodec::new(1024 * 1024);

    write_supervision_request(
        &mut stream,
        SupervisionRequest::Announce(
            Presence {
                expected_component: ComponentName::new("terminal").into(),
                expected_kind: ComponentKind::Terminal.into(),
                engine_management_protocol_version: EngineManagementProtocolVersion::new(1),
            }
            .into(),
        ),
    );
    assert!(matches!(
        codec.read_reply(&mut stream).expect("identity reply"),
        SupervisionReply::Identified(identity)
            if identity.payload().component_name.as_ref() == "terminal"
                && identity.payload().component_kind == ComponentKind::Terminal
    ));

    write_supervision_request(
        &mut stream,
        SupervisionRequest::Query(
            SupervisionQuery::ReadinessStatus(ComponentName::new("terminal")).into(),
        ),
    );
    assert!(matches!(
        codec.read_reply(&mut stream).expect("readiness reply"),
        SupervisionReply::Ready(_)
    ));

    write_supervision_request(
        &mut stream,
        SupervisionRequest::Query(
            SupervisionQuery::HealthStatus(ComponentName::new("terminal")).into(),
        ),
    );
    assert!(matches!(
        codec.read_reply(&mut stream).expect("health reply"),
        SupervisionReply::HealthReport(report)
            if report.payload().payload() == &ComponentHealth::Running
    ));

    stop_child(&mut child);
}

#[test]
fn terminal_supervisor_configuration_rejects_nota_arguments() {
    let fixture = SupervisorFixture::new("reject-nota-configuration");
    fs::create_dir_all(&fixture.root).expect("fixture directory is created");
    let nota_path = fixture.root.join("terminal-daemon.nota");
    fs::write(&nota_path, "(TerminalDaemonConfiguration)").expect("write nota fixture");

    let inline = TerminalSupervisorDaemonCommand::from_arguments(["(TerminalDaemonConfiguration)"])
        .configuration()
        .expect_err("inline NOTA is rejected");
    let file = TerminalSupervisorDaemonCommand::from_arguments([nota_path.display().to_string()])
        .configuration()
        .expect_err(".nota file is rejected");

    assert!(matches!(inline, terminal::Error::Argument(_)));
    assert!(matches!(file, terminal::Error::Argument(_)));
}

#[test]
fn terminal_supervisor_subscription_streams_initial_state_then_delta() {
    let fixture = SupervisorFixture::new("streams-lifecycle");
    let terminal = TerminalName::new("responder".to_string());
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
                SubscribeTerminalWorkerLifecycle::new(terminal.clone().into()).into()
            );
            let stream: &mut UnixStream = stream.get_mut();
            codec
                .write_event(
                    stream,
                    Output::from(TerminalWorkerLifecycleSnapshot {
                        terminal: terminal.clone().into(),
                        observations: vec![TerminalWorkerLifecycle::Started(
                            TerminalWorkerKind::OutputReader,
                        )]
                        .into(),
                    }),
                )
                .expect("fake cell writes lifecycle snapshot");
            codec
                .write_stream_event(
                    stream,
                    TerminalEvent::from(TerminalWorkerLifecycleEvent {
                        terminal: terminal.into(),
                        observation: TerminalWorkerLifecycle::Stopped(worker_stopped(
                            TerminalWorkerKind::OutputReader,
                            TerminalWorkerStopReason::OutputReaderFinished,
                        ))
                        .into(),
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
            SubscribeTerminalWorkerLifecycle::new(terminal.clone().into()).into(),
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
        Output::from(TerminalWorkerLifecycleSnapshot {
            terminal: TerminalName::new("responder".to_string()).into(),
            observations: vec![TerminalWorkerLifecycle::Started(
                TerminalWorkerKind::OutputReader,
            )]
            .into(),
        })
    );
    assert_eq!(
        delta,
        TerminalEvent::from(TerminalWorkerLifecycleEvent {
            terminal: TerminalName::new("responder".to_string()).into(),
            observation: TerminalWorkerLifecycle::Stopped(worker_stopped(
                TerminalWorkerKind::OutputReader,
                TerminalWorkerStopReason::OutputReaderFinished,
            ))
            .into(),
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
        signal_terminal::TerminalOperationKind::SubscribeTerminalWorkerLifecycle
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
        exchange: test_supervision_exchange(),
        request: FrameRequest::from_payload(request),
    });
    let bytes = frame
        .encode_length_prefixed()
        .expect("supervision request encodes");
    stream
        .write_all(bytes.as_slice())
        .expect("supervision request writes");
    stream.flush().expect("supervision request flushes");
}

fn write_meta_terminal_request(stream: &mut UnixStream, request: MetaTerminalRequest) {
    let frame = MetaFrame::new(MetaFrameBody::Request {
        exchange: test_exchange(),
        request: FrameRequest::from_payload(request),
    });
    let bytes = frame
        .encode_length_prefixed()
        .expect("meta request encodes");
    stream
        .write_all(bytes.as_slice())
        .expect("meta request writes");
    stream.flush().expect("meta request flushes");
}

fn read_meta_terminal_reply(stream: &mut UnixStream) -> MetaTerminalReply {
    let mut prefix = [0_u8; 4];
    stream.read_exact(&mut prefix).expect("meta reply prefix");
    let length = u32::from_be_bytes(prefix) as usize;
    let mut bytes = Vec::with_capacity(4 + length);
    bytes.extend_from_slice(&prefix);
    bytes.resize(4 + length, 0);
    stream.read_exact(&mut bytes[4..]).expect("meta reply body");
    let frame = MetaFrame::decode_length_prefixed(&bytes).expect("meta reply decodes");
    match frame.into_body() {
        MetaFrameBody::Reply { reply, .. } => match reply {
            signal_frame::Reply::Accepted { per_operation, .. } => {
                match per_operation.into_head() {
                    signal_frame::SubReply::Ok(reply) => reply,
                    other => panic!("expected accepted meta reply, got {other:?}"),
                }
            }
            other => panic!("expected accepted meta frame, got {other:?}"),
        },
        other => panic!("expected meta reply frame, got {other:?}"),
    }
}

fn test_exchange() -> ExchangeIdentifier {
    ExchangeIdentifier::new(
        SessionEpoch::new(0),
        ExchangeLane::Connector,
        LaneSequence::first(),
    )
}

fn test_supervision_exchange() -> ExchangeIdentifier {
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
