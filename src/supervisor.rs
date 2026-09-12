use std::ffi::OsString;
use std::io::{BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use kameo::actor::{Actor, ActorRef, Spawn};
use kameo::error::Infallible;
use kameo::message::{Context, Message};
use meta_signal_terminal::{
    MetaTerminalOperationKind, MetaTerminalRequestUnimplemented, MetaTerminalUnimplementedReason,
    Query as MetaQuery, Response as MetaResponse,
};
use signal_terminal::{
    Query, Response, ResolveSessionRequest, SessionEntry, SessionListReply, SessionResolvedReply,
    SubscribeTerminalWorkerLifecycleRequest, TerminalDaemonConfiguration, TerminalOperationKind,
    TerminalRejectedReply, TerminalRejectionReason,
};

use crate::contract::TerminalTransportBinding;
use crate::error::{Error, Result};
use crate::frame;
use crate::operation::{
    meta_query_operation_kind, meta_query_terminal, query_operation_kind, query_terminal,
    response_terminal,
};
use crate::records::{
    TerminalDeliveryAttemptObservation, TerminalEventObservation, TerminalObservationSequence,
};
use crate::socket::SocketMode;
use crate::supervision::{SupervisionListener, SupervisionProfile, SupervisionSocketMode};
use crate::tables::{StoreLocation, TerminalTables};

#[derive(Debug, Clone, PartialEq)]
pub struct TerminalSupervisorDaemon {
    socket: PathBuf,
    store: StoreLocation,
    socket_mode: Option<SocketMode>,
    supervision: Option<SupervisionListener>,
}

impl TerminalSupervisorDaemon {
    /// Canonical constructor — production launch reads a binary
    /// `TerminalDaemonConfiguration` and hands the decoded record here.
    pub fn from_configuration(configuration: TerminalDaemonConfiguration) -> Self {
        let supervision = SupervisionListener::new(
            SupervisionProfile::terminal(),
            PathBuf::from(&configuration.supervision_socket_path),
            SupervisionSocketMode::from_octal(configuration.supervision_socket_mode as u32),
        );
        Self {
            socket: PathBuf::from(&configuration.terminal_socket_path),
            store: StoreLocation::new(&configuration.store_path),
            socket_mode: Some(SocketMode::from_octal(
                configuration.terminal_socket_mode as u32,
            )),
            supervision: Some(supervision),
        }
    }

    pub fn from_socket(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            store: StoreLocation::from_environment(),
            socket_mode: None,
            supervision: None,
        }
    }

    pub fn with_store(mut self, store: StoreLocation) -> Self {
        self.store = store;
        self
    }

    pub fn with_socket_mode(mut self, socket_mode: SocketMode) -> Self {
        self.socket_mode = Some(socket_mode);
        self
    }

    pub fn socket(&self) -> &PathBuf {
        &self.socket
    }

    pub fn store(&self) -> &StoreLocation {
        &self.store
    }

    pub fn run(self) -> Result<()> {
        let supervision = self.supervision.clone();
        let bound = self.bind()?;
        let _supervision = supervision.map(SupervisionListener::spawn).transpose()?;
        eprintln!("terminal-supervisor socket={}", bound.socket.display());
        bound.serve_forever()
    }

    pub fn bind(self) -> Result<BoundTerminalSupervisorDaemon> {
        if let Some(parent) = self.socket.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _ = std::fs::remove_file(&self.socket);
        let listener = UnixListener::bind(&self.socket)?;
        if let Some(socket_mode) = self.socket_mode {
            socket_mode.apply_to(&self.socket)?;
        }
        let runtime = tokio::runtime::Runtime::new()?;
        let supervisor = runtime.block_on(TerminalSupervisor::start(self.store));
        Ok(BoundTerminalSupervisorDaemon {
            socket: self.socket,
            runtime,
            listener,
            supervisor,
        })
    }

    pub fn serve_one(self) -> Result<Response> {
        self.bind()?.serve_one()
    }

    fn handle_connection(
        runtime: &tokio::runtime::Runtime,
        supervisor: &ActorRef<TerminalSupervisor>,
        stream: UnixStream,
    ) -> Result<Response> {
        let mut connection = TerminalSupervisorConnection::from_stream(stream);
        let request = connection.read_signal_request()?;
        if let Query::SubscribeTerminalWorkerLifecycle(subscription) = request {
            return Self::handle_subscription(runtime, supervisor, connection, subscription);
        }
        let event = runtime.block_on(async {
            supervisor
                .ask(TerminalSupervisorRequest::new(request))
                .await
                .map_err(|error| Error::ActorCall {
                    detail: error.to_string(),
                })
        })?;
        connection.write_signal_reply(&event)?;
        Ok(event)
    }

    fn handle_subscription(
        runtime: &tokio::runtime::Runtime,
        supervisor: &ActorRef<TerminalSupervisor>,
        mut client: TerminalSupervisorConnection,
        subscription: SubscribeTerminalWorkerLifecycleRequest,
    ) -> Result<Response> {
        let start = runtime.block_on(async {
            supervisor
                .ask(TerminalSupervisorSubscriptionRequest::new(subscription))
                .await
                .map_err(|error| Error::ActorCall {
                    detail: error.to_string(),
                })
        })?;
        match start {
            TerminalSupervisorSubscriptionStart::Immediate(event) => {
                client.write_signal_reply(&event)?;
                Ok(event)
            }
            TerminalSupervisorSubscriptionStart::Stream(plan) => {
                Self::stream_subscription(runtime, supervisor, client, plan)
            }
        }
    }

    /// Relay one cell's lifecycle stream to the client.
    ///
    /// Every frame the cell sends is an ordinary `Response`; the first is
    /// the initial snapshot, and each later one is a streamed event. The
    /// supervisor records the snapshot and passes everything through.
    fn stream_subscription(
        runtime: &tokio::runtime::Runtime,
        supervisor: &ActorRef<TerminalSupervisor>,
        mut client: TerminalSupervisorConnection,
        plan: TerminalSupervisorSubscriptionPlan,
    ) -> Result<Response> {
        let mut cell = BufReader::new(UnixStream::connect(plan.socket_path())?);
        frame::terminal::write_query(
            cell.get_mut(),
            &Query::SubscribeTerminalWorkerLifecycle(plan.into_subscription()),
        )?;

        let mut first = None;
        loop {
            let response = match frame::terminal::read_response(&mut cell) {
                Ok(response) => response,
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    break;
                }
                Err(error) => return Err(error),
            };
            if first.is_none() {
                runtime.block_on(async {
                    supervisor
                        .ask(TerminalSupervisorObservedEvent::new(response.clone()))
                        .await
                        .map_err(|error| Error::ActorCall {
                            detail: error.to_string(),
                        })
                })?;
                first = Some(response.clone());
            }
            client.write_signal_reply(&response)?;
        }
        first.ok_or_else(|| Error::UnexpectedSignalFrame {
            got: "subscription ended before initial state".to_string(),
        })
    }
}

