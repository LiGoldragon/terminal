use kameo::actor::ActorRef;
use meta_signal_terminal::{
    MetaTerminalFrame, MetaTerminalFrameBody, MetaTerminalReply, MetaTerminalRequest,
};
use signal_frame::{
    ExchangeIdentifier, ExchangeLane, LaneSequence, NonEmpty, Reply, Request, SessionEpoch,
    SubReply,
};
use signal_terminal::{
    Frame, FrameBody, Input, Output, SubscribeTerminalWorkerLifecycle, TerminalEvent,
};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::sync::OnceCell;
use triad_runtime::{
    AcceptedConnection, FrameBody as LengthPrefixedFrameBody, FrameError, LengthPrefixedCodec,
};

use crate::{
    Configuration, ConfigurationError, Error as TerminalError, Result as TerminalResult,
    schema::daemon::ComponentDaemon,
    supervisor::{
        TerminalSupervisor, TerminalSupervisorMetaRequest, TerminalSupervisorObservedEvent,
        TerminalSupervisorRequest, TerminalSupervisorSubscriptionRequest,
        TerminalSupervisorSubscriptionStart,
    },
    tables::StoreLocation,
};

#[derive(Debug)]
pub struct TerminalProcessDaemon;

pub struct TerminalEngine {
    store: StoreLocation,
    supervisor: OnceCell<ActorRef<TerminalSupervisor>>,
}

