use std::path::{Path, PathBuf};

use signal_terminal::TerminalDaemonConfiguration;
use thiserror::Error;
use triad_runtime::{
    DaemonConfiguration, RequestConcurrencyLimit, SocketMode as RuntimeSocketMode,
};

use crate::{
    SupervisionListener, SupervisionProfile, SupervisionSocketMode, tables::StoreLocation,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Configuration {
    raw: TerminalDaemonConfiguration,
    socket_path: PathBuf,
    meta_socket_path: PathBuf,
    supervision_socket_path: PathBuf,
    database_path: PathBuf,
}

#[derive(Debug, Error)]
pub enum ConfigurationError {
    #[error("failed to read terminal daemon configuration {path:?}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to decode terminal daemon configuration archive {path:?}")]
    Decode { path: PathBuf },
}

impl Configuration {
    pub fn from_raw(raw: TerminalDaemonConfiguration) -> Self {
        Self {
            socket_path: PathBuf::from(raw.terminal_socket_path.as_str()),
            meta_socket_path: PathBuf::from(raw.meta_terminal_socket_path.as_str()),
            supervision_socket_path: PathBuf::from(raw.supervision_socket_path.as_str()),
            database_path: PathBuf::from(raw.store_path.as_str()),
            raw,
        }
    }

    pub fn from_binary_path(path: &Path) -> Result<Self, ConfigurationError> {
        let bytes = std::fs::read(path).map_err(|source| ConfigurationError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let raw = TerminalDaemonConfiguration::from_rkyv_bytes(&bytes).map_err(|_| {
            ConfigurationError::Decode {
                path: path.to_path_buf(),
            }
        })?;
        Ok(Self::from_raw(raw))
    }

    pub fn raw(&self) -> &TerminalDaemonConfiguration {
        &self.raw
    }

    pub fn store_location(&self) -> StoreLocation {
        StoreLocation::new(self.database_path.clone())
    }

    pub fn supervision_listener(&self) -> SupervisionListener {
        SupervisionListener::new(
            SupervisionProfile::terminal(),
            self.supervision_socket_path.clone(),
            SupervisionSocketMode::from_octal(self.raw.supervision_socket_mode.clone().into_u32()),
        )
    }
}

impl DaemonConfiguration for Configuration {
    fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    fn socket_mode(&self) -> Option<RuntimeSocketMode> {
        Some(RuntimeSocketMode::new(
            self.raw.terminal_socket_mode.clone().into_u32(),
        ))
    }

    fn request_concurrency_limit(&self) -> RequestConcurrencyLimit {
        RequestConcurrencyLimit::new(64)
    }

    fn meta_socket_path(&self) -> Option<&Path> {
        Some(&self.meta_socket_path)
    }

    fn database_path(&self) -> &Path {
        &self.database_path
    }

    fn meta_socket_mode(&self) -> Option<RuntimeSocketMode> {
        Some(RuntimeSocketMode::new(
            self.raw.meta_terminal_socket_mode.clone().into_u32(),
        ))
    }
}

impl From<TerminalDaemonConfiguration> for Configuration {
    fn from(raw: TerminalDaemonConfiguration) -> Self {
        Self::from_raw(raw)
    }
}