pub struct BoundTerminalSupervisorDaemon {
    socket: PathBuf,
    runtime: tokio::runtime::Runtime,
    listener: UnixListener,
    supervisor: ActorRef<TerminalSupervisor>,
}

impl BoundTerminalSupervisorDaemon {
    pub fn socket(&self) -> &PathBuf {
        &self.socket
    }

    pub fn serve_one(self) -> Result<Response> {
        let (stream, _address) = self.listener.accept()?;
        let event =
            TerminalSupervisorDaemon::handle_connection(&self.runtime, &self.supervisor, stream)?;
        self.runtime
            .block_on(TerminalSupervisor::stop(self.supervisor))?;
        let _ = std::fs::remove_file(&self.socket);
        Ok(event)
    }

    pub fn serve_forever(self) -> Result<()> {
        for stream in self.listener.incoming() {
            let stream = stream?;
            let _ = TerminalSupervisorDaemon::handle_connection(
                &self.runtime,
                &self.supervisor,
                stream,
            )?;
        }
        Ok(())
    }
}

/// One accepted connection, reading and writing `signal-terminal` frames.
pub struct TerminalSupervisorConnection {
    stream: BufReader<UnixStream>,
}

impl TerminalSupervisorConnection {
    pub fn from_stream(stream: UnixStream) -> Self {
        Self {
            stream: BufReader::new(stream),
        }
    }

    pub fn read_signal_request(&mut self) -> Result<Query> {
        frame::terminal::read_query(&mut self.stream)
    }

    pub fn write_signal_reply(&mut self, event: &Response) -> Result<()> {
        frame::terminal::write_response(self.stream.get_mut(), event)
    }
}

/// The `signal-terminal` frame pair, as the supervisor and its witnesses use
/// it.
///
/// The retired stack wrapped every value in an exchange envelope with a
/// reply lane and a subscription token. `signal-terminal` 2.0.1 carries the
/// value alone, so this is a thin naming over [`crate::frame::terminal`],
/// kept because the supervisor's witnesses address it by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TerminalSupervisorFrameCodec;

