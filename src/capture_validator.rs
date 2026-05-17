use std::ffi::OsString;
use std::path::PathBuf;

use signal_persona_terminal::TerminalName;

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureValidatorCommandLine {
    arguments: Vec<OsString>,
}

impl CaptureValidatorCommandLine {
    pub fn from_environment() -> Self {
        Self::from_arguments(std::env::args_os().skip(1))
    }

    pub fn from_arguments<I, S>(arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            arguments: arguments.into_iter().map(Into::into).collect(),
        }
    }

    pub fn run(&self) -> Result<()> {
        let validation = CaptureValidation::from_arguments(&self.arguments)?;
        validation.check()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CaptureValidation {
    capture_path: PathBuf,
    terminal: TerminalName,
    expected_text: String,
}

impl CaptureValidation {
    fn from_arguments(arguments: &[OsString]) -> Result<Self> {
        let mut parser = CaptureValidatorArguments::new(arguments);
        let capture_path = parser.required_path_option("--file")?;
        let terminal = TerminalName::new(parser.required_string_option("--terminal")?);
        let expected_text = parser.required_string_option("--contains-text")?;
        parser.expect_finished()?;
        Ok(Self {
            capture_path,
            terminal,
            expected_text,
        })
    }

    fn check(&self) -> Result<()> {
        let text = std::fs::read_to_string(&self.capture_path)?;
        let artifact = CaptureArtifact::from_text(&text)?;
        artifact.require_contains_text(&self.terminal, &self.expected_text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CaptureArtifact {
    captured: Vec<TerminalCapturedLine>,
}

impl CaptureArtifact {
    fn from_text(text: &str) -> Result<Self> {
        let captured = text
            .lines()
            .filter(|line| line.starts_with("TerminalCaptured\t"))
            .map(TerminalCapturedLine::from_line)
            .collect::<Result<Vec<_>>>()?;
        if captured.is_empty() {
            return Err(Error::ArtifactValidation {
                detail: "no TerminalCaptured line found".to_string(),
            });
        }
        Ok(Self { captured })
    }

    fn require_contains_text(&self, terminal: &TerminalName, expected_text: &str) -> Result<()> {
        let expected_bytes = expected_text.as_bytes();
        for captured in &self.captured {
            if captured.terminal == *terminal && captured.bytes.contains_subslice(expected_bytes) {
                return Ok(());
            }
        }
        Err(Error::ArtifactValidation {
            detail: format!(
                "terminal {:?} capture did not contain text {:?}",
                terminal.as_str(),
                expected_text
            ),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalCapturedLine {
    terminal: TerminalName,
    bytes: CapturedBytes,
}

impl TerminalCapturedLine {
    fn from_line(line: &str) -> Result<Self> {
        let fields = TabSeparatedFields::from_line(line);
        fields.require_len(4)?;
        fields.require_value(0, "TerminalCaptured")?;
        fields.require_u64(2)?;
        Ok(Self {
            terminal: TerminalName::new(fields.required(1)?.to_string()),
            bytes: CapturedBytes::from_hex(fields.required(3)?)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedBytes {
    bytes: Vec<u8>,
}

impl CapturedBytes {
    fn from_hex(hex: &str) -> Result<Self> {
        if hex.len() % 2 != 0 {
            return Err(Error::ArtifactValidation {
                detail: format!("hex payload has odd length: {}", hex.len()),
            });
        }
        let mut bytes = Vec::with_capacity(hex.len() / 2);
        for chunk in hex.as_bytes().chunks(2) {
            bytes.push(HexByte::from_pair(chunk)?.into_u8());
        }
        Ok(Self { bytes })
    }

    fn contains_subslice(&self, needle: &[u8]) -> bool {
        self.bytes
            .windows(needle.len())
            .any(|window| window == needle)
    }
}

struct HexByte {
    value: u8,
}

impl HexByte {
    fn from_pair(pair: &[u8]) -> Result<Self> {
        let high = HexDigit::from_byte(pair[0])?;
        let low = HexDigit::from_byte(pair[1])?;
        Ok(Self {
            value: (high.into_u8() << 4) | low.into_u8(),
        })
    }

    fn into_u8(self) -> u8 {
        self.value
    }
}

struct HexDigit {
    value: u8,
}

impl HexDigit {
    fn from_byte(byte: u8) -> Result<Self> {
        let value = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            other => {
                return Err(Error::ArtifactValidation {
                    detail: format!("invalid hex digit: {}", other as char),
                });
            }
        };
        Ok(Self { value })
    }

    fn into_u8(self) -> u8 {
        self.value
    }
}

struct TabSeparatedFields<'line> {
    fields: Vec<&'line str>,
}

impl<'line> TabSeparatedFields<'line> {
    fn from_line(line: &'line str) -> Self {
        Self {
            fields: line.split('\t').collect(),
        }
    }

    fn require_len(&self, expected: usize) -> Result<()> {
        if self.fields.len() == expected {
            return Ok(());
        }
        Err(Error::ArtifactValidation {
            detail: format!(
                "expected {expected} tab-separated fields, got {}",
                self.fields.len()
            ),
        })
    }

    fn require_value(&self, index: usize, expected: &str) -> Result<()> {
        let got = self.required(index)?;
        if got == expected {
            return Ok(());
        }
        Err(Error::ArtifactValidation {
            detail: format!("expected field {index} to be {expected:?}, got {got:?}"),
        })
    }

    fn require_u64(&self, index: usize) -> Result<u64> {
        self.required(index)?
            .parse::<u64>()
            .map_err(|_| Error::ArtifactValidation {
                detail: format!("field {index} is not an unsigned integer"),
            })
    }

    fn required(&self, index: usize) -> Result<&'line str> {
        self.fields
            .get(index)
            .copied()
            .ok_or_else(|| Error::ArtifactValidation {
                detail: format!("missing field {index}"),
            })
    }
}

struct CaptureValidatorArguments<'arguments> {
    arguments: &'arguments [OsString],
    index: usize,
}

impl<'arguments> CaptureValidatorArguments<'arguments> {
    fn new(arguments: &'arguments [OsString]) -> Self {
        Self {
            arguments,
            index: 0,
        }
    }

    fn required_path_option(&mut self, name: &str) -> Result<PathBuf> {
        self.expect_option_name(name)?;
        self.required_value(name).map(PathBuf::from)
    }

    fn required_string_option(&mut self, name: &str) -> Result<String> {
        self.expect_option_name(name)?;
        self.required_word(name)
    }

    fn expect_finished(&self) -> Result<()> {
        if let Some(extra) = self.arguments.get(self.index) {
            return Err(Error::InvalidArgument {
                detail: format!("unexpected argument: {:?}", extra),
            });
        }
        Ok(())
    }

    fn expect_option_name(&mut self, name: &str) -> Result<()> {
        let got = self.required_word("option")?;
        if got == name {
            return Ok(());
        }
        Err(Error::InvalidArgument {
            detail: format!("expected option {name}, got {got:?}"),
        })
    }

    fn required_word(&mut self, name: &str) -> Result<String> {
        self.required_value(name)?
            .into_os_string()
            .into_string()
            .map_err(|got| Error::InvalidArgument {
                detail: format!("{name} must be UTF-8, got {got:?}"),
            })
    }

    fn required_value(&mut self, name: &str) -> Result<PathBuf> {
        let Some(value) = self.arguments.get(self.index) else {
            return Err(Error::InvalidArgument {
                detail: format!("missing value for {name}"),
            });
        };
        self.index += 1;
        Ok(PathBuf::from(value))
    }
}
