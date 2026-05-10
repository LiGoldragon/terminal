use std::path::{Path, PathBuf};

use signal_persona_terminal::{
    TerminalCapture, TerminalCaptured, TerminalConnection, TerminalDetached, TerminalEvent,
    TerminalGeneration, TerminalInput, TerminalInputAccepted, TerminalName, TerminalReady,
    TerminalRejected, TerminalRejectionReason, TerminalRequest, TerminalResize, TerminalResized,
    TerminalSequence, TerminalTranscriptBytes, TranscriptDelta,
};

use crate::error::Result;
use crate::pty::PtySocket;

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
            generation: TerminalGeneration::new(1),
            transcript_sequence: TerminalSequence::new(0),
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

    pub fn ready_event(&self) -> TerminalEvent {
        TerminalReady {
            terminal: self.terminal.clone(),
            generation: self.generation,
        }
        .into()
    }

    pub fn transcript_event(&mut self, bytes: impl Into<Vec<u8>>) -> TerminalEvent {
        self.transcript_sequence =
            TerminalSequence::new(self.transcript_sequence.into_u64().saturating_add(1));
        TranscriptDelta {
            terminal: self.terminal.clone(),
            sequence: self.transcript_sequence,
            bytes: TerminalTranscriptBytes::new(bytes.into()),
        }
        .into()
    }

    pub fn handle_request(&mut self, request: TerminalRequest) -> Result<TerminalEvent> {
        match request {
            TerminalRequest::TerminalConnection(connection) => {
                Ok(self.handle_connection(connection))
            }
            TerminalRequest::TerminalInput(input) => self.handle_input(input),
            TerminalRequest::TerminalResize(resize) => self.handle_resize(resize),
            TerminalRequest::TerminalDetachment(detachment) => {
                if !self.contains_terminal(&detachment.terminal) {
                    return Ok(Self::rejected(
                        detachment.terminal,
                        TerminalRejectionReason::NotConnected,
                    ));
                }
                Ok(TerminalDetached {
                    terminal: detachment.terminal,
                    generation: self.generation,
                    reason: detachment.reason,
                }
                .into())
            }
            TerminalRequest::TerminalCapture(capture) => self.handle_capture(capture),
        }
    }

    fn handle_connection(&self, connection: TerminalConnection) -> TerminalEvent {
        if !self.contains_terminal(&connection.terminal) {
            return Self::rejected(connection.terminal, TerminalRejectionReason::NotConnected);
        }
        self.ready_event()
    }

    fn handle_input(&self, input: TerminalInput) -> Result<TerminalEvent> {
        if !self.contains_terminal(&input.terminal) {
            return Ok(Self::rejected(
                input.terminal,
                TerminalRejectionReason::NotConnected,
            ));
        }
        self.socket().send_bytes(input.bytes.as_slice())?;
        Ok(TerminalInputAccepted {
            terminal: input.terminal,
            generation: self.generation,
        }
        .into())
    }

    fn handle_resize(&self, resize: TerminalResize) -> Result<TerminalEvent> {
        if !self.contains_terminal(&resize.terminal) {
            return Ok(Self::rejected(
                resize.terminal,
                TerminalRejectionReason::NotConnected,
            ));
        }
        self.socket()
            .resize(resize.rows.into_u16(), resize.columns.into_u16())?;
        Ok(TerminalResized {
            terminal: resize.terminal,
            rows: resize.rows,
            columns: resize.columns,
            generation: self.generation,
        }
        .into())
    }

    fn handle_capture(&self, capture: TerminalCapture) -> Result<TerminalEvent> {
        if !self.contains_terminal(&capture.terminal) {
            return Ok(Self::rejected(
                capture.terminal,
                TerminalRejectionReason::NotConnected,
            ));
        }
        let snapshot = self.socket().capture()?;
        Ok(TerminalCaptured {
            terminal: capture.terminal,
            generation: self.generation,
            bytes: TerminalTranscriptBytes::new(snapshot.as_bytes().to_vec()),
        }
        .into())
    }

    fn contains_terminal(&self, terminal: &TerminalName) -> bool {
        terminal == &self.terminal
    }

    fn socket(&self) -> PtySocket {
        PtySocket::from_path(self.socket_path.clone())
    }

    fn rejected(terminal: TerminalName, reason: TerminalRejectionReason) -> TerminalEvent {
        TerminalRejected { terminal, reason }.into()
    }
}
