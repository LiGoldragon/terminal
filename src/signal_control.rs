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

use crate::contract::{narrow_bytes, widen_bytes};

/// The generation every reply this control plane issues carries.
///
/// One `TerminalSignalControl` owns exactly one cell for the life of that
/// cell, so the generation never advances within it.
const GENERATION: terminal_signal::Generation = 1;

#[derive(Debug)]
pub struct TerminalSignalControl {
    terminal: ActorRef<TerminalCell>,
    input_port: TerminalInputPort,
    next_prompt_pattern: i64,
    prompt_patterns: HashMap<String, terminal_signal::PromptPattern>,
    signal_leases: HashMap<i64, terminal_signal::PromptState>,
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
        request: terminal_signal::Query,
    ) -> Result<terminal_signal::Response, TerminalSignalControlFailure> {
        match request {
            terminal_signal::Query::TerminalConnection(connection) => Ok(
                terminal_signal::Response::TerminalReady(terminal_signal::TerminalReadyReply {
                    terminal: connection.terminal,
                    generation: GENERATION,
                }),
            ),
            terminal_signal::Query::TerminalInput(input) => {
                self.input_port
                    .accept(TerminalInput::new(
                        narrow_bytes(&input.input_bytes),
                        InputSource::Programmatic,
                    ))
                    .map_err(TerminalSignalControlFailure::from_terminal_cell)?;
                Ok(terminal_signal::Response::TerminalInputAccepted(
                    terminal_signal::TerminalInputAcceptedReply {
                        terminal: input.terminal,
                        generation: GENERATION,
                    },
                ))
            }
            terminal_signal::Query::TerminalResize(resize) => {
                let size = TerminalSize::new(resize.rows as u16, resize.columns as u16);
                self.terminal
                    .ask(size)
                    .await
                    .map_err(TerminalSignalControlFailure::from_actor_send)?;
                Ok(terminal_signal::Response::TerminalResized(
                    terminal_signal::TerminalResizedReply {
                        terminal: resize.terminal,
                        rows: resize.rows,
                        columns: resize.columns,
                        generation: GENERATION,
                    },
                ))
            }
            terminal_signal::Query::TerminalDetachment(detachment) => Ok(
                terminal_signal::Response::TerminalDetached(terminal_signal::TerminalDetachedReply {
                    terminal: detachment.terminal,
                    generation: GENERATION,
                    terminal_detachment_reason: detachment.terminal_detachment_reason,
                }),
            ),
            terminal_signal::Query::TerminalCapture(capture) => {
                let snapshot = self.snapshot().await?;
                Ok(terminal_signal::Response::TerminalCaptured(
                    terminal_signal::TerminalCapturedReply {
                        terminal: capture.terminal,
                        generation: GENERATION,
                        transcript_bytes: widen_bytes(snapshot.bytes()),
                    },
                ))
            }
            terminal_signal::Query::RegisterPromptPattern(registration) => {
                let pattern_id = self.register_prompt_pattern(registration.pattern);
                Ok(terminal_signal::Response::PromptPatternRegistered(
                    terminal_signal::PromptPatternRegisteredReply {
                        terminal: registration.terminal,
                        pattern_identifier: pattern_id,
                    },
                ))
            }
            terminal_signal::Query::UnregisterPromptPattern(unregistration) => {
                self.prompt_patterns
                    .remove(unregistration.pattern_identifier.as_str());
                Ok(terminal_signal::Response::PromptPatternUnregistered(
                    terminal_signal::PromptPatternUnregisteredReply {
                        terminal: unregistration.terminal,
                        pattern_identifier: unregistration.pattern_identifier,
                    },
                ))
            }
            terminal_signal::Query::ListPromptPatterns(list) => Ok(
                terminal_signal::Response::PromptPatternList(
                    terminal_signal::PromptPatternListReply {
                        terminal: list.terminal,
                        entries: self.prompt_pattern_entries(),
                    },
                ),
            ),
            terminal_signal::Query::AcquireInputGate(acquire) => {
                self.acquire_input_gate(acquire).await
            }
            terminal_signal::Query::ReleaseInputGate(release) => self.release_input_gate(release),
            terminal_signal::Query::WriteInjection(injection) => {
                self.write_injection(injection).await
            }
            terminal_signal::Query::SubscribeTerminalWorkerLifecycle(subscription) => {
                self.open_worker_lifecycle_subscription(subscription).await
            }
            terminal_signal::Query::TerminalWorkerLifecycleRetraction(token) => {
                Ok(self.close_worker_lifecycle_subscription(token))
            }
            terminal_signal::Query::ListSessions(_)
            | terminal_signal::Query::ResolveSession(_) => {
                Err(TerminalSignalControlFailure::new(
                    "session registry queries belong to the consolidated terminal daemon",
                ))
            }
        }
    }

    async fn open_worker_lifecycle_subscription(
        &mut self,
        subscription: terminal_signal::SubscribeTerminalWorkerLifecycleRequest,
    ) -> Result<terminal_signal::Response, TerminalSignalControlFailure> {
        let terminal = subscription.terminal;
        let token = terminal_signal::TerminalWorkerLifecycleToken {
            terminal: terminal.clone(),
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
            .collect::<Vec<_>>();
        Ok(terminal_signal::Response::TerminalWorkerLifecycleSnapshot(
            terminal_signal::TerminalWorkerLifecycleSnapshotReply {
                terminal,
                observations,
            },
        ))
    }

    fn close_worker_lifecycle_subscription(
        &mut self,
        token: terminal_signal::TerminalWorkerLifecycleToken,
    ) -> terminal_signal::Response {
        let position = self
            .lifecycle_subscriptions
            .iter()
            .position(|existing| existing == &token);
        match position {
            Some(index) => {
                self.lifecycle_subscriptions.remove(index);
                terminal_signal::Response::SubscriptionRetracted(
                    terminal_signal::SubscriptionRetractedReply { token },
                )
            }
            None => terminal_signal::Response::TerminalRejected(
                terminal_signal::TerminalRejectedReply {
                    terminal: token.terminal,
                    terminal_rejection_reason:
                        terminal_signal::TerminalRejectionReason::NotConnected,
                },
            ),
        }
    }

    fn register_prompt_pattern(
        &mut self,
        pattern: terminal_signal::PromptPattern,
    ) -> terminal_signal::PromptPatternIdentifier {
        let pattern_id = format!("prompt-pattern-{}", self.next_prompt_pattern);
        self.next_prompt_pattern = self.next_prompt_pattern.saturating_add(1);
        self.prompt_patterns.insert(pattern_id.clone(), pattern);
        pattern_id
    }

    fn prompt_pattern_entries(&self) -> Vec<terminal_signal::PromptPatternEntry> {
        self.prompt_patterns
            .iter()
            .map(
                |(pattern_id, pattern)| terminal_signal::PromptPatternEntry {
                    pattern_identifier: pattern_id.clone(),
                    pattern: pattern.clone(),
                },
            )
            .collect()
    }

    async fn acquire_input_gate(
        &mut self,
        acquire: terminal_signal::AcquireInputGateRequest,
    ) -> Result<terminal_signal::Response, TerminalSignalControlFailure> {
        let prompt_state = self
            .prompt_state(acquire.prompt_pattern_identifier_selection.as_ref())
            .await?;
        match self.input_port.close_human_input() {
            Ok(lease) => {
                let signal_lease = Self::signal_lease(lease);
                self.signal_leases
                    .insert(signal_lease.input_gate_lease_identifier, prompt_state.clone());
                Ok(terminal_signal::Response::GateAcquired(
                    terminal_signal::GateAcquiredReply {
                        terminal: acquire.terminal,
                        lease: signal_lease,
                        prompt_state,
                    },
                ))
            }
            Err(TerminalCellError::InputGateAlreadyClosed(lease)) => Ok(
                terminal_signal::Response::GateBusy(terminal_signal::GateBusyReply {
                    terminal: acquire.terminal,
                    current_holder: lease.sequence().into_u64() as i64,
                }),
            ),
            Err(error) => Err(TerminalSignalControlFailure::from_terminal_cell(error)),
        }
    }

    fn release_input_gate(
        &mut self,
        release: terminal_signal::ReleaseInputGateRequest,
    ) -> Result<terminal_signal::Response, TerminalSignalControlFailure> {
        let lease_key = release.lease.input_gate_lease_identifier;
        if !self.signal_leases.contains_key(&lease_key) {
            return Ok(terminal_signal::Response::InjectionRejected(
                terminal_signal::InjectionRejectedReply {
                    terminal: release.terminal,
                    injection_rejection_reason:
                        terminal_signal::InjectionRejectionReason::UnknownLease,
                },
            ));
        }

        let terminal_lease = Self::terminal_lease(&release.lease);
        match self.input_port.open_human_input(terminal_lease) {
            Ok(gate_release) => {
                self.signal_leases.remove(&lease_key);
                Ok(terminal_signal::Response::GateReleased(
                    terminal_signal::GateReleasedReply {
                        terminal: release.terminal,
                        lease: release.lease,
                        cached_human_bytes: gate_release.held_byte_count() as i64,
                    },
                ))
            }
            Err(TerminalCellError::StaleInputGateLease) => {
                self.signal_leases.remove(&lease_key);
                Ok(terminal_signal::Response::InjectionRejected(
                    terminal_signal::InjectionRejectedReply {
                        terminal: release.terminal,
                        injection_rejection_reason:
                            terminal_signal::InjectionRejectionReason::UnknownLease,
                    },
                ))
            }
            Err(error) => Err(TerminalSignalControlFailure::from_terminal_cell(error)),
        }
    }

    async fn write_injection(
        &mut self,
        injection: terminal_signal::WriteInjectionRequest,
    ) -> Result<terminal_signal::Response, TerminalSignalControlFailure> {
        let lease_key = injection.lease.input_gate_lease_identifier;
        let Some(prompt_state) = self.signal_leases.get(&lease_key) else {
            return Ok(terminal_signal::Response::InjectionRejected(
                terminal_signal::InjectionRejectedReply {
                    terminal: injection.terminal,
                    injection_rejection_reason:
                        terminal_signal::InjectionRejectionReason::UnknownLease,
                },
            ));
        };

        if matches!(prompt_state, terminal_signal::PromptState::Dirty(_)) {
            return Ok(terminal_signal::Response::InjectionRejected(
                terminal_signal::InjectionRejectedReply {
                    terminal: injection.terminal,
                    injection_rejection_reason:
                        terminal_signal::InjectionRejectionReason::DirtyPrompt,
                },
            ));
        }

        self.input_port
            .accept(TerminalInput::new(
                narrow_bytes(&injection.input_bytes),
                InputSource::Programmatic,
            ))
            .map_err(TerminalSignalControlFailure::from_terminal_cell)?;
        let snapshot = self.snapshot().await?;
        Ok(terminal_signal::Response::InjectionAck(
            terminal_signal::InjectionAckReply {
                terminal: injection.terminal,
                generation: GENERATION,
                sequence: snapshot.last_sequence().into_u64() as i64,
            },
        ))
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
                self.snapshot().await?.bytes().len() as i64,
            ));
        };
        let snapshot = self.snapshot().await?;
        let trailing_count = Self::prompt_suffix_trailing_count(pattern, snapshot.bytes())?;
        if trailing_count == 0 {
            Ok(terminal_signal::PromptState::Clean)
        } else {
            Ok(terminal_signal::PromptState::Dirty(trailing_count as i64))
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
                &narrow_bytes(suffix.as_slice()),
            )),
            terminal_signal::PromptPattern::RegexSuffix(pattern) => {
                let pattern = narrow_bytes(pattern.as_slice());
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
        terminal_signal::InputGateLease {
            input_gate_lease_identifier: lease.sequence().into_u64() as i64,
        }
    }

    fn terminal_lease(lease: &terminal_signal::Lease) -> TerminalInputGateLease {
        TerminalInputGateLease::new(TerminalInputGateSequence::new(
            lease.input_gate_lease_identifier as u64,
        ))
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
                terminal_signal::TerminalWorkerStopReason::InputWriteFailed(error)
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
    request: terminal_signal::Query,
}

impl TerminalSignalControlRequest {
    pub fn new(request: terminal_signal::Query) -> Self {
        Self { request }
    }
}

impl Message<TerminalSignalControlRequest> for TerminalSignalControl {
    type Reply = Result<terminal_signal::Response, TerminalSignalControlFailure>;

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
