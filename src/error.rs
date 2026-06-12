use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("sema engine: {0}")]
    SemaEngine(#[from] sema_engine::Error),

    #[error("signal frame: {0}")]
    SignalFrame(#[from] signal_frame::FrameError),

    #[error("triad runtime frame: {0}")]
    TriadRuntimeFrame(#[from] triad_runtime::FrameError),

    #[error("daemon argument: {0}")]
    Argument(#[from] triad_runtime::ArgumentError),

    #[cfg(feature = "nota-text")]
    #[error("nota decode: {0}")]
    Nota(#[from] nota_next::NotaDecodeError),

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
        reason: signal_frame::RequestRejectionReason,
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

    #[cfg(feature = "nota-text")]
    #[error("failed to read terminal NOTA input {path:?}: {source}")]
    NotaFileRead {
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
