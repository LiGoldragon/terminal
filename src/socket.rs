use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocketMode(u32);

impl SocketMode {
    pub const fn from_octal(value: u32) -> Self {
        Self(value)
    }

    pub fn from_environment() -> Option<Self> {
        std::env::var("PERSONA_SOCKET_MODE")
            .ok()
            .and_then(|value| u32::from_str_radix(value.as_str(), 8).ok())
            .map(Self::from_octal)
    }

    pub const fn as_octal(self) -> u32 {
        self.0
    }

    pub fn apply_to(self, path: &Path) -> Result<()> {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(self.as_octal()))?;
        Ok(())
    }
}
