use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("sema: {0}")]
    Sema(#[from] sema::Error),

    #[error("signal frame: {0}")]
    SignalFrame(#[from] signal_core::FrameError),

    #[error("daemon argument: {0}")]
    Argument(#[from] triad_runtime::ArgumentError),

    #[error("actor call: {detail}")]
    ActorCall { detail: String },

    #[error("invalid argument: {detail}")]
    InvalidArgument { detail: String },

    #[error("artifact validation failed: {detail}")]
    ArtifactValidation { detail: String },

    #[error("{component} socket path is missing")]
    MissingSocket { component: &'static str },

    #[error("unexpected signal frame: {got}")]
    UnexpectedSignalFrame { got: String },

    #[error("signal request failed structural checks: {reason}")]
    InvalidSignalRequest {
        reason: signal_core::RequestRejectionReason,
    },

    #[error("unknown terminal session: {terminal}")]
    UnknownTerminalSession { terminal: String },

    #[error("terminal cell: {detail}")]
    TerminalCell { detail: String },

    #[error("missing command for PTY daemon")]
    MissingCommand,

    #[error("PTY socket {path:?} did not become ready")]
    SocketNotReady { path: PathBuf },

    #[error("failed to read terminal daemon configuration {path:?}: {source}")]
    ConfigurationRead {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to write terminal daemon configuration {path:?}: {source}")]
    ConfigurationWrite {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to decode terminal daemon configuration archive")]
    ConfigurationArchiveDecode,

    #[error("failed to encode terminal daemon configuration archive")]
    ConfigurationArchiveEncode,
}

pub type Result<T> = std::result::Result<T, Error>;
