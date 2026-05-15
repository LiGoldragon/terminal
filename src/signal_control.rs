use std::collections::HashMap;

use kameo::Actor;
use kameo::actor::ActorRef;
use kameo::error::Infallible;
use kameo::message::{Context, Message};
use regex::bytes::Regex;
use signal_persona_terminal as terminal_signal;
use terminal_cell::{
    InputSource, TerminalCell, TerminalCellError, TerminalInput, TerminalInputGateLease,
    TerminalInputGateSequence, TerminalInputPort, TerminalSize, TerminalWorkerKind,
    TerminalWorkerLifecycle, TerminalWorkerObservationRequest, TerminalWorkerStop,
    TranscriptSnapshotRequest,
};

#[derive(Debug)]
pub struct TerminalSignalControl {
    terminal: ActorRef<TerminalCell>,
    input_port: TerminalInputPort,
    next_prompt_pattern: u64,
    prompt_patterns: HashMap<String, terminal_signal::PromptPattern>,
    signal_leases: HashMap<u64, terminal_signal::PromptState>,
    lifecycle_subscriptions: Vec<terminal_signal::TerminalWorkerLifecycleToken>,
}

impl TerminalSignalControl {
    pub fn new(terminal: ActorRef<TerminalCell>, input_port: TerminalInputPort) -> Self {
        Self {
            terminal,
            input_port,
            next_prompt_pattern: 1,
            prompt_patterns: HashMap::new(),
            signal_leases: HashMap::new(),
            lifecycle_subscriptions: Vec::new(),
        }
    }

    async fn event(
        &mut self,
        request: terminal_signal::TerminalRequest,
    ) -> Result<terminal_signal::TerminalReply, TerminalSignalControlFailure> {
        match request {
            terminal_signal::TerminalRequest::TerminalConnection(connection) => {
                Ok(terminal_signal::TerminalReady {
                    terminal: connection.terminal,
                    generation: terminal_signal::TerminalGeneration::new(1),
                }
                .into())
            }
            terminal_signal::TerminalRequest::TerminalInput(input) => {
                self.input_port
                    .accept(TerminalInput::new(
                        input.bytes.as_slice().to_vec(),
                        InputSource::Programmatic,
                    ))
                    .map_err(TerminalSignalControlFailure::from_terminal_cell)?;
                Ok(terminal_signal::TerminalInputAccepted {
                    terminal: input.terminal,
                    generation: terminal_signal::TerminalGeneration::new(1),
                }
                .into())
            }
            terminal_signal::TerminalRequest::TerminalResize(resize) => {
                let size = TerminalSize::new(resize.rows.into_u16(), resize.columns.into_u16());
                self.terminal
                    .ask(size)
                    .await
                    .map_err(TerminalSignalControlFailure::from_actor_send)?;
                Ok(terminal_signal::TerminalResized {
                    terminal: resize.terminal,
                    rows: resize.rows,
                    columns: resize.columns,
                    generation: terminal_signal::TerminalGeneration::new(1),
                }
                .into())
            }
            terminal_signal::TerminalRequest::TerminalDetachment(detachment) => {
                Ok(terminal_signal::TerminalDetached {
                    terminal: detachment.terminal,
                    generation: terminal_signal::TerminalGeneration::new(1),
                    reason: detachment.reason,
                }
                .into())
            }
            terminal_signal::TerminalRequest::TerminalCapture(capture) => {
                let snapshot = self.snapshot().await?;
                Ok(terminal_signal::TerminalCaptured {
                    terminal: capture.terminal,
                    generation: terminal_signal::TerminalGeneration::new(1),
                    bytes: terminal_signal::TerminalTranscriptBytes::new(snapshot.bytes().to_vec()),
                }
                .into())
            }
            terminal_signal::TerminalRequest::RegisterPromptPattern(registration) => {
                let pattern_id = self.register_prompt_pattern(registration.pattern);
                Ok(terminal_signal::PromptPatternRegistered {
                    terminal: registration.terminal,
                    pattern_id,
                }
                .into())
            }
            terminal_signal::TerminalRequest::UnregisterPromptPattern(unregistration) => {
                self.prompt_patterns
                    .remove(unregistration.pattern_id.as_str());
                Ok(terminal_signal::PromptPatternUnregistered {
                    terminal: unregistration.terminal,
                    pattern_id: unregistration.pattern_id,
                }
                .into())
            }
            terminal_signal::TerminalRequest::ListPromptPatterns(list) => {
                Ok(terminal_signal::PromptPatternList {
                    terminal: list.terminal,
                    entries: self.prompt_pattern_entries(),
                }
                .into())
            }
            terminal_signal::TerminalRequest::AcquireInputGate(acquire) => {
                self.acquire_input_gate(acquire).await
            }
            terminal_signal::TerminalRequest::ReleaseInputGate(release) => {
                self.release_input_gate(release)
            }
            terminal_signal::TerminalRequest::WriteInjection(injection) => {
                self.write_injection(injection).await
            }
            terminal_signal::TerminalRequest::SubscribeTerminalWorkerLifecycle(subscription) => {
                self.open_worker_lifecycle_subscription(subscription).await
            }
            terminal_signal::TerminalRequest::TerminalWorkerLifecycleRetraction(token) => {
                Ok(self.close_worker_lifecycle_subscription(token))
            }
        }
    }