impl TerminalSupervisorFrameCodec {
    pub const fn new() -> Self {
        Self
    }

    pub fn read_request(&self, reader: &mut impl Read) -> Result<Query> {
        frame::terminal::read_query(reader)
    }

    pub fn write_request(&self, writer: &mut impl Write, request: &Query) -> Result<()> {
        frame::terminal::write_query(writer, request)
    }

    pub fn read_reply(&self, reader: &mut impl Read) -> Result<Response> {
        frame::terminal::read_response(reader)
    }

    pub fn write_reply(&self, writer: &mut impl Write, event: &Response) -> Result<()> {
        frame::terminal::write_response(writer, event)
    }
}

#[derive(Debug, Clone, PartialEq, kameo::Reply)]
pub struct TerminalSupervisorState {
    pub served_request_count: i64,
    pub served_meta_request_count: i64,
    pub recorded_event_count: i64,
    pub last_operation: Option<TerminalOperationKind>,
    pub last_meta_operation: Option<MetaTerminalOperationKind>,
}

#[derive(Debug)]
pub struct TerminalSupervisor {
    store: StoreLocation,
    served_request_count: i64,
    served_meta_request_count: i64,
    recorded_event_count: i64,
    last_operation: Option<TerminalOperationKind>,
    last_meta_operation: Option<MetaTerminalOperationKind>,
}

impl TerminalSupervisor {
    pub fn new(store: StoreLocation) -> Self {
        Self {
            store,
            served_request_count: 0,
            served_meta_request_count: 0,
            recorded_event_count: 0,
            last_operation: None,
            last_meta_operation: None,
        }
    }

    pub async fn start(store: StoreLocation) -> ActorRef<Self> {
        let reference = Self::spawn(store);
        reference.wait_for_startup().await;
        reference
    }

    pub async fn stop(reference: ActorRef<Self>) -> Result<()> {
        reference
            .stop_gracefully()
            .await
            .map_err(|error| Error::ActorCall {
                detail: error.to_string(),
            })?;
        reference.wait_for_shutdown().await;
        Ok(())
    }

    fn state(&self) -> TerminalSupervisorState {
        TerminalSupervisorState {
            served_request_count: self.served_request_count,
            served_meta_request_count: self.served_meta_request_count,
            recorded_event_count: self.recorded_event_count,
            last_operation: self.last_operation.clone(),
            last_meta_operation: self.last_meta_operation.clone(),
        }
    }

    fn event_for_request(
        &mut self,
        sequence: TerminalObservationSequence,
        request: Query,
    ) -> Result<Response> {
        match request {
            Query::ListSessions(_) => self.list_sessions(),
            Query::ResolveSession(resolve) => self.resolve_session(resolve),
            other => self.forward_terminal_request(sequence, other),
        }
    }

    fn forward_terminal_request(
        &mut self,
        sequence: TerminalObservationSequence,
        request: Query,
    ) -> Result<Response> {
        let operation = query_operation_kind(&request);
        let terminal = query_terminal(&request)
            .ok_or_else(|| Error::InvalidArgument {
                detail: "request names no terminal".to_string(),
            })?
            .clone();
        let tables = TerminalTables::open(&self.store)?;
        tables.put_delivery_attempt(&TerminalDeliveryAttemptObservation::started(
            sequence,
            terminal.clone(),
            operation,
        ))?;
        let Some(session) = tables.session(&terminal)? else {
            let event = Response::TerminalRejected(TerminalRejectedReply {
                terminal,
                terminal_rejection_reason: TerminalRejectionReason::NotConnected,
            });
            self.record_terminal_event(&tables, event.clone())?;
            return Ok(event);
        };
        let mut binding =
            TerminalTransportBinding::from_socket_path(terminal, session.control_socket_path());
        let event = binding.handle_query(request)?;
        self.record_terminal_event(&tables, event.clone())?;
        Ok(event)
    }

    fn list_sessions(&self) -> Result<Response> {
        let tables = TerminalTables::open(&self.store)?;
        let session_entries = tables
            .sessions()?
            .into_iter()
            .map(|session| SessionEntry {
                name: session.terminal().clone(),
                data_socket_path: session.data_socket_path().to_string(),
            })
            .collect::<Vec<_>>();
        Ok(Response::SessionList(SessionListReply { session_entries }))
    }