#[derive(Debug, Error)]
pub enum TerminalDaemonError {
    #[error("daemon IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("daemon frame error: {0}")]
    Frame(#[from] FrameError),

    #[error("daemon signal frame error: {0}")]
    SignalFrame(#[from] signal_frame::FrameError),

    #[error("daemon terminal error: {0}")]
    Terminal(#[from] TerminalError),
}

impl TerminalEngine {
    pub fn from_configuration(configuration: &Configuration) -> TerminalResult<Self> {
        let _supervision = configuration.supervision_listener().spawn()?;
        Ok(Self {
            store: configuration.store_location(),
            supervisor: OnceCell::new(),
        })
    }

    async fn supervisor(&self) -> Result<&ActorRef<TerminalSupervisor>, TerminalDaemonError> {
        self.supervisor
            .get_or_try_init(|| async { Ok(TerminalSupervisor::start(self.store.clone()).await) })
            .await
    }

    async fn handle_working_connection(
        &self,
        mut connection: AcceptedConnection,
    ) -> Result<(), TerminalDaemonError> {
        let body = LengthPrefixedCodec::default()
            .read_body_async(connection.stream_mut())
            .await?;
        let request = TerminalSignalRequest::decode(body.bytes())?;
        let exchange = request.exchange();
        match request.into_request() {
            Input::SubscribeTerminalWorkerLifecycle(subscription) => {
                self.handle_subscription(connection, exchange, subscription)
                    .await?;
            }
            request_payload => {
                let reply = self
                    .supervisor()
                    .await?
                    .ask(TerminalSupervisorRequest::new(request_payload))
                    .await
                    .map_err(TerminalActorCall::from_error)?;
                TerminalSignalReply::new(exchange, reply)
                    .write(connection.stream_mut())
                    .await?;
            }
        }
        Ok(())
    }

    async fn handle_subscription(
        &self,
        mut connection: AcceptedConnection,
        exchange: ExchangeIdentifier,
        subscription: SubscribeTerminalWorkerLifecycle,
    ) -> Result<(), TerminalDaemonError> {
        let start = self
            .supervisor()
            .await?
            .ask(TerminalSupervisorSubscriptionRequest::new(subscription))
            .await
            .map_err(TerminalActorCall::from_error)?;
        match start {
            TerminalSupervisorSubscriptionStart::Immediate(reply) => {
                TerminalSignalReply::new(exchange, reply)
                    .write(connection.stream_mut())
                    .await?;
            }
            TerminalSupervisorSubscriptionStart::Stream(plan) => {
                TerminalSubscriptionRelay::new(
                    self.supervisor().await?.clone(),
                    exchange,
                    connection,
                    plan.socket_path().clone(),
                    plan.into_subscription(),
                )
                .run()
                .await?;
            }
        }
        Ok(())
    }

    async fn handle_meta_connection(
        &self,
        mut connection: AcceptedConnection,
    ) -> Result<(), TerminalDaemonError> {
        let body = LengthPrefixedCodec::default()
            .read_body_async(connection.stream_mut())
            .await?;
        let request = MetaTerminalSignalRequest::decode(body.bytes())?;
        let reply = self
            .supervisor()
            .await?
            .ask(TerminalSupervisorMetaRequest::new(
                request.request().clone(),
            ))
            .await
            .map_err(TerminalActorCall::from_error)?
            .into_reply();
        MetaTerminalSignalReply::new(request.exchange(), reply)
            .write(connection.stream_mut())
            .await?;
        Ok(())
    }
}

impl ComponentDaemon for TerminalProcessDaemon {
    type Configuration = Configuration;
    type ConfigurationError = ConfigurationError;
    type Engine = TerminalEngine;
    type Error = TerminalDaemonError;

    const PROCESS_NAME: &'static str = "terminal-supervisor";

    fn load_configuration(
        path: &std::path::Path,
    ) -> Result<Self::Configuration, Self::ConfigurationError> {
        Configuration::from_binary_path(path)
    }

    fn build_runtime(configuration: &Self::Configuration) -> Result<Self::Engine, Self::Error> {
        Ok(TerminalEngine::from_configuration(configuration)?)
    }

    async fn handle_working_connection(
        engine: &Self::Engine,
        connection: AcceptedConnection,
    ) -> Result<(), Self::Error> {
        engine.handle_working_connection(connection).await
    }

    async fn handle_meta_connection(
        engine: &Self::Engine,
        connection: AcceptedConnection,
    ) -> Result<(), Self::Error> {
        engine.handle_meta_connection(connection).await
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalSignalRequest {
    exchange: ExchangeIdentifier,
    request: Input,
}

impl TerminalSignalRequest {
    fn decode(body: &[u8]) -> Result<Self, signal_frame::FrameError> {
        match Frame::decode(body)?.into_body() {
            FrameBody::Request { exchange, request } => {
                Ok(Self::new(exchange, Self::single_payload(request)?))
            }
            _ => Err(signal_frame::FrameError::ArchiveDeserialize),
        }
    }

    fn new(exchange: ExchangeIdentifier, request: Input) -> Self {
        Self { exchange, request }
    }

    fn exchange(&self) -> ExchangeIdentifier {
        self.exchange
    }

    fn into_request(self) -> Input {
        self.request
    }

    fn single_payload(request: Request<Input>) -> Result<Input, signal_frame::FrameError> {
        let (request, tail) = request.payloads.into_head_and_tail();
        if tail.is_empty() {
            Ok(request)
        } else {
            Err(signal_frame::FrameError::ArchiveDeserialize)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalSignalReply {
    exchange: ExchangeIdentifier,
    reply: Output,
}

impl TerminalSignalReply {
    fn new(exchange: ExchangeIdentifier, reply: Output) -> Self {
        Self { exchange, reply }
    }

    async fn write(self, stream: &mut tokio::net::UnixStream) -> Result<(), TerminalDaemonError> {
        let frame = Frame::new(FrameBody::Reply {
            exchange: self.exchange,
            reply: Reply::committed(NonEmpty::single(SubReply::Ok(self.reply))),
        });
        LengthPrefixedCodec::default()
            .write_body_async(stream, &LengthPrefixedFrameBody::new(frame.encode()?))
            .await?;
        stream.flush().await.map_err(FrameError::from)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MetaTerminalSignalRequest {
    exchange: ExchangeIdentifier,
    request: MetaTerminalRequest,
}

impl MetaTerminalSignalRequest {
    fn decode(body: &[u8]) -> Result<Self, signal_frame::FrameError> {
        match MetaTerminalFrame::decode(body)?.into_body() {
            MetaTerminalFrameBody::Request { exchange, request } => {
                Ok(Self::new(exchange, Self::single_payload(request)?))
            }
            _ => Err(signal_frame::FrameError::ArchiveDeserialize),
        }
    }

    fn new(exchange: ExchangeIdentifier, request: MetaTerminalRequest) -> Self {
        Self { exchange, request }
    }

    fn exchange(&self) -> ExchangeIdentifier {
        self.exchange
    }

    fn request(&self) -> &MetaTerminalRequest {
        &self.request
    }

    fn single_payload(
        request: Request<MetaTerminalRequest>,
    ) -> Result<MetaTerminalRequest, signal_frame::FrameError> {
        let (request, tail) = request.payloads.into_head_and_tail();
        if tail.is_empty() {
            Ok(request)
        } else {
            Err(signal_frame::FrameError::ArchiveDeserialize)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MetaTerminalSignalReply {
    exchange: ExchangeIdentifier,
    reply: MetaTerminalReply,
}

impl MetaTerminalSignalReply {
    fn new(exchange: ExchangeIdentifier, reply: MetaTerminalReply) -> Self {
        Self { exchange, reply }
    }

    async fn write(self, stream: &mut tokio::net::UnixStream) -> Result<(), TerminalDaemonError> {
        let frame = MetaTerminalFrame::new(MetaTerminalFrameBody::Reply {
            exchange: self.exchange,
            reply: Reply::committed(NonEmpty::single(SubReply::Ok(self.reply))),
        });
        LengthPrefixedCodec::default()
            .write_body_async(stream, &LengthPrefixedFrameBody::new(frame.encode()?))
            .await?;
        stream.flush().await.map_err(FrameError::from)?;
        Ok(())
    }
}

struct TerminalSubscriptionRelay {
    supervisor: ActorRef<TerminalSupervisor>,
    exchange: ExchangeIdentifier,
    client: AcceptedConnection,
    cell_socket_path: std::path::PathBuf,
    subscription: SubscribeTerminalWorkerLifecycle,
}

impl TerminalSubscriptionRelay {
    fn new(
        supervisor: ActorRef<TerminalSupervisor>,
        exchange: ExchangeIdentifier,
        client: AcceptedConnection,
        cell_socket_path: std::path::PathBuf,
        subscription: SubscribeTerminalWorkerLifecycle,
    ) -> Self {
        Self {
            supervisor,
            exchange,
            client,
            cell_socket_path,
            subscription,
        }
    }

    async fn run(mut self) -> Result<(), TerminalDaemonError> {
        let mut cell = tokio::net::UnixStream::connect(&self.cell_socket_path).await?;
        TerminalSignalRequestFrame::new(self.subscription.clone())
            .write(&mut cell)
            .await?;
        let mut first_reply_seen = false;
        loop {
            match TerminalSignalOutput::read_from(&mut cell).await {
                Ok(TerminalSignalOutput::Reply(reply)) => {
                    self.record_reply(reply.clone()).await?;
                    first_reply_seen = true;
                    TerminalSignalReply::new(self.exchange, reply)
                        .write(self.client.stream_mut())
                        .await?;
                }
                Ok(TerminalSignalOutput::Event(event)) => {
                    event.write(self.client.stream_mut()).await?;
                }
                Err(TerminalDaemonError::Frame(FrameError::Io(error)))
                    if error.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        if first_reply_seen {
            Ok(())
        } else {
            Err(TerminalError::UnexpectedSignalFrame {
                got: "subscription ended before initial state".to_string(),
            }
            .into())
        }
    }

    async fn record_reply(&self, reply: Output) -> Result<(), TerminalDaemonError> {
        self.supervisor
            .ask(TerminalSupervisorObservedEvent::new(reply))
            .await
            .map_err(TerminalActorCall::from_error)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalSignalRequestFrame {
    request: Input,
}

impl TerminalSignalRequestFrame {
    fn new(subscription: SubscribeTerminalWorkerLifecycle) -> Self {
        Self {
            request: Input::SubscribeTerminalWorkerLifecycle(subscription),
        }
    }

    async fn write(self, stream: &mut tokio::net::UnixStream) -> Result<(), TerminalDaemonError> {
        let frame = Frame::new(FrameBody::Request {
            exchange: TerminalSyntheticExchange::new().into_exchange(),
            request: Request::from_payload(self.request),
        });
        LengthPrefixedCodec::default()
            .write_body_async(stream, &LengthPrefixedFrameBody::new(frame.encode()?))
            .await?;
        stream.flush().await.map_err(FrameError::from)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TerminalSignalOutput {
    Reply(Output),
    Event(TerminalSignalEvent),
}

impl TerminalSignalOutput {
    async fn read_from(stream: &mut tokio::net::UnixStream) -> Result<Self, TerminalDaemonError> {
        let body = LengthPrefixedCodec::default()
            .read_body_async(stream)
            .await?;
        match Frame::decode(body.bytes())?.into_body() {
            FrameBody::Reply { reply, .. } => match reply {
                Reply::Accepted { per_operation, .. } => match per_operation.into_head() {
                    SubReply::Ok(reply) => Ok(Self::Reply(reply)),
                    _ => Err(TerminalError::UnexpectedSignalFrame {
                        got: "non-accepted terminal subscription reply".to_string(),
                    }
                    .into()),
                },
                Reply::Rejected { reason } => Err(TerminalError::UnexpectedSignalFrame {
                    got: format!("{reason:?}"),
                }
                .into()),
            },
            FrameBody::SubscriptionEvent {
                event_identifier,
                token,
                event,
            } => Ok(Self::Event(TerminalSignalEvent {
                event_identifier,
                token,
                event,
            })),
            other => Err(TerminalError::UnexpectedSignalFrame {
                got: format!("{other:?}"),
            }
            .into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalSignalEvent {
    event_identifier: signal_frame::StreamEventIdentifier,
    token: signal_frame::SubscriptionTokenInner,
    event: TerminalEvent,
}

impl TerminalSignalEvent {
    async fn write(self, stream: &mut tokio::net::UnixStream) -> Result<(), TerminalDaemonError> {
        let frame = Frame::new(FrameBody::SubscriptionEvent {
            event_identifier: self.event_identifier,
            token: self.token,
            event: self.event,
        });
        LengthPrefixedCodec::default()
            .write_body_async(stream, &LengthPrefixedFrameBody::new(frame.encode()?))
            .await?;
        stream.flush().await.map_err(FrameError::from)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TerminalSyntheticExchange {
    exchange: ExchangeIdentifier,
}

impl TerminalSyntheticExchange {
    fn new() -> Self {
        Self {
            exchange: ExchangeIdentifier::new(
                SessionEpoch::new(0),
                ExchangeLane::Connector,
                LaneSequence::first(),
            ),
        }
    }

    fn into_exchange(self) -> ExchangeIdentifier {
        self.exchange
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalActorCall {
    detail: String,
}

impl TerminalActorCall {
    fn from_error(error: impl std::fmt::Display) -> Self {
        Self {
            detail: error.to_string(),
        }
    }

    fn into_terminal_error(self) -> TerminalError {
        TerminalError::ActorCall {
            detail: self.detail,
        }
    }
}

impl From<TerminalActorCall> for TerminalDaemonError {
    fn from(call: TerminalActorCall) -> Self {
        Self::Terminal(call.into_terminal_error())
    }
}