    async fn open_worker_lifecycle_subscription(
        &mut self,
        subscription: terminal_signal::SubscribeTerminalWorkerLifecycle,
    ) -> Result<terminal_signal::TerminalReply, TerminalSignalControlFailure> {
        let token = terminal_signal::TerminalWorkerLifecycleToken {
            terminal: subscription.terminal.clone(),
        };
        if !self.lifecycle_subscriptions.contains(&token) {
            self.lifecycle_subscriptions.push(token);
        }
        let observation = self
            .terminal
            .ask(TerminalWorkerObservationRequest)
            .await
            .map_err(TerminalSignalControlFailure::from_actor_send)?;
        let observations = observation
            .events()
            .iter()
            .cloned()
            .map(Self::worker_lifecycle)
            .collect();
        Ok(terminal_signal::TerminalWorkerLifecycleSnapshot {
            terminal: subscription.terminal,
            observations,
        }
        .into())
    }

    fn close_worker_lifecycle_subscription(
        &mut self,
        token: terminal_signal::TerminalWorkerLifecycleToken,
    ) -> terminal_signal::TerminalReply {
        let position = self
            .lifecycle_subscriptions
            .iter()
            .position(|existing| existing == &token);
        match position {
            Some(index) => {
                self.lifecycle_subscriptions.remove(index);
                terminal_signal::TerminalDetached {
                    terminal: token.terminal,
                    generation: terminal_signal::TerminalGeneration::new(1),
                    reason: terminal_signal::TerminalDetachmentReason::HumanRequested,
                }
                .into()
            }
            None => terminal_signal::TerminalRejected {
                terminal: token.terminal,
                reason: terminal_signal::TerminalRejectionReason::NotConnected,
            }
            .into(),
        }
    }

    fn register_prompt_pattern(
        &mut self,
        pattern: terminal_signal::PromptPattern,
    ) -> terminal_signal::PromptPatternId {
        let pattern_id = terminal_signal::PromptPatternId::new(format!(
            "prompt-pattern-{}",
            self.next_prompt_pattern
        ));
        self.next_prompt_pattern = self.next_prompt_pattern.saturating_add(1);
        self.prompt_patterns
            .insert(pattern_id.as_str().to_string(), pattern);
        pattern_id
    }

    fn prompt_pattern_entries(&self) -> Vec<terminal_signal::PromptPatternEntry> {
        self.prompt_patterns
            .iter()
            .map(
                |(pattern_id, pattern)| terminal_signal::PromptPatternEntry {
                    pattern_id: terminal_signal::PromptPatternId::new(pattern_id.clone()),
                    pattern: pattern.clone(),
                },
            )
            .collect()
    }