    fn resolve_session(&self, resolve: ResolveSessionRequest) -> Result<Response> {
        let tables = TerminalTables::open(&self.store)?;
        let terminal = resolve.name;
        let Some(session) = tables.session(&terminal)? else {
            return Ok(Response::TerminalRejected(TerminalRejectedReply {
                terminal,
                terminal_rejection_reason: TerminalRejectionReason::NotConnected,
            }));
        };
        Ok(Response::SessionResolved(SessionResolvedReply {
            name: session.terminal().clone(),
            data_socket_path: session.data_socket_path().to_string(),
        }))
    }

    fn subscription_start(
        &mut self,
        sequence: TerminalObservationSequence,
        subscription: SubscribeTerminalWorkerLifecycleRequest,
    ) -> Result<TerminalSupervisorSubscriptionStart> {
        let terminal = subscription.terminal.clone();
        let tables = TerminalTables::open(&self.store)?;
        tables.put_delivery_attempt(&TerminalDeliveryAttemptObservation::started(
            sequence,
            terminal.clone(),
            TerminalOperationKind::SubscribeTerminalWorkerLifecycle,
        ))?;
        let Some(session) = tables.session(&terminal)? else {
            let event = Response::TerminalRejected(TerminalRejectedReply {
                terminal,
                terminal_rejection_reason: TerminalRejectionReason::NotConnected,
            });
            self.record_terminal_event(&tables, event.clone())?;
            return Ok(TerminalSupervisorSubscriptionStart::Immediate(event));
        };
        Ok(TerminalSupervisorSubscriptionStart::Stream(
            TerminalSupervisorSubscriptionPlan::new(
                subscription,
                PathBuf::from(session.control_socket_path()),
            ),
        ))
    }

    fn record_terminal_event(&mut self, tables: &TerminalTables, event: Response) -> Result<()> {
        self.recorded_event_count = self.recorded_event_count.saturating_add(1);
        let Some(terminal) = response_terminal(&event).cloned() else {
            return Ok(());
        };
        tables.put_terminal_event(&TerminalEventObservation::new(
            self.recorded_event_count,
            terminal,
            event,
        ))
    }

    fn event_for_meta_request(&mut self, request: MetaQuery) -> MetaResponse {
        let terminal = meta_query_terminal(&request).clone();
        let operation = meta_query_operation_kind(&request);
        MetaResponse::MetaTerminalRequestUnimplemented(MetaTerminalRequestUnimplemented {
            terminal_name: terminal,
            meta_terminal_operation_kind: operation,
            meta_terminal_unimplemented_reason: MetaTerminalUnimplementedReason::NotBuiltYet,
        })
    }
}

#[derive(Debug, Clone, PartialEq, kameo::Reply)]
pub struct TerminalSupervisorMetaReply {
    reply: MetaResponse,
}

impl TerminalSupervisorMetaReply {
    pub const fn new(reply: MetaResponse) -> Self {
        Self { reply }
    }

    pub fn into_reply(self) -> MetaResponse {
        self.reply
    }

    pub const fn reply(&self) -> &MetaResponse {
        &self.reply
    }
}

impl Actor for TerminalSupervisor {
    type Args = StoreLocation;
    type Error = Infallible;

