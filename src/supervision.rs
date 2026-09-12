use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::thread::JoinHandle;

use kameo::actor::{Actor, ActorRef, Spawn};
use kameo::error::Infallible;
use kameo::message::{Context, Message};
use signal_persona::{
    ComponentHealth, ComponentIdentity, ComponentKind, ComponentName, LifecycleQuery,
    Query as SupervisionQuery, Response as SupervisionResponse,
};

use crate::Result;
use crate::frame;

/// The identity and health this component announces on its supervision
/// socket.
#[derive(Debug, Clone, PartialEq)]
pub struct SupervisionProfile {
    name: ComponentName,
    kind: ComponentKind,
    health: ComponentHealth,
}

impl SupervisionProfile {
    pub fn terminal() -> Self {
        Self {
            name: "terminal".to_string(),
            kind: ComponentKind::Terminal,
            health: ComponentHealth::Running,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupervisionSocketMode(u32);

impl SupervisionSocketMode {
    pub const fn from_octal(value: u32) -> Self {
        Self(value)
    }

    pub const fn as_octal(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SupervisionListener {
    profile: SupervisionProfile,
    socket: PathBuf,
    mode: SupervisionSocketMode,
}

impl SupervisionListener {
    pub fn new(
        profile: SupervisionProfile,
        socket: impl Into<PathBuf>,
        mode: SupervisionSocketMode,
    ) -> Self {
        Self {
            profile,
            socket: socket.into(),
            mode,
        }
    }

    pub fn spawn(self) -> std::io::Result<SupervisionHandle> {
        if let Some(parent) = self.socket.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _ = std::fs::remove_file(&self.socket);
        let listener = UnixListener::bind(&self.socket)?;
        std::fs::set_permissions(
            &self.socket,
            std::fs::Permissions::from_mode(self.mode.as_octal()),
        )?;
        let server = SupervisionServer::new(self.profile, listener);
        Ok(SupervisionHandle {
            _thread: std::thread::spawn(move || server.run()),
        })
    }
}

pub struct SupervisionHandle {
    _thread: JoinHandle<()>,
}

#[derive(Debug)]
pub struct SupervisionPhase {
    profile: SupervisionProfile,
    request_count: u64,
}

impl SupervisionPhase {
    fn new(profile: SupervisionProfile) -> Self {
        Self {
            profile,
            request_count: 0,
        }
    }

    async fn start(profile: SupervisionProfile) -> ActorRef<Self> {
        let reference = Self::spawn(Self::new(profile));
        reference.wait_for_startup().await;
        reference
    }

    fn reply(&mut self, request: SupervisionQuery) -> SupervisionResponse {
        self.request_count = self.request_count.saturating_add(1);
        match request {
            SupervisionQuery::Announce(_) => SupervisionResponse::Identified(ComponentIdentity {
                component_name: self.profile.name.clone(),
                component_kind: self.profile.kind.clone(),
                engine_management_protocol_version: 1,
                component_startup_error_option: None,
            }),
            SupervisionQuery::Query(query) => match query {
                LifecycleQuery::ReadinessStatus(_) => SupervisionResponse::Ready(None),
                LifecycleQuery::HealthStatus(_) => {
                    SupervisionResponse::HealthReport(self.profile.health.clone())
                }
            },
            SupervisionQuery::Stop(_) => SupervisionResponse::StopAcknowledged(None),
        }
    }
}

#[derive(Debug, kameo::Reply)]
struct SupervisionPhaseReply {
    reply: SupervisionResponse,
}

impl Actor for SupervisionPhase {
    type Args = Self;
    type Error = Infallible;

    async fn on_start(
        phase: Self::Args,
        _actor_reference: ActorRef<Self>,
    ) -> std::result::Result<Self, Self::Error> {
        Ok(phase)
    }
}

#[derive(Debug)]
struct HandleSupervisionRequest {
    request: SupervisionQuery,
}

impl Message<HandleSupervisionRequest> for SupervisionPhase {
    type Reply = SupervisionPhaseReply;

    async fn handle(
        &mut self,
        message: HandleSupervisionRequest,
        _context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        SupervisionPhaseReply {
            reply: self.reply(message.request),
        }
    }
}

struct SupervisionServer {
    profile: SupervisionProfile,
    listener: UnixListener,
}

impl SupervisionServer {
    fn new(profile: SupervisionProfile, listener: UnixListener) -> Self {
        Self { profile, listener }
    }

    fn run(self) {
        let runtime = tokio::runtime::Runtime::new().expect("supervision runtime starts");
        let phase = runtime.block_on(SupervisionPhase::start(self.profile.clone()));
        for incoming in self.listener.incoming() {
            let Ok(mut stream) = incoming else {
                continue;
            };
            let _ = Self::serve_connection(&runtime, &phase, &mut stream);
        }
    }

    fn serve_connection(
        runtime: &tokio::runtime::Runtime,
        phase: &ActorRef<SupervisionPhase>,
        stream: &mut UnixStream,
    ) -> Result<()> {
        while let Ok(request) = frame::persona::read_query(stream) {
            let reply = runtime
                .block_on(phase.ask(HandleSupervisionRequest { request }).send())
                .map_err(|error| crate::Error::ActorCall {
                    detail: error.to_string(),
                })?;
            frame::persona::write_response(stream, &reply.reply)?;
        }
        Ok(())
    }
}

/// The `signal-persona` frame pair, as the supervision plane uses it.
///
/// A thin naming over [`crate::frame::persona`], kept because the
/// supervision witnesses address it by name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SupervisionFrameCodec;

impl SupervisionFrameCodec {
    pub const fn new() -> Self {
        Self
    }

    pub fn read_reply(&self, reader: &mut impl Read) -> Result<SupervisionResponse> {
        frame::persona::read_response(reader)
    }

    pub fn write_request(&self, writer: &mut impl Write, request: &SupervisionQuery) -> Result<()> {
        frame::persona::write_query(writer, request)
    }
}