    async fn acquire_input_gate(
        &mut self,
        acquire: terminal_signal::AcquireInputGate,
    ) -> Result<terminal_signal::TerminalReply, TerminalSignalControlFailure> {
        let prompt_state = self
            .prompt_state(acquire.prompt_pattern_id.as_ref())
            .await?;
        match self.input_port.close_human_input() {
            Ok(lease) => {
                let signal_lease = Self::signal_lease(lease);
                self.signal_leases
                    .insert(signal_lease.id.into_u64(), prompt_state.clone());
                Ok(terminal_signal::GateAcquired {
                    terminal: acquire.terminal,
                    lease: signal_lease,
                    prompt_state,
                }
                .into())
            }
            Err(TerminalCellError::InputGateAlreadyClosed(lease)) => {
                Ok(terminal_signal::GateBusy {
                    terminal: acquire.terminal,
                    current_holder: terminal_signal::InputGateLeaseId::new(
                        lease.sequence().into_u64(),
                    ),
                }
                .into())
            }
            Err(error) => Err(TerminalSignalControlFailure::from_terminal_cell(error)),
        }
    }

    fn release_input_gate(
        &mut self,
        release: terminal_signal::ReleaseInputGate,
    ) -> Result<terminal_signal::TerminalReply, TerminalSignalControlFailure> {
        if !self
            .signal_leases
            .contains_key(&release.lease.id.into_u64())
        {
            return Ok(terminal_signal::InjectionRejected {
                terminal: release.terminal,
                reason: terminal_signal::InjectionRejectionReason::UnknownLease,
            }
            .into());
        }

        let terminal_lease = Self::terminal_lease(&release.lease);
        match self.input_port.open_human_input(terminal_lease) {
            Ok(gate_release) => {
                self.signal_leases.remove(&release.lease.id.into_u64());
                Ok(terminal_signal::GateReleased {
                    terminal: release.terminal,
                    lease: release.lease,
                    cached_human_bytes: terminal_signal::TerminalByteCount::new(
                        gate_release.held_byte_count() as u64,
                    ),
                }
                .into())
            }
            Err(TerminalCellError::StaleInputGateLease) => {
                self.signal_leases.remove(&release.lease.id.into_u64());
                Ok(terminal_signal::InjectionRejected {
                    terminal: release.terminal,
                    reason: terminal_signal::InjectionRejectionReason::UnknownLease,
                }
                .into())
            }
            Err(error) => Err(TerminalSignalControlFailure::from_terminal_cell(error)),
        }
    }

    async fn write_injection(
        &mut self,
        injection: terminal_signal::WriteInjection,
    ) -> Result<terminal_signal::TerminalReply, TerminalSignalControlFailure> {
        let Some(prompt_state) = self.signal_leases.get(&injection.lease.id.into_u64()) else {
            return Ok(terminal_signal::InjectionRejected {
                terminal: injection.terminal,
                reason: terminal_signal::InjectionRejectionReason::UnknownLease,
            }
            .into());
        };

        if matches!(prompt_state, terminal_signal::PromptState::Dirty { .. }) {
            return Ok(terminal_signal::InjectionRejected {
                terminal: injection.terminal,
                reason: terminal_signal::InjectionRejectionReason::DirtyPrompt,
            }
            .into());
        }

        self.input_port
            .accept(TerminalInput::new(
                injection.bytes.as_slice().to_vec(),
                InputSource::Programmatic,
            ))
            .map_err(TerminalSignalControlFailure::from_terminal_cell)?;
        let snapshot = self.snapshot().await?;
        Ok(terminal_signal::InjectionAck {
            terminal: injection.terminal,
            generation: terminal_signal::TerminalGeneration::new(1),
            sequence: terminal_signal::TerminalSequence::new(snapshot.last_sequence().into_u64()),
        }
        .into())
    }

