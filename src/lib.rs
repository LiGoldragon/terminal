pub mod contract;
pub mod datom_text;
pub mod error;
pub mod frame;
pub mod operation;
pub mod pty;
pub mod records;
pub mod registry;
pub mod signal_control;
pub mod socket;
pub mod tables;

pub mod schema {
    #[rustfmt::skip]
    pub mod daemon;
}

pub use error::{Error, Result};
pub use socket::SocketMode;
