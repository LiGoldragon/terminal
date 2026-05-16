use std::path::{Path, PathBuf};

use signal_persona_terminal::{
    AcquireInputGate, ListPromptPatterns, RegisterPromptPattern, ReleaseInputGate, TerminalCapture,
    TerminalCaptured, TerminalConnection, TerminalDetached, TerminalGeneration, TerminalInput,
    TerminalInputAccepted, TerminalName, TerminalReady, TerminalRejected, TerminalRejectionReason,
    TerminalReply, TerminalRequest, TerminalResize, TerminalResized, TerminalSequence,
    TerminalTranscriptBytes, TranscriptDelta, UnregisterPromptPattern, WriteInjection,
};

use crate::error::Result;
use crate::pty::TerminalSocket;

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

    pub fn ready_event(&self) -> TerminalReply {
        TerminalReady {
            terminal: self.terminal.clone(),
            generation: self.generation,
        }
        .into()
    }

    pub fn transcript_event(&mut self, bytes: impl Into<Vec<u8>>) -> TerminalReply {
        self.transcript_sequence =
            TerminalSequence::new(self.transcript_sequence.into_u64().saturating_add(1));
        TranscriptDelta {
            terminal: self.terminal.clone(),
            sequence: self.transcript_sequence,
            bytes: TerminalTranscriptBytes::new(bytes.into()),
        }
        .into()
    }

    pub fn handle_request(&mut self, request: TerminalRequest) -> Result<TerminalReply> {
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
            TerminalRequest::RegisterPromptPattern(registration) => {
                self.handle_register_prompt_pattern(registration)
            }
            TerminalRequest::UnregisterPromptPattern(unregistration) => {
                self.handle_unregister_prompt_pattern(unregistration)
            }
            TerminalRequest::ListPromptPatterns(list) => self.handle_list_prompt_patterns(list),
            TerminalRequest::AcquireInputGate(acquire) => self.handle_acquire_input_gate(acquire),
            TerminalRequest::ReleaseInputGate(release) => self.handle_release_input_gate(release),
            TerminalRequest::WriteInjection(injection) => self.handle_write_injection(injection),
            TerminalRequest::SubscribeTerminalWorkerLifecycle(subscription) => self
                .handle_signal_control(
                    subscription.terminal.clone(),
                    TerminalRequest::SubscribeTerminalWorkerLifecycle(subscription),
                ),
            TerminalRequest::TerminalWorkerLifecycleRetraction(token) => self
                .handle_signal_control(
                    token.terminal.clone(),
                    TerminalRequest::TerminalWorkerLifecycleRetraction(token),
                ),
        }
    }

    fn handle_connection(&self, connection: TerminalConnection) -> TerminalReply {
        if !self.contains_terminal(&connection.terminal) {
            return Self::rejected(connection.terminal, TerminalRejectionReason::NotConnected);
        }
        self.ready_event()
    }

    fn handle_input(&self, input: TerminalInput) -> Result<TerminalReply> {
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

    fn handle_resize(&self, resize: TerminalResize) -> Result<TerminalReply> {
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

    fn handle_capture(&self, capture: TerminalCapture) -> Result<TerminalReply> {
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

    fn handle_register_prompt_pattern(
        &self,
        registration: RegisterPromptPattern,
    ) -> Result<TerminalReply> {
        self.handle_signal_control(
            registration.terminal.clone(),
            TerminalRequest::RegisterPromptPattern(registration),
        )
    }

    fn handle_unregister_prompt_pattern(
        &self,
        unregistration: UnregisterPromptPattern,
    ) -> Result<TerminalReply> {
        self.handle_signal_control(
            unregistration.terminal.clone(),
            TerminalRequest::UnregisterPromptPattern(unregistration),
        )
    }

    fn handle_list_prompt_patterns(&self, list: ListPromptPatterns) -> Result<TerminalReply> {
        self.handle_signal_control(
            list.terminal.clone(),
            TerminalRequest::ListPromptPatterns(list),
        )
    }

    fn handle_acquire_input_gate(&self, acquire: AcquireInputGate) -> Result<TerminalReply> {
        self.handle_signal_control(
            acquire.terminal.clone(),
            TerminalRequest::AcquireInputGate(acquire),
        )
    }

    fn handle_release_input_gate(&self, release: ReleaseInputGate) -> Result<TerminalReply> {
        self.handle_signal_control(
            release.terminal.clone(),
            TerminalRequest::ReleaseInputGate(release),
        )
    }

    fn handle_write_injection(&self, injection: WriteInjection) -> Result<TerminalReply> {
        self.handle_signal_control(
            injection.terminal.clone(),
            TerminalRequest::WriteInjection(injection),
        )
    }

    fn handle_signal_control(
        &self,
        terminal: TerminalName,
        request: TerminalRequest,
    ) -> Result<TerminalReply> {
        if !self.contains_terminal(&terminal) {
            return Ok(Self::rejected(
                terminal,
                TerminalRejectionReason::NotConnected,
            ));
        }
        self.socket().signal(request)
    }

    fn contains_terminal(&self, terminal: &TerminalName) -> bool {
        terminal == &self.terminal
    }

    fn socket(&self) -> TerminalSocket {
        TerminalSocket::from_control_socket(self.socket_path.clone())
    }

    fn rejected(terminal: TerminalName, reason: TerminalRejectionReason) -> TerminalReply {
        TerminalRejected { terminal, reason }.into()
    }
}
