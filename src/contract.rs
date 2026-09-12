use std::path::{Path, PathBuf};

use signal_terminal::{
    Query, Response, TerminalCapturedReply, TerminalDetachedReply, TerminalGeneration,
    TerminalName, TerminalReadyReply, TerminalRejectedReply, TerminalRejectionReason,
    TerminalSequence, TranscriptDeltaReply,
};

use crate::error::{Error, Result};
use crate::operation::query_terminal;
use crate::pty::TerminalSocket;

/// One named terminal's binding onto the terminal-cell control socket that
/// serves it.
///
/// Every query addressed to a terminal this binding does not own is refused
/// as `NotConnected` rather than forwarded; everything it does own crosses
/// the socket as a `signal-terminal` Signal frame and returns the cell's own
/// reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalTransportBinding {
    terminal: TerminalName,
    socket_path: PathBuf,
    generation: TerminalGeneration,
    transcript_sequence: TerminalSequence,
}

impl TerminalTransportBinding {
    pub fn from_socket_path(terminal: TerminalName, socket_path: impl Into<PathBuf>) -> Self {
        Self {
            terminal,
            socket_path: socket_path.into(),
            generation: 1,
            transcript_sequence: 0,
        }
    }

    pub fn terminal(&self) -> &TerminalName {
        &self.terminal
    }

    pub fn socket_path(&self) -> &Path {
        self.socket_path.as_path()
    }

    pub fn generation(&self) -> TerminalGeneration {
        self.generation
    }

    pub fn transcript_sequence(&self) -> TerminalSequence {
        self.transcript_sequence
    }

    pub fn ready_event(&self) -> Response {
        Response::TerminalReady(TerminalReadyReply {
            terminal: self.terminal.clone(),
            generation: self.generation,
        })
    }

    pub fn transcript_event(&mut self, bytes: impl Into<Vec<u8>>) -> Response {
        self.transcript_sequence = self.transcript_sequence.saturating_add(1);
        Response::TranscriptDelta(TranscriptDeltaReply {
            terminal: self.terminal.clone(),
            sequence: self.transcript_sequence,
            transcript_bytes: widen_bytes(&bytes.into()),
        })
    }

    /// Serve one query against this binding.
    ///
    /// Session-registry queries are refused here: they address the whole
    /// registry, which the consolidated terminal daemon owns, not one
    /// terminal's transport.
    pub fn handle_query(&mut self, query: Query) -> Result<Response> {
        match &query {
            Query::ListSessions(_) | Query::ResolveSession(_) => {
                return Err(Error::InvalidArgument {
                    detail: "session registry queries belong to the consolidated terminal daemon"
                        .to_string(),
                });
            }
            _ => {}
        }

        let Some(terminal) = query_terminal(&query) else {
            return Err(Error::InvalidArgument {
                detail: "query names no terminal".to_string(),
            });
        };
        if terminal != &self.terminal {
            return Ok(rejected(
                terminal.clone(),
                TerminalRejectionReason::NotConnected,
            ));
        }

        match query {
            Query::TerminalConnection(_) => Ok(self.ready_event()),
            Query::TerminalDetachment(detachment) => {
                Ok(Response::TerminalDetached(TerminalDetachedReply {
                    terminal: detachment.terminal,
                    generation: self.generation,
                    terminal_detachment_reason: detachment.terminal_detachment_reason,
                }))
            }
            forwarded => self.socket().signal(forwarded),
        }
    }

    /// A capture reply carrying the transcript this binding last saw.
    pub fn captured_event(&self, bytes: &[u8]) -> Response {
        Response::TerminalCaptured(TerminalCapturedReply {
            terminal: self.terminal.clone(),
            generation: self.generation,
            transcript_bytes: widen_bytes(bytes),
        })
    }

    fn socket(&self) -> TerminalSocket {
        TerminalSocket::from_control_socket(self.socket_path.clone())
    }
}

fn rejected(terminal: TerminalName, reason: TerminalRejectionReason) -> Response {
    Response::TerminalRejected(TerminalRejectedReply {
        terminal,
        terminal_rejection_reason: reason,
    })
}

/// Widen a `u8` buffer into the `Integer` byte vector the contract carries.
///
/// The Datom integer is `i64`, so a transcript byte travels as an `i64`
/// holding a value in `0..=255`.
pub fn widen_bytes(bytes: &[u8]) -> Vec<i64> {
    bytes.iter().map(|byte| i64::from(*byte)).collect()
}

/// Narrow the contract's `Integer` byte vector back into a `u8` buffer,
/// truncating each element to its low byte.
pub fn narrow_bytes(bytes: &[i64]) -> Vec<u8> {
    bytes.iter().map(|byte| *byte as u8).collect()
}