    async fn prompt_state(
        &self,
        pattern_id: Option<&terminal_signal::PromptPatternId>,
    ) -> Result<terminal_signal::PromptState, TerminalSignalControlFailure> {
        let Some(pattern_id) = pattern_id else {
            return Ok(terminal_signal::PromptState::NotChecked);
        };
        let Some(pattern) = self.prompt_patterns.get(pattern_id.as_str()) else {
            return Ok(terminal_signal::PromptState::Dirty {
                trailing_count: terminal_signal::TerminalByteCount::new(
                    self.snapshot().await?.bytes().len() as u64,
                ),
            });
        };
        let snapshot = self.snapshot().await?;
        let trailing_count = Self::prompt_suffix_trailing_count(pattern, snapshot.bytes())?;
        if trailing_count == 0 {
            Ok(terminal_signal::PromptState::Clean)
        } else {
            Ok(terminal_signal::PromptState::Dirty {
                trailing_count: terminal_signal::TerminalByteCount::new(trailing_count as u64),
            })
        }
    }

    async fn snapshot(
        &self,
    ) -> Result<terminal_cell::TranscriptSnapshot, TerminalSignalControlFailure> {
        self.terminal
            .ask(TranscriptSnapshotRequest)
            .await
            .map_err(TerminalSignalControlFailure::from_actor_send)
    }

    fn prompt_suffix_trailing_count(
        pattern: &terminal_signal::PromptPattern,
        transcript: &[u8],
    ) -> Result<usize, TerminalSignalControlFailure> {
        match pattern {
            terminal_signal::PromptPattern::LiteralSuffix(suffix) => {
                Ok(Self::literal_suffix_gap(transcript, suffix.as_slice()))
            }
            terminal_signal::PromptPattern::RegexSuffix { pattern } => {
                let pattern = std::str::from_utf8(pattern.as_slice()).map_err(|error| {
                    TerminalSignalControlFailure::new(format!(
                        "prompt regex pattern is not utf-8: {error}"
                    ))
                })?;
                Regex::new(pattern)
                    .map(|regex| {
                        regex
                            .find_iter(transcript)
                            .last()
                            .map_or(transcript.len(), |matched| transcript.len() - matched.end())
                    })
                    .map_err(|error| {
                        TerminalSignalControlFailure::new(format!(
                            "prompt regex pattern is invalid: {error}"
                        ))
                    })
            }
        }
    }

    fn literal_suffix_gap(transcript: &[u8], suffix: &[u8]) -> usize {
        if suffix.is_empty() || transcript.ends_with(suffix) {
            return 0;
        }

        transcript
            .windows(suffix.len())
            .rposition(|window| window == suffix)
            .map_or(transcript.len(), |position| {
                transcript.len() - position - suffix.len()
            })
    }

    fn signal_lease(lease: TerminalInputGateLease) -> terminal_signal::InputGateLease {
        terminal_signal::InputGateLease {
            id: terminal_signal::InputGateLeaseId::new(lease.sequence().into_u64()),
        }
    }

    fn terminal_lease(lease: &terminal_signal::InputGateLease) -> TerminalInputGateLease {
        TerminalInputGateLease::new(TerminalInputGateSequence::new(lease.id.into_u64()))
    }

    pub fn worker_lifecycle(
        lifecycle: TerminalWorkerLifecycle,
    ) -> terminal_signal::TerminalWorkerLifecycle {
        match lifecycle {
            TerminalWorkerLifecycle::Started(worker) => {
                terminal_signal::TerminalWorkerLifecycle::Started(Self::worker_kind(worker))
            }
            TerminalWorkerLifecycle::Stopped { worker, reason } => {
                terminal_signal::TerminalWorkerLifecycle::Stopped {
                    worker: Self::worker_kind(worker),
                    reason: Self::worker_stop(reason),
                }
            }
        }
    }

