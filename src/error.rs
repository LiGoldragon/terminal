use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("terminal cell: {detail}")]
    TerminalCell { detail: String },

    #[error("missing command for PTY daemon")]
    MissingCommand,

    #[error("PTY socket {path:?} did not become ready")]
    SocketNotReady { path: PathBuf },
}

pub type Result<T> = std::result::Result<T, Error>;
