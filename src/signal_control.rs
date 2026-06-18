use std::collections::HashMap;

use kameo::Actor;
use kameo::actor::ActorRef;
use kameo::error::Infallible;
use kameo::message::{Context, Message};
use regex::bytes::Regex;
use signal_terminal as terminal_signal;
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
        request: terminal_signal::Input,
    ) -> Result<terminal_signal::Output, TerminalSignalControlFailure> {
        match request {
            terminal_signal::Input::TerminalConnection(connection) => {
                Ok(terminal_signal::TerminalReady {
                    terminal: connection.into_payload(),
                    generation: Self::signal_generation(1),
                }
                .into())
            }
            terminal_signal::Input::TerminalInput(input) => {
                self.input_port
                    .accept(TerminalInput::new(
                        Self::input_bytes_to_bytes(&input.input_bytes),
                        InputSource::Programmatic,
                    ))
                    .map_err(TerminalSignalControlFailure::from_terminal_cell)?;
                Ok(terminal_signal::TerminalInputAccepted {
                    terminal: input.terminal,
                    generation: Self::signal_generation(1),
                }
                .into())
            }
            terminal_signal::Input::TerminalResize(resize) => {
                let size = TerminalSize::new(
                    Self::rows_to_u16(&resize.rows),
                    Self::columns_to_u16(&resize.columns),
                );
                self.terminal
                    .ask(size)
                    .await
                    .map_err(TerminalSignalControlFailure::from_actor_send)?;
                Ok(terminal_signal::TerminalResized {
                    terminal: resize.terminal,
                    rows: resize.rows,
                    columns: resize.columns,
                    generation: Self::signal_generation(1),
                }
                .into())
            }
            terminal_signal::Input::TerminalDetachment(detachment) => {
                Ok(terminal_signal::TerminalDetached {
                    terminal: detachment.terminal,
                    generation: Self::signal_generation(1),
                    terminal_detachment_reason: detachment.terminal_detachment_reason,
                }
                .into())
            }
            terminal_signal::Input::TerminalCapture(capture) => {
                let snapshot = self.snapshot().await?;
                Ok(terminal_signal::TerminalCaptured {
                    terminal: capture.into_payload(),
                    generation: Self::signal_generation(1),
                    transcript_bytes: Self::signal_transcript_bytes(snapshot.bytes()),
                }
                .into())
            }
            terminal_signal::Input::RegisterPromptPattern(registration) => {
                let pattern_id = self.register_prompt_pattern(registration.pattern.into_payload());
                Ok(terminal_signal::PromptPatternRegistered {
                    terminal: registration.terminal,
                    pattern_identifier: pattern_id.into(),
                }
                .into())
            }
            terminal_signal::Input::UnregisterPromptPattern(unregistration) => {
                self.prompt_patterns
                    .remove(unregistration.pattern_identifier.payload().as_str());
                Ok(terminal_signal::PromptPatternUnregistered {
                    terminal: unregistration.terminal,
                    pattern_identifier: unregistration.pattern_identifier,
                }
                .into())
            }
            terminal_signal::Input::ListPromptPatterns(list) => {
                Ok(terminal_signal::PromptPatternList {
                    terminal: list.into_payload(),
                    entries: self.prompt_pattern_entries().into(),
                }
                .into())
            }
            terminal_signal::Input::AcquireInputGate(acquire) => {
                self.acquire_input_gate(acquire).await
            }
            terminal_signal::Input::ReleaseInputGate(release) => self.release_input_gate(release),
            terminal_signal::Input::WriteInjection(injection) => {
                self.write_injection(injection).await
            }
            terminal_signal::Input::SubscribeTerminalWorkerLifecycle(subscription) => {
                self.open_worker_lifecycle_subscription(subscription).await
            }
            terminal_signal::Input::TerminalWorkerLifecycleRetraction(token) => {
                Ok(self.close_worker_lifecycle_subscription(token))
            }
            terminal_signal::Input::ListSessions(_) | terminal_signal::Input::ResolveSession(_) => {
                Err(TerminalSignalControlFailure::new(
                    "session registry queries belong to the consolidated terminal daemon",
                ))
            }
        }
    }

    async fn open_worker_lifecycle_subscription(
        &mut self,
        subscription: terminal_signal::SubscribeTerminalWorkerLifecycle,
    ) -> Result<terminal_signal::Output, TerminalSignalControlFailure> {
        let terminal = subscription.into_payload();
        let token = terminal_signal::TerminalWorkerLifecycleToken::new(terminal.clone());
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
            .collect::<Vec<_>>()
            .into();
        Ok(terminal_signal::TerminalWorkerLifecycleSnapshot {
            terminal,
            observations,
        }
        .into())
    }

    fn close_worker_lifecycle_subscription(
        &mut self,
        token: terminal_signal::TerminalWorkerLifecycleToken,
    ) -> terminal_signal::Output {
        let position = self
            .lifecycle_subscriptions
            .iter()
            .position(|existing| existing == &token);
        match position {
            Some(index) => {
                self.lifecycle_subscriptions.remove(index);
                terminal_signal::SubscriptionRetracted::new(token.into()).into()
            }
            None => terminal_signal::TerminalRejected {
                terminal: token.into_payload(),
                terminal_rejection_reason: terminal_signal::TerminalRejectionReason::NotConnected,
            }
            .into(),
        }
    }

    fn register_prompt_pattern(
        &mut self,
        pattern: terminal_signal::PromptPattern,
    ) -> terminal_signal::PromptPatternIdentifier {
        let pattern_id = terminal_signal::PromptPatternIdentifier::new(format!(
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
                    pattern_identifier: terminal_signal::PromptPatternIdentifier::new(
                        pattern_id.clone(),
                    )
                    .into(),
                    pattern: pattern.clone().into(),
                },
            )
            .collect()
    }

    async fn acquire_input_gate(
        &mut self,
        acquire: terminal_signal::AcquireInputGate,
    ) -> Result<terminal_signal::Output, TerminalSignalControlFailure> {
        let prompt_state = self
            .prompt_state(
                acquire
                    .prompt_pattern_identifier_selection
                    .payload()
                    .as_ref(),
            )
            .await?;
        match self.input_port.close_human_input() {
            Ok(lease) => {
                let signal_lease = Self::signal_lease(lease);
                self.signal_leases
                    .insert(Self::signal_lease_key(&signal_lease), prompt_state.clone());
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
                    current_holder: terminal_signal::InputGateLeaseIdentifier::new(
                        lease.sequence().into_u64(),
                    )
                    .into(),
                }
                .into())
            }
            Err(error) => Err(TerminalSignalControlFailure::from_terminal_cell(error)),
        }
    }

    fn release_input_gate(
        &mut self,
        release: terminal_signal::ReleaseInputGate,
    ) -> Result<terminal_signal::Output, TerminalSignalControlFailure> {
        let lease_key = Self::signal_lease_key(&release.lease);
        if !self.signal_leases.contains_key(&lease_key) {
            return Ok(terminal_signal::InjectionRejected {
                terminal: release.terminal,
                injection_rejection_reason: terminal_signal::InjectionRejectionReason::UnknownLease,
            }
            .into());
        }

        let terminal_lease = Self::terminal_lease(&release.lease);
        match self.input_port.open_human_input(terminal_lease) {
            Ok(gate_release) => {
                self.signal_leases.remove(&lease_key);
                Ok(terminal_signal::GateReleased {
                    terminal: release.terminal,
                    lease: release.lease,
                    cached_human_bytes: terminal_signal::TerminalByteCount::new(
                        gate_release.held_byte_count() as u64,
                    )
                    .into(),
                }
                .into())
            }
            Err(TerminalCellError::StaleInputGateLease) => {
                self.signal_leases.remove(&lease_key);
                Ok(terminal_signal::InjectionRejected {
                    terminal: release.terminal,
                    injection_rejection_reason:
                        terminal_signal::InjectionRejectionReason::UnknownLease,
                }
                .into())
            }
            Err(error) => Err(TerminalSignalControlFailure::from_terminal_cell(error)),
        }
    }

    async fn write_injection(
        &mut self,
        injection: terminal_signal::WriteInjection,
    ) -> Result<terminal_signal::Output, TerminalSignalControlFailure> {
        let lease_key = Self::signal_lease_key(&injection.lease);
        let Some(prompt_state) = self.signal_leases.get(&lease_key) else {
            return Ok(terminal_signal::InjectionRejected {
                terminal: injection.terminal,
                injection_rejection_reason: terminal_signal::InjectionRejectionReason::UnknownLease,
            }
            .into());
        };

        if matches!(prompt_state, terminal_signal::PromptState::Dirty(_)) {
            return Ok(terminal_signal::InjectionRejected {
                terminal: injection.terminal,
                injection_rejection_reason: terminal_signal::InjectionRejectionReason::DirtyPrompt,
            }
            .into());
        }

        self.input_port
            .accept(TerminalInput::new(
                Self::input_bytes_to_bytes(&injection.input_bytes),
                InputSource::Programmatic,
            ))
            .map_err(TerminalSignalControlFailure::from_terminal_cell)?;
        let snapshot = self.snapshot().await?;
        Ok(terminal_signal::InjectionAck {
            terminal: injection.terminal,
            generation: Self::signal_generation(1),
            sequence: terminal_signal::TerminalSequence::new(snapshot.last_sequence().into_u64())
                .into(),
        }
        .into())
    }

    async fn prompt_state(
        &self,
        pattern_id: Option<&terminal_signal::PromptPatternIdentifier>,
    ) -> Result<terminal_signal::PromptState, TerminalSignalControlFailure> {
        let Some(pattern_id) = pattern_id else {
            return Ok(terminal_signal::PromptState::NotChecked);
        };
        let Some(pattern) = self.prompt_patterns.get(pattern_id.as_str()) else {
            return Ok(terminal_signal::PromptState::Dirty(
                terminal_signal::TerminalByteCount::new(self.snapshot().await?.bytes().len() as u64),
            ));
        };
        let snapshot = self.snapshot().await?;
        let trailing_count = Self::prompt_suffix_trailing_count(pattern, snapshot.bytes())?;
        if trailing_count == 0 {
            Ok(terminal_signal::PromptState::Clean)
        } else {
            Ok(terminal_signal::PromptState::Dirty(
                terminal_signal::TerminalByteCount::new(trailing_count as u64),
            ))
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
            terminal_signal::PromptPattern::LiteralSuffix(suffix) => Ok(Self::literal_suffix_gap(
                transcript,
                &Self::signal_bytes_to_bytes(suffix.payload().as_slice()),
            )),
            terminal_signal::PromptPattern::RegexSuffix(pattern) => {
                let pattern = Self::signal_bytes_to_bytes(pattern.payload().as_slice());
                let pattern = std::str::from_utf8(&pattern).map_err(|error| {
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

    fn signal_lease(lease: TerminalInputGateLease) -> terminal_signal::Lease {
        terminal_signal::InputGateLease::new(terminal_signal::InputGateLeaseIdentifier::new(
            lease.sequence().into_u64(),
        ))
        .into()
    }

    fn terminal_lease(lease: &terminal_signal::Lease) -> TerminalInputGateLease {
        TerminalInputGateLease::new(TerminalInputGateSequence::new(Self::signal_lease_key(
            lease,
        )))
    }

    fn signal_lease_key(lease: &terminal_signal::Lease) -> u64 {
        *lease.payload().payload().payload()
    }

    fn signal_generation(value: u64) -> terminal_signal::Generation {
        terminal_signal::TerminalGeneration::new(value).into()
    }

    fn rows_to_u16(rows: &terminal_signal::Rows) -> u16 {
        *rows.payload().payload() as u16
    }

    fn columns_to_u16(columns: &terminal_signal::Columns) -> u16 {
        *columns.payload().payload() as u16
    }

    fn input_bytes_to_bytes(input_bytes: &terminal_signal::InputBytes) -> Vec<u8> {
        Self::signal_bytes_to_bytes(input_bytes.payload().payload().as_slice())
    }

    fn signal_transcript_bytes(bytes: &[u8]) -> terminal_signal::TranscriptBytes {
        terminal_signal::TerminalTranscriptBytes::new(Self::bytes_to_signal_bytes(bytes)).into()
    }

    /// Lower terminal-cell's `u8` byte buffer into the schema-emitted
    /// `Integer` (`u64`) byte vector the signal-terminal contract carries.
    fn bytes_to_signal_bytes(bytes: &[u8]) -> Vec<u64> {
        bytes.iter().map(|byte| u64::from(*byte)).collect()
    }

    /// Narrow the schema-emitted `Integer` (`u64`) byte vector back into a
    /// terminal-cell `u8` buffer, truncating each element to its low byte.
    fn signal_bytes_to_bytes(bytes: &[u64]) -> Vec<u8> {
        bytes.iter().map(|byte| *byte as u8).collect()
    }

    pub fn worker_lifecycle(
        lifecycle: TerminalWorkerLifecycle,
    ) -> terminal_signal::TerminalWorkerLifecycle {
        match lifecycle {
            TerminalWorkerLifecycle::Started(worker) => {
                terminal_signal::TerminalWorkerLifecycle::Started(Self::worker_kind(worker))
            }
            TerminalWorkerLifecycle::Stopped { worker, reason } => {
                terminal_signal::TerminalWorkerLifecycle::Stopped(
                    terminal_signal::TerminalWorkerStop {
                        terminal_worker_kind: Self::worker_kind(worker),
                        terminal_worker_stop_reason: Self::worker_stop(reason),
                    },
                )
            }
        }
    }

    fn worker_kind(worker: TerminalWorkerKind) -> terminal_signal::TerminalWorkerKind {
        match worker {
            TerminalWorkerKind::InputWriter => terminal_signal::TerminalWorkerKind::InputWriter,
            TerminalWorkerKind::ViewerFanout => terminal_signal::TerminalWorkerKind::ViewerFanout,
            TerminalWorkerKind::TranscriptScriber => {
                terminal_signal::TerminalWorkerKind::TranscriptScriber
            }
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
                terminal_signal::TerminalWorkerStopReason::InputWriteFailed(
                    terminal_signal::WorkerFailureDetail::new(error),
                )
            }
            TerminalWorkerStop::OutputCommandChannelClosed => {
                terminal_signal::TerminalWorkerStopReason::OutputCommandChannelClosed
            }
            TerminalWorkerStop::TranscriptNoticeChannelClosed => {
                terminal_signal::TerminalWorkerStopReason::TranscriptNoticeChannelClosed
            }
            TerminalWorkerStop::OutputReaderFinished => {
                terminal_signal::TerminalWorkerStopReason::OutputReaderFinished
            }
            TerminalWorkerStop::OutputReadFailed(error) => {
                terminal_signal::TerminalWorkerStopReason::OutputReadFailed(
                    terminal_signal::WorkerFailureDetail::new(error),
                )
            }
            TerminalWorkerStop::OutputPortClosed => {
                terminal_signal::TerminalWorkerStopReason::OutputPortClosed
            }
            TerminalWorkerStop::ChildExited(status) => {
                terminal_signal::TerminalWorkerStopReason::ChildExited(
                    terminal_signal::WorkerFailureDetail::new(status),
                )
            }
            TerminalWorkerStop::ChildWaitFailed(error) => {
                terminal_signal::TerminalWorkerStopReason::ChildWaitFailed(
                    terminal_signal::WorkerFailureDetail::new(error),
                )
            }
            TerminalWorkerStop::SocketAcceptFailed(error) => {
                terminal_signal::TerminalWorkerStopReason::SocketAcceptFailed(
                    terminal_signal::WorkerFailureDetail::new(error),
                )
            }
            TerminalWorkerStop::AttachConnectionClosed => {
                terminal_signal::TerminalWorkerStopReason::AttachConnectionClosed
            }
            TerminalWorkerStop::AttachConnectionFailed(error) => {
                terminal_signal::TerminalWorkerStopReason::AttachConnectionFailed(
                    terminal_signal::WorkerFailureDetail::new(error),
                )
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
    request: terminal_signal::Input,
}

impl TerminalSignalControlRequest {
    pub fn new(request: terminal_signal::Input) -> Self {
        Self { request }
    }
}

impl Message<TerminalSignalControlRequest> for TerminalSignalControl {
    type Reply = Result<terminal_signal::Output, TerminalSignalControlFailure>;

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