    fn worker_kind(worker: TerminalWorkerKind) -> terminal_signal::TerminalWorkerKind {
        match worker {
            TerminalWorkerKind::InputWriter => terminal_signal::TerminalWorkerKind::InputWriter,
            TerminalWorkerKind::OutputFanout => terminal_signal::TerminalWorkerKind::OutputFanout,
            TerminalWorkerKind::OutputReader => terminal_signal::TerminalWorkerKind::OutputReader,
            TerminalWorkerKind::ChildExitWatcher => {
                terminal_signal::TerminalWorkerKind::ChildExitWatcher
            }
            TerminalWorkerKind::SocketAcceptLoop => {
                terminal_signal::TerminalWorkerKind::SocketAcceptLoop
            }
            TerminalWorkerKind::AttachConnectionPump => {
                terminal_signal::TerminalWorkerKind::AttachConnectionPump
            }
        }
    }

    fn worker_stop(reason: TerminalWorkerStop) -> terminal_signal::TerminalWorkerStopReason {
        match reason {
            TerminalWorkerStop::InputCommandChannelClosed => {
                terminal_signal::TerminalWorkerStopReason::InputCommandChannelClosed
            }
            TerminalWorkerStop::InputWriteFailed(error) => {
                terminal_signal::TerminalWorkerStopReason::InputWriteFailed(error)
            }
            TerminalWorkerStop::OutputCommandChannelClosed => {
                terminal_signal::TerminalWorkerStopReason::OutputCommandChannelClosed
            }
            TerminalWorkerStop::OutputReaderFinished => {
                terminal_signal::TerminalWorkerStopReason::OutputReaderFinished
            }
            TerminalWorkerStop::OutputReadFailed(error) => {
                terminal_signal::TerminalWorkerStopReason::OutputReadFailed(error)
            }
            TerminalWorkerStop::OutputPortClosed => {
                terminal_signal::TerminalWorkerStopReason::OutputPortClosed
            }
            TerminalWorkerStop::ChildExited(status) => {
                terminal_signal::TerminalWorkerStopReason::ChildExited(status)
            }
            TerminalWorkerStop::ChildWaitFailed(error) => {
                terminal_signal::TerminalWorkerStopReason::ChildWaitFailed(error)
            }
            TerminalWorkerStop::SocketAcceptFailed(error) => {
                terminal_signal::TerminalWorkerStopReason::SocketAcceptFailed(error)
            }
            TerminalWorkerStop::AttachConnectionClosed => {
                terminal_signal::TerminalWorkerStopReason::AttachConnectionClosed
            }
            TerminalWorkerStop::AttachConnectionFailed(error) => {
                terminal_signal::TerminalWorkerStopReason::AttachConnectionFailed(error)
            }
        }
    }
}

impl Actor for TerminalSignalControl {
    type Args = Self;
    type Error = Infallible;

    async fn on_start(
        state: Self::Args,
        _actor_reference: ActorRef<Self>,
    ) -> Result<Self, Self::Error> {
        Ok(state)
    }
}

pub struct TerminalSignalControlRequest {
    request: terminal_signal::TerminalRequest,
}

impl TerminalSignalControlRequest {
    pub fn new(request: terminal_signal::TerminalRequest) -> Self {
        Self { request }
    }
}

impl Message<TerminalSignalControlRequest> for TerminalSignalControl {
    type Reply = Result<terminal_signal::TerminalReply, TerminalSignalControlFailure>;

    async fn handle(
        &mut self,
        message: TerminalSignalControlRequest,
        _context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.event(message.request).await
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSignalControlFailure {
    detail: String,
}

impl TerminalSignalControlFailure {
    fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }

    fn from_terminal_cell(error: TerminalCellError) -> Self {
        Self::new(error.to_string())
    }

    fn from_actor_send(error: impl std::fmt::Display) -> Self {
        Self::new(error.to_string())
    }
}

impl std::fmt::Display for TerminalSignalControlFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for TerminalSignalControlFailure {}
