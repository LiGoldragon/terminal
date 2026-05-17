use std::ffi::OsString;
use std::io::{BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use kameo::actor::{Actor, ActorRef, Spawn};
use kameo::error::Infallible;
use kameo::message::{Context, Message};
use signal_core::{
    ExchangeIdentifier, ExchangeLane, LaneSequence, NonEmpty, Reply, Request, SessionEpoch,
    SignalVerb, StreamEventIdentifier, SubReply, SubscriptionTokenInner,
};
use signal_persona_terminal::{
    SubscribeTerminalWorkerLifecycle, TerminalDaemonConfiguration,
    TerminalDeliveryAttemptObservation, TerminalEvent, TerminalEventObservation, TerminalFrame,
    TerminalFrameBody as FrameBody, TerminalName, TerminalObservationSequence,
    TerminalOperationKind, TerminalRejected, TerminalRejectionReason, TerminalReply,
    TerminalRequest,
};

use crate::contract::TerminalTransportBinding;
use crate::error::{Error, Result};
use crate::socket::SocketMode;
use crate::supervision::{SupervisionListener, SupervisionProfile, SupervisionSocketMode};
use crate::tables::{StoreLocation, TerminalTables};

fn synthetic_exchange() -> ExchangeIdentifier {
    ExchangeIdentifier::new(
        SessionEpoch::new(0),
        ExchangeLane::Connector,
        LaneSequence::first(),
    )
}

fn synthetic_stream_event() -> StreamEventIdentifier {
    StreamEventIdentifier::new(
        SessionEpoch::new(0),
        ExchangeLane::Acceptor,
        LaneSequence::first(),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSupervisorDaemon {
    socket: PathBuf,
    store: StoreLocation,
    socket_mode: Option<SocketMode>,
    supervision: Option<SupervisionListener>,
}

impl TerminalSupervisorDaemon {
    /// Canonical constructor — every production launch reads typed
    /// `TerminalDaemonConfiguration` from argv via `nota-config` and
    /// hands the record here.
    pub fn from_configuration(configuration: TerminalDaemonConfiguration) -> Self {
        let supervision = SupervisionListener::new(
            SupervisionProfile::terminal(),
            PathBuf::from(configuration.supervision_socket_path.as_str()),
            SupervisionSocketMode::from_octal(configuration.supervision_socket_mode.into_u32()),
        );
        Self {
            socket: PathBuf::from(configuration.terminal_socket_path.as_str()),
            store: StoreLocation::new(configuration.store_path.as_str()),
            socket_mode: Some(SocketMode::from_octal(
                configuration.terminal_socket_mode.into_u32(),
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
        eprintln!(
            "persona-terminal-supervisor socket={}",
            bound.socket.display()
        );
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

    pub fn serve_one(self) -> Result<TerminalReply> {
        self.bind()?.serve_one()
    }

    fn handle_connection(
        runtime: &tokio::runtime::Runtime,
        supervisor: &ActorRef<TerminalSupervisor>,
        stream: UnixStream,
    ) -> Result<TerminalReply> {
        let mut connection = TerminalSupervisorConnection::from_stream(stream);
        let request = connection.read_signal_request()?;
        if let TerminalRequest::SubscribeTerminalWorkerLifecycle(subscription) = request {
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
        connection.write_signal_reply(event.clone())?;
        Ok(event)
    }

    fn handle_subscription(
        runtime: &tokio::runtime::Runtime,
        supervisor: &ActorRef<TerminalSupervisor>,
        mut client: TerminalSupervisorConnection,
        subscription: SubscribeTerminalWorkerLifecycle,
    ) -> Result<TerminalReply> {
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
                client.write_signal_reply(event.clone())?;
                Ok(event)
            }
            TerminalSupervisorSubscriptionStart::Stream(plan) => {
                Self::stream_subscription(runtime, supervisor, client, plan)
            }
        }
    }

    fn stream_subscription(
        runtime: &tokio::runtime::Runtime,
        supervisor: &ActorRef<TerminalSupervisor>,
        mut client: TerminalSupervisorConnection,
        plan: TerminalSupervisorSubscriptionPlan,
    ) -> Result<TerminalReply> {
        let mut cell = BufReader::new(UnixStream::connect(plan.socket_path())?);
        let codec = TerminalSupervisorFrameCodec::default();
        codec.write_request(
            cell.get_mut(),
            TerminalRequest::SubscribeTerminalWorkerLifecycle(plan.into_subscription()),
        )?;

        let mut first = None;
        loop {
            let output = match codec.read_output(&mut cell) {
                Ok(output) => output,
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    break;
                }
                Err(error) => return Err(error),
            };
            match output {
                TerminalSupervisorSignalOutput::Reply(event) => {
                    runtime.block_on(async {
                        supervisor
                            .ask(TerminalSupervisorObservedEvent::new(event.clone()))
                            .await
                            .map_err(|error| Error::ActorCall {
                                detail: error.to_string(),
                            })
                    })?;
                    if first.is_none() {
                        first = Some(event.clone());
                    }
                    client.write_signal_reply(event)?;
                }
                TerminalSupervisorSignalOutput::Event(event) => {
                    client.write_signal_stream_event(event)?;
                }
            }
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

    pub fn serve_one(self) -> Result<TerminalReply> {
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

pub struct TerminalSupervisorConnection {
    stream: BufReader<UnixStream>,
    signal: TerminalSupervisorFrameCodec,
}

impl TerminalSupervisorConnection {
    pub fn from_stream(stream: UnixStream) -> Self {
        Self {
            stream: BufReader::new(stream),
            signal: TerminalSupervisorFrameCodec::default(),
        }
    }

    pub fn read_signal_request(&mut self) -> Result<TerminalRequest> {
        self.signal.read_request(&mut self.stream)
    }

    pub fn write_signal_reply(&mut self, event: TerminalReply) -> Result<()> {
        let stream = self.stream.get_mut();
        self.signal.write_reply(stream, event)
    }

    pub fn write_signal_stream_event(&mut self, event: TerminalEvent) -> Result<()> {
        let stream = self.stream.get_mut();
        self.signal.write_stream_event(stream, event)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSupervisorFrameCodec {
    maximum_frame_bytes: usize,
}

impl TerminalSupervisorFrameCodec {
    pub const fn new(maximum_frame_bytes: usize) -> Self {
        Self {
            maximum_frame_bytes,
        }
    }

    pub fn read_frame(&self, reader: &mut impl Read) -> Result<TerminalFrame> {
        let mut prefix = [0_u8; 4];
        reader.read_exact(&mut prefix)?;
        let length = u32::from_be_bytes(prefix) as usize;
        if length > self.maximum_frame_bytes {
            return Err(Error::UnexpectedSignalFrame {
                got: format!("frame length {length} exceeds {}", self.maximum_frame_bytes),
            });
        }
        let mut bytes = Vec::with_capacity(4 + length);
        bytes.extend_from_slice(&prefix);
        bytes.resize(4 + length, 0);
        reader.read_exact(&mut bytes[4..])?;
        Ok(TerminalFrame::decode_length_prefixed(&bytes)?)
    }

    pub fn read_request(&self, reader: &mut impl Read) -> Result<TerminalRequest> {
        match self.read_frame(reader)?.into_body() {
            FrameBody::Request { request, .. } => request
                .into_checked()
                .map_err(|(reason, _)| Error::InvalidSignalRequest { reason })
                .map(|checked| checked.operations.into_head().payload),
            other => Err(Error::UnexpectedSignalFrame {
                got: format!("{other:?}"),
            }),
        }
    }

    pub fn write_request(&self, writer: &mut impl Write, request: TerminalRequest) -> Result<()> {
        let frame = TerminalFrame::new(FrameBody::Request {
            exchange: synthetic_exchange(),
            request: Request::from_payload(request),
        });
        let bytes = frame.encode_length_prefixed()?;
        writer.write_all(&bytes)?;
        writer.flush()?;
        Ok(())
    }

    pub fn write_event(&self, writer: &mut impl Write, event: TerminalReply) -> Result<()> {
        self.write_reply(writer, event)
    }

    pub fn write_reply(&self, writer: &mut impl Write, event: TerminalReply) -> Result<()> {
        let frame = TerminalFrame::new(FrameBody::Reply {
            exchange: synthetic_exchange(),
            reply: Reply::completed(NonEmpty::single(SubReply::Ok {
                verb: SignalVerb::Subscribe,
                payload: event,
            })),
        });
        let bytes = frame.encode_length_prefixed()?;
        writer.write_all(&bytes)?;
        writer.flush()?;
        Ok(())
    }

    pub fn write_stream_event(&self, writer: &mut impl Write, event: TerminalEvent) -> Result<()> {
        let frame = TerminalFrame::new(FrameBody::SubscriptionEvent {
            event_identifier: synthetic_stream_event(),
            token: SubscriptionTokenInner::new(1),
            event,
        });
        let bytes = frame.encode_length_prefixed()?;
        writer.write_all(&bytes)?;
        writer.flush()?;
        Ok(())
    }

    pub fn read_event(&self, reader: &mut impl Read) -> Result<TerminalReply> {
        self.read_reply(reader)
    }

    pub fn read_reply(&self, reader: &mut impl Read) -> Result<TerminalReply> {
        match self.read_frame(reader)?.into_body() {
            FrameBody::Reply { reply, .. } => match reply {
                Reply::Accepted { per_operation, .. } => match per_operation.into_head() {
                    SubReply::Ok { payload, .. } => Ok(payload),
                    other => Err(Error::UnexpectedSignalFrame {
                        got: format!("{other:?}"),
                    }),
                },
                Reply::Rejected { reason } => Err(Error::UnexpectedSignalFrame {
                    got: format!("{reason:?}"),
                }),
            },
            other => Err(Error::UnexpectedSignalFrame {
                got: format!("{other:?}"),
            }),
        }
    }

    pub fn read_stream_event(&self, reader: &mut impl Read) -> Result<TerminalEvent> {
        match self.read_frame(reader)?.into_body() {
            FrameBody::SubscriptionEvent { event, .. } => Ok(event),
            other => Err(Error::UnexpectedSignalFrame {
                got: format!("{other:?}"),
            }),
        }
    }

    pub fn read_output(&self, reader: &mut impl Read) -> Result<TerminalSupervisorSignalOutput> {
        match self.read_frame(reader)?.into_body() {
            FrameBody::Reply { reply, .. } => match reply {
                Reply::Accepted { per_operation, .. } => match per_operation.into_head() {
                    SubReply::Ok { payload, .. } => {
                        Ok(TerminalSupervisorSignalOutput::Reply(payload))
                    }
                    other => Err(Error::UnexpectedSignalFrame {
                        got: format!("{other:?}"),
                    }),
                },
                Reply::Rejected { reason } => Err(Error::UnexpectedSignalFrame {
                    got: format!("{reason:?}"),
                }),
            },
            FrameBody::SubscriptionEvent { event, .. } => {
                Ok(TerminalSupervisorSignalOutput::Event(event))
            }
            other => Err(Error::UnexpectedSignalFrame {
                got: format!("{other:?}"),
            }),
        }
    }
}

impl Default for TerminalSupervisorFrameCodec {
    fn default() -> Self {
        Self::new(1024 * 1024)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalSupervisorSignalOutput {
    Reply(TerminalReply),
    Event(TerminalEvent),
}

#[derive(Debug, Clone, PartialEq, Eq, kameo::Reply)]
pub struct TerminalSupervisorState {
    pub served_request_count: u64,
    pub recorded_event_count: u64,
    pub last_operation: Option<TerminalOperationKind>,
}

#[derive(Debug)]
pub struct TerminalSupervisor {
    store: StoreLocation,
    served_request_count: u64,
    recorded_event_count: u64,
    last_operation: Option<TerminalOperationKind>,
}

impl TerminalSupervisor {
    pub fn new(store: StoreLocation) -> Self {
        Self {
            store,
            served_request_count: 0,
            recorded_event_count: 0,
            last_operation: None,
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
            recorded_event_count: self.recorded_event_count,
            last_operation: self.last_operation,
        }
    }

    fn event_for_request(
        &mut self,
        sequence: u64,
        request: TerminalRequest,
    ) -> Result<TerminalReply> {
        let terminal = TerminalRequestTerminal::from_request(&request).into_terminal();
        let tables = TerminalTables::open(&self.store)?;
        tables.put_delivery_attempt(&TerminalDeliveryAttemptObservation::started(
            TerminalObservationSequence::new(sequence),
            terminal.clone(),
            request.operation_kind(),
        ))?;
        let Some(session) = tables.session(&terminal)? else {
            let event: TerminalReply = TerminalRejected {
                terminal,
                reason: TerminalRejectionReason::NotConnected,
            }
            .into();
            self.record_terminal_event(&tables, event.clone())?;
            return Ok(event);
        };
        let mut binding = TerminalTransportBinding::from_socket_path(
            terminal,
            session.control_socket_path().as_str(),
        );
        let event = binding.handle_request(request)?;
        self.record_terminal_event(&tables, event.clone())?;
        Ok(event)
    }

    fn subscription_start(
        &mut self,
        sequence: u64,
        subscription: SubscribeTerminalWorkerLifecycle,
    ) -> Result<TerminalSupervisorSubscriptionStart> {
        let terminal = subscription.terminal.clone();
        let tables = TerminalTables::open(&self.store)?;
        tables.put_delivery_attempt(&TerminalDeliveryAttemptObservation::started(
            TerminalObservationSequence::new(sequence),
            terminal.clone(),
            TerminalOperationKind::SubscribeTerminalWorkerLifecycle,
        ))?;
        let Some(session) = tables.session(&terminal)? else {
            let event: TerminalReply = TerminalRejected {
                terminal,
                reason: TerminalRejectionReason::NotConnected,
            }
            .into();
            self.record_terminal_event(&tables, event.clone())?;
            return Ok(TerminalSupervisorSubscriptionStart::Immediate(event));
        };
        Ok(TerminalSupervisorSubscriptionStart::Stream(
            TerminalSupervisorSubscriptionPlan::new(
                subscription,
                PathBuf::from(session.control_socket_path().as_str()),
            ),
        ))
    }

    fn record_terminal_event(
        &mut self,
        tables: &TerminalTables,
        event: TerminalReply,
    ) -> Result<()> {
        self.recorded_event_count = self.recorded_event_count.saturating_add(1);
        tables.put_terminal_event(&TerminalEventObservation::new(
            TerminalObservationSequence::new(self.recorded_event_count),
            TerminalRequestTerminal::from_event(&event).into_terminal(),
            event,
        ))
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
    pub minimum_served_request_count: u64,
}

impl ReadTerminalSupervisorState {
    pub const fn expecting_at_least(minimum_served_request_count: u64) -> Self {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSupervisorRequest {
    request: TerminalRequest,
}

impl TerminalSupervisorRequest {
    pub fn new(request: TerminalRequest) -> Self {
        Self { request }
    }
}

impl Message<TerminalSupervisorRequest> for TerminalSupervisor {
    type Reply = Result<TerminalReply>;

    async fn handle(
        &mut self,
        message: TerminalSupervisorRequest,
        _context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let sequence = self.served_request_count.saturating_add(1);
        self.last_operation = Some(message.request.operation_kind());
        self.served_request_count = sequence;
        self.event_for_request(sequence, message.request)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSupervisorSubscriptionRequest {
    subscription: SubscribeTerminalWorkerLifecycle,
}

impl TerminalSupervisorSubscriptionRequest {
    pub fn new(subscription: SubscribeTerminalWorkerLifecycle) -> Self {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSupervisorObservedEvent {
    event: TerminalReply,
}

impl TerminalSupervisorObservedEvent {
    pub fn new(event: TerminalReply) -> Self {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalSupervisorSubscriptionStart {
    Immediate(TerminalReply),
    Stream(TerminalSupervisorSubscriptionPlan),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSupervisorSubscriptionPlan {
    subscription: SubscribeTerminalWorkerLifecycle,
    socket_path: PathBuf,
}

impl TerminalSupervisorSubscriptionPlan {
    pub fn new(subscription: SubscribeTerminalWorkerLifecycle, socket_path: PathBuf) -> Self {
        Self {
            subscription,
            socket_path,
        }
    }

    pub fn socket_path(&self) -> &PathBuf {
        &self.socket_path
    }

    pub fn into_subscription(self) -> SubscribeTerminalWorkerLifecycle {
        self.subscription
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalRequestTerminal {
    terminal: TerminalName,
}

impl TerminalRequestTerminal {
    fn from_request(request: &TerminalRequest) -> Self {
        let terminal = match request {
            TerminalRequest::TerminalConnection(payload) => payload.terminal.clone(),
            TerminalRequest::TerminalInput(payload) => payload.terminal.clone(),
            TerminalRequest::TerminalResize(payload) => payload.terminal.clone(),
            TerminalRequest::TerminalDetachment(payload) => payload.terminal.clone(),
            TerminalRequest::TerminalCapture(payload) => payload.terminal.clone(),
            TerminalRequest::RegisterPromptPattern(payload) => payload.terminal.clone(),
            TerminalRequest::UnregisterPromptPattern(payload) => payload.terminal.clone(),
            TerminalRequest::ListPromptPatterns(payload) => payload.terminal.clone(),
            TerminalRequest::AcquireInputGate(payload) => payload.terminal.clone(),
            TerminalRequest::ReleaseInputGate(payload) => payload.terminal.clone(),
            TerminalRequest::WriteInjection(payload) => payload.terminal.clone(),
            TerminalRequest::SubscribeTerminalWorkerLifecycle(payload) => payload.terminal.clone(),
            TerminalRequest::TerminalWorkerLifecycleRetraction(payload) => payload.terminal.clone(),
        };
        Self { terminal }
    }

    fn from_event(event: &TerminalReply) -> Self {
        let terminal = match event {
            TerminalReply::TerminalReady(payload) => payload.terminal.clone(),
            TerminalReply::TerminalInputAccepted(payload) => payload.terminal.clone(),
            TerminalReply::TranscriptDelta(payload) => payload.terminal.clone(),
            TerminalReply::TerminalResized(payload) => payload.terminal.clone(),
            TerminalReply::TerminalCaptured(payload) => payload.terminal.clone(),
            TerminalReply::TerminalDetached(payload) => payload.terminal.clone(),
            TerminalReply::TerminalExited(payload) => payload.terminal.clone(),
            TerminalReply::TerminalRejected(payload) => payload.terminal.clone(),
            TerminalReply::PromptPatternRegistered(payload) => payload.terminal.clone(),
            TerminalReply::PromptPatternUnregistered(payload) => payload.terminal.clone(),
            TerminalReply::PromptPatternList(payload) => payload.terminal.clone(),
            TerminalReply::GateAcquired(payload) => payload.terminal.clone(),
            TerminalReply::GateBusy(payload) => payload.terminal.clone(),
            TerminalReply::GateReleased(payload) => payload.terminal.clone(),
            TerminalReply::InjectionAck(payload) => payload.terminal.clone(),
            TerminalReply::InjectionRejected(payload) => payload.terminal.clone(),
            TerminalReply::TerminalWorkerLifecycleSnapshot(payload) => payload.terminal.clone(),
            TerminalReply::SubscriptionRetracted(payload) => payload.token.terminal.clone(),
            // TerminalWorkerLifecycleEvent now belongs to TerminalEvent
            // (the streaming-event payload); routed via
            // StreamingFrameBody::SubscriptionEvent, not Reply.
        };
        Self { terminal }
    }

    fn into_terminal(self) -> TerminalName {
        self.terminal
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
                component: "persona-terminal-supervisor",
            })?;
        Ok(TerminalSupervisorDaemon::from_socket(socket).with_store(self.store))
    }
}
