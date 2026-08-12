use std::{fs, path::PathBuf};

use triad_runtime::{ComponentArgument, ComponentCommand};

use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DotosCommandText {
    text: String,
}

impl DotosCommandText {
    pub fn from_command(command: ComponentCommand) -> Result<Self> {
        match command.dotos_argument()? {
            ComponentArgument::InlineDotos(argument) => Ok(Self::new(argument.into_string())),
            ComponentArgument::DotosFile(argument) => Self::from_path(argument.into_path()),
            ComponentArgument::SignalFile(argument) => Self::from_path(argument.into_path()),
        }
    }

    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    pub fn from_path(path: PathBuf) -> Result<Self> {
        let text = fs::read_to_string(&path).map_err(|source| Error::DotosFileRead {
            path: path.clone(),
            source,
        })?;
        Ok(Self::new(text))
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }
}