    async fn on_start(
        store: Self::Args,
        _actor_reference: ActorRef<Self>,
    ) -> std::result::Result<Self, Self::Error> {
        Ok(Self::new(store))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadTerminalSupervisorState {
    pub minimum_served_request_count: i64,
}

impl ReadTerminalSupervisorState {
    pub const fn expecting_at_least(minimum_served_request_count: i64) -> Self {
        Self {
            minimum_served_request_count,
        }
    }
}

impl Message<ReadTerminalSupervisorState> for TerminalSupervisor {
    type Reply = TerminalSupervisorState;

    async fn handle(
        &mut self,
        message: ReadTerminalSupervisorState,
        _context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let _satisfied = self.served_request_count >= message.minimum_served_request_count;
        self.state()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TerminalSupervisorRequest {
    request: Query,
}

impl TerminalSupervisorRequest {
    pub fn new(request: Query) -> Self {
        Self { request }
    }
}

impl Message<TerminalSupervisorRequest> for TerminalSupervisor {
    type Reply = Result<Response>;

    async fn handle(
        &mut self,
        message: TerminalSupervisorRequest,
        _context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let sequence = self.served_request_count.saturating_add(1);
        self.last_operation = Some(query_operation_kind(&message.request));
        self.served_request_count = sequence;
        self.event_for_request(sequence, message.request)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TerminalSupervisorMetaRequest {
    request: MetaQuery,
}

impl TerminalSupervisorMetaRequest {
    pub fn new(request: MetaQuery) -> Self {
        Self { request }
    }
}

impl Message<TerminalSupervisorMetaRequest> for TerminalSupervisor {
    type Reply = TerminalSupervisorMetaReply;

    async fn handle(
        &mut self,
        message: TerminalSupervisorMetaRequest,
        _context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.served_meta_request_count = self.served_meta_request_count.saturating_add(1);
        self.last_meta_operation = Some(meta_query_operation_kind(&message.request));
        TerminalSupervisorMetaReply::new(self.event_for_meta_request(message.request))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TerminalSupervisorSubscriptionRequest {
    subscription: SubscribeTerminalWorkerLifecycleRequest,
}

impl TerminalSupervisorSubscriptionRequest {
    pub fn new(subscription: SubscribeTerminalWorkerLifecycleRequest) -> Self {
        Self { subscription }
    }
}

impl Message<TerminalSupervisorSubscriptionRequest> for TerminalSupervisor {
    type Reply = Result<TerminalSupervisorSubscriptionStart>;

    async fn handle(
        &mut self,
        message: TerminalSupervisorSubscriptionRequest,
        _context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let sequence = self.served_request_count.saturating_add(1);
        self.last_operation = Some(TerminalOperationKind::SubscribeTerminalWorkerLifecycle);
        self.served_request_count = sequence;
        self.subscription_start(sequence, message.subscription)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TerminalSupervisorObservedEvent {
    event: Response,
}

impl TerminalSupervisorObservedEvent {
    pub fn new(event: Response) -> Self {
        Self { event }
    }
}

impl Message<TerminalSupervisorObservedEvent> for TerminalSupervisor {
    type Reply = Result<()>;

    async fn handle(
        &mut self,
        message: TerminalSupervisorObservedEvent,
        _context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let tables = TerminalTables::open(&self.store)?;
        self.record_terminal_event(&tables, message.event)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TerminalSupervisorSubscriptionStart {
    Immediate(Response),
    Stream(TerminalSupervisorSubscriptionPlan),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TerminalSupervisorSubscriptionPlan {
    subscription: SubscribeTerminalWorkerLifecycleRequest,
    socket_path: PathBuf,
}

impl TerminalSupervisorSubscriptionPlan {
    pub fn new(
        subscription: SubscribeTerminalWorkerLifecycleRequest,
        socket_path: PathBuf,
    ) -> Self {
        Self {
            subscription,
            socket_path,
        }
    }

    pub fn socket_path(&self) -> &PathBuf {
        &self.socket_path
    }

    pub fn into_subscription(self) -> SubscribeTerminalWorkerLifecycleRequest {
        self.subscription
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSupervisorCommandLine {
    arguments: Vec<OsString>,
}

impl TerminalSupervisorCommandLine {
    pub fn from_environment() -> Self {
        Self::from_arguments(std::env::args_os().skip(1))
    }

    pub fn from_arguments<I, S>(arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            arguments: arguments.into_iter().map(Into::into).collect(),
        }
    }

    pub fn daemon(&self) -> Result<TerminalSupervisorDaemon> {
        TerminalSupervisorArguments::from_arguments(self.arguments.clone()).into_daemon()
    }

    pub fn run(&self) -> Result<()> {
        self.daemon()?.run()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalSupervisorArguments {
    socket: Option<PathBuf>,
    store: StoreLocation,
}

impl TerminalSupervisorArguments {
    fn from_arguments(arguments: impl IntoIterator<Item = OsString>) -> Self {
        let mut socket = None;
        let mut store = None;
        let mut iterator = arguments.into_iter();

        while let Some(argument) = iterator.next() {
            match argument.to_string_lossy().as_ref() {
                "--socket" => socket = iterator.next().map(PathBuf::from),
                "--store" => store = iterator.next().map(StoreLocation::new),
                value if socket.is_none() => socket = Some(PathBuf::from(value)),
                _ => {}
            }
        }

        Self {
            socket,
            store: store.unwrap_or_else(StoreLocation::from_environment),
        }
    }

    fn into_daemon(self) -> Result<TerminalSupervisorDaemon> {
        let socket = self
            .socket
            .or_else(|| std::env::var_os("PERSONA_SOCKET_PATH").map(PathBuf::from))
            .ok_or(Error::MissingSocket {
                component: "terminal-supervisor",
            })?;
        Ok(TerminalSupervisorDaemon::from_socket(socket).with_store(self.store))
    }
}
