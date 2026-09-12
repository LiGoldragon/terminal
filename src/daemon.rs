use kameo::actor::ActorRef;
use signal_terminal::{Query, Response, SubscribeTerminalWorkerLifecycleRequest};
use thiserror::Error;
use tokio::sync::OnceCell;
use triad_runtime::{AcceptedConnection, FrameError};

use crate::{
    Configuration, ConfigurationError, Error as TerminalError, Result as TerminalResult, frame,
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
        let request = frame::terminal::read_query_async(connection.stream_mut()).await?;
        match request {
            Query::SubscribeTerminalWorkerLifecycle(subscription) => {
                self.handle_subscription(connection, subscription).await?;
            }
            request_payload => {
                let reply = self
                    .supervisor()
                    .await?
                    .ask(TerminalSupervisorRequest::new(request_payload))
                    .await
                    .map_err(actor_call)?;
                frame::terminal::write_response_async(connection.stream_mut(), &reply).await?;
            }
        }
        Ok(())
    }

    async fn handle_subscription(
        &self,
        mut connection: AcceptedConnection,
        subscription: SubscribeTerminalWorkerLifecycleRequest,
    ) -> Result<(), TerminalDaemonError> {
        let start = self
            .supervisor()
            .await?
            .ask(TerminalSupervisorSubscriptionRequest::new(subscription))
            .await
            .map_err(actor_call)?;
        match start {
            TerminalSupervisorSubscriptionStart::Immediate(reply) => {
                frame::terminal::write_response_async(connection.stream_mut(), &reply).await?;
            }
            TerminalSupervisorSubscriptionStart::Stream(plan) => {
                TerminalSubscriptionRelay::new(
                    self.supervisor().await?.clone(),
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
        let request = frame::meta::read_query_async(connection.stream_mut()).await?;
        let reply = self
            .supervisor()
            .await?
            .ask(TerminalSupervisorMetaRequest::new(request))
            .await
            .map_err(actor_call)?
            .into_reply();
        frame::meta::write_response_async(connection.stream_mut(), &reply).await?;
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

/// Relays one cell's lifecycle subscription to the client that asked for it.
///
/// The cell sends its initial snapshot and then every later event on the
/// same `Response` frame shape, so the relay records the first and passes
/// each one through until the cell closes the connection.
struct TerminalSubscriptionRelay {
    supervisor: ActorRef<TerminalSupervisor>,
    client: AcceptedConnection,
    cell_socket_path: std::path::PathBuf,
    subscription: SubscribeTerminalWorkerLifecycleRequest,
}

impl TerminalSubscriptionRelay {
    fn new(
        supervisor: ActorRef<TerminalSupervisor>,
        client: AcceptedConnection,
        cell_socket_path: std::path::PathBuf,
        subscription: SubscribeTerminalWorkerLifecycleRequest,
    ) -> Self {
        Self {
            supervisor,
            client,
            cell_socket_path,
            subscription,
        }
    }

    async fn run(mut self) -> Result<(), TerminalDaemonError> {
        let mut cell = tokio::net::UnixStream::connect(&self.cell_socket_path).await?;
        frame::terminal::write_query_async(
            &mut cell,
            &Query::SubscribeTerminalWorkerLifecycle(self.subscription.clone()),
        )
        .await?;
        let mut first_reply_seen = false;
        loop {
            match frame::terminal::read_response_async(&mut cell).await {
                Ok(response) => {
                    if !first_reply_seen {
                        self.record_reply(response.clone()).await?;
                        first_reply_seen = true;
                    }
                    frame::terminal::write_response_async(self.client.stream_mut(), &response)
                        .await?;
                }
                Err(TerminalError::Io(error))
                    if error.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    break;
                }
                Err(error) => return Err(error.into()),
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

    async fn record_reply(&self, reply: Response) -> Result<(), TerminalDaemonError> {
        self.supervisor
            .ask(TerminalSupervisorObservedEvent::new(reply))
            .await
            .map_err(actor_call)?;
        Ok(())
    }
}

fn actor_call(error: impl std::fmt::Display) -> TerminalDaemonError {
    TerminalDaemonError::Terminal(TerminalError::ActorCall {
        detail: error.to_string(),
    })
}
