pub mod capture_validator;
pub mod command;
pub mod config;
pub mod contract;
pub mod daemon;
pub mod error;
pub mod pty;
pub mod registry;
pub mod signal_cli;
pub mod signal_control;
pub mod socket;
pub mod supervision;
pub mod supervisor;
pub mod tables;

pub mod schema {
    #[rustfmt::skip]
    pub mod signal;
    #[rustfmt::skip]
    pub mod sema;
    #[rustfmt::skip]
    pub mod nexus;
    #[rustfmt::skip]
    pub mod daemon;
}

pub use command::{TerminalDaemonConfigurationFile, TerminalSupervisorDaemonCommand};
pub use config::{Configuration, ConfigurationError};
pub use daemon::{TerminalDaemonError, TerminalEngine, TerminalProcessDaemon};
pub use error::{Error, Result};
pub use schema::daemon::{ComponentDaemon, DaemonCommand, DaemonEntry, DaemonError, ListenerTier};
pub use socket::SocketMode;
pub use supervision::{
    SupervisionFrameCodec, SupervisionListener, SupervisionProfile, SupervisionSocketMode,
};
pub use terminal_cell as cell;
