use std::path::{Path, PathBuf};

use signal_terminal::{
    AcquireInputGate, Input, ListPromptPatterns, Output, RegisterPromptPattern, ReleaseInputGate,
    TerminalCapture, TerminalConnection, TerminalDetached, TerminalGeneration, TerminalInput,
    TerminalName, TerminalReady, TerminalRejected, TerminalRejectionReason, TerminalResize,
    TerminalSequence, TerminalTranscriptBytes, TranscriptDelta, UnregisterPromptPattern,
    WriteInjection,
};

use crate::error::{Error, Result};
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
        self.generation.clone()
    }

    pub fn transcript_sequence(&self) -> TerminalSequence {
        self.transcript_sequence.clone()
    }

    pub fn ready_event(&self) -> Output {
        TerminalReady {
            terminal: self.terminal.clone().into(),
            generation: self.generation.clone().into(),
        }
        .into()
    }

    pub fn transcript_event(&mut self, bytes: impl Into<Vec<u8>>) -> Output {
        self.transcript_sequence = TerminalSequence::new(
            self.transcript_sequence
                .clone()
                .into_u64()
                .saturating_add(1),
        );
        TranscriptDelta {
            terminal: self.terminal.clone().into(),
            sequence: self.transcript_sequence.clone().into(),
            transcript_bytes: TerminalTranscriptBytes::new(Self::bytes_to_signal_bytes(
                &bytes.into(),
            ))
            .into(),
        }
        .into()
    }

    /// Lower terminal-cell's `u8` byte buffer into the schema-emitted
    /// `Integer` (`u64`) byte vector the signal-terminal contract carries.
    fn bytes_to_signal_bytes(bytes: &[u8]) -> Vec<u64> {
        bytes.iter().map(|byte| u64::from(*byte)).collect()
    }

    pub fn handle_request(&mut self, request: Input) -> Result<Output> {
        match request {
            Input::TerminalConnection(connection) => Ok(self.handle_connection(connection)),
            Input::TerminalInput(input) => self.handle_input(input),
            Input::TerminalResize(resize) => self.handle_resize(resize),
            Input::TerminalDetachment(detachment) => {
                if !self.contains_signal_terminal(&detachment.terminal) {
                    return Ok(Self::rejected(
                        detachment.terminal,
                        TerminalRejectionReason::NotConnected,
                    ));
                }
                Ok(TerminalDetached {
                    terminal: detachment.terminal,
                    generation: self.generation.clone().into(),
                    terminal_detachment_reason: detachment.terminal_detachment_reason,
                }
                .into())
            }
            Input::TerminalCapture(capture) => self.handle_capture(capture),
            Input::RegisterPromptPattern(registration) => {
                self.handle_register_prompt_pattern(registration)
            }
            Input::UnregisterPromptPattern(unregistration) => {
                self.handle_unregister_prompt_pattern(unregistration)
            }
            Input::ListPromptPatterns(list) => self.handle_list_prompt_patterns(list),
            Input::AcquireInputGate(acquire) => self.handle_acquire_input_gate(acquire),
            Input::ReleaseInputGate(release) => self.handle_release_input_gate(release),
            Input::WriteInjection(injection) => self.handle_write_injection(injection),
            Input::SubscribeTerminalWorkerLifecycle(subscription) => {
                let terminal = subscription.payload().clone();
                self.handle_signal_control(
                    terminal,
                    Input::SubscribeTerminalWorkerLifecycle(subscription),
                )
            }
            Input::TerminalWorkerLifecycleRetraction(token) => {
                let terminal = token.payload().clone();
                self.handle_signal_control(
                    terminal,
                    Input::TerminalWorkerLifecycleRetraction(token),
                )
            }
            Input::ListSessions(_) | Input::ResolveSession(_) => Err(Error::InvalidArgument {
                detail: "session registry queries belong to the consolidated terminal daemon"
                    .to_string(),
            }),
        }
    }

    fn handle_connection(&self, connection: TerminalConnection) -> Output {
        let terminal = connection.into_payload();
        if !self.contains_signal_terminal(&terminal) {
            return Self::rejected(terminal, TerminalRejectionReason::NotConnected);
        }
        self.ready_event()
    }

    fn handle_input(&self, input: TerminalInput) -> Result<Output> {
        self.handle_signal_control(input.terminal.clone(), Input::TerminalInput(input))
    }

    fn handle_resize(&self, resize: TerminalResize) -> Result<Output> {
        self.handle_signal_control(resize.terminal.clone(), Input::TerminalResize(resize))
    }

    fn handle_capture(&self, capture: TerminalCapture) -> Result<Output> {
        let terminal = capture.payload().clone();
        self.handle_signal_control(terminal, Input::TerminalCapture(capture))
    }

    fn handle_register_prompt_pattern(
        &self,
        registration: RegisterPromptPattern,
    ) -> Result<Output> {
        self.handle_signal_control(
            registration.terminal.clone(),
            Input::RegisterPromptPattern(registration),
        )
    }

    fn handle_unregister_prompt_pattern(
        &self,
        unregistration: UnregisterPromptPattern,
    ) -> Result<Output> {
        self.handle_signal_control(
            unregistration.terminal.clone(),
            Input::UnregisterPromptPattern(unregistration),
        )
    }

    fn handle_list_prompt_patterns(&self, list: ListPromptPatterns) -> Result<Output> {
        let terminal = list.payload().clone();
        self.handle_signal_control(terminal, Input::ListPromptPatterns(list))
    }

    fn handle_acquire_input_gate(&self, acquire: AcquireInputGate) -> Result<Output> {
        self.handle_signal_control(acquire.terminal.clone(), Input::AcquireInputGate(acquire))
    }

    fn handle_release_input_gate(&self, release: ReleaseInputGate) -> Result<Output> {
        self.handle_signal_control(release.terminal.clone(), Input::ReleaseInputGate(release))
    }

    fn handle_write_injection(&self, injection: WriteInjection) -> Result<Output> {
        self.handle_signal_control(injection.terminal.clone(), Input::WriteInjection(injection))
    }

    fn handle_signal_control(
        &self,
        terminal: signal_terminal::Terminal,
        request: Input,
    ) -> Result<Output> {
        if !self.contains_signal_terminal(&terminal) {
            return Ok(Self::rejected(
                terminal,
                TerminalRejectionReason::NotConnected,
            ));
        }
        self.socket().signal(request)
    }

    fn contains_signal_terminal(&self, terminal: &signal_terminal::Terminal) -> bool {
        terminal.payload() == &self.terminal
    }

    fn socket(&self) -> TerminalSocket {
        TerminalSocket::from_control_socket(self.socket_path.clone())
    }

    fn rejected(terminal: signal_terminal::Terminal, reason: TerminalRejectionReason) -> Output {
        TerminalRejected {
            terminal,
            terminal_rejection_reason: reason,
        }
        .into()
    }
}
