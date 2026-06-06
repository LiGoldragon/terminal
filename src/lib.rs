extern crate self as terminal;

pub mod capture_validator;
pub mod contract;
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
    #[allow(dead_code)]
    pub mod daemon;
}

pub use error::{Error, Result};
pub use socket::SocketMode;
pub use supervision::{
    SupervisionFrameCodec, SupervisionListener, SupervisionProfile, SupervisionSocketMode,
};
pub use terminal_cell as cell;
