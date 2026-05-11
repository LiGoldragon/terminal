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

    #[error("invalid argument: {detail}")]
    InvalidArgument { detail: String },

    #[error("unknown terminal session: {terminal}")]
    UnknownTerminalSession { terminal: String },

    #[error("terminal cell: {detail}")]
    TerminalCell { detail: String },

    #[error("missing command for PTY daemon")]
    MissingCommand,

    #[error("PTY socket {path:?} did not become ready")]
    SocketNotReady { path: PathBuf },
}

pub type Result<T> = std::result::Result<T, Error>;
