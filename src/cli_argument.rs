use std::fs;
use std::path::PathBuf;

use triad_runtime::{ComponentArgument, ComponentCommand};

use crate::{Error, Result};

/// One Datom value taken from the command line, inline or from a file.
///
/// The component CLIs take exactly one inline Datom value and no flags, as
/// every Datom-speaking CLI does. `triad-runtime` still spells its argument
/// variants in the retired notation's names; the text they carry is Datom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatomCommandText {
    text: String,
}

impl DatomCommandText {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    pub fn from_command(command: ComponentCommand) -> Result<Self> {
        match command.nota_argument()? {
            ComponentArgument::InlineNota(argument) => Ok(Self::new(argument.into_string())),
            ComponentArgument::NotaFile(argument) => Self::from_path(argument.into_path()),
            other => Err(Error::InvalidArgument {
                detail: format!("expected one Datom argument, got {other:?}"),
            }),
        }
    }

    pub fn from_path(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let text = fs::read_to_string(&path).map_err(|source| Error::DatomFileRead {
            path: path.clone(),
            source,
        })?;
        Ok(Self::new(text))
    }

    pub fn as_str(&self) -> &str {
        self.text.as_str()
    }

    pub fn into_string(self) -> String {
        self.text
    }
}
