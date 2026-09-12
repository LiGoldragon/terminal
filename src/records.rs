//! Terminal's own durable observation records.
//!
//! These are the component's internal Sema state, not a wire contract. They
//! were borrowed from `signal-terminal` until that contract was rewritten
//! onto the Datom stack and dropped them: `signal-terminal` 2.0.1 carries
//! only what crosses the component boundary, and an audit row that never
//! leaves terminal's own store does not. They are declared here because the
//! component that stores them is the component that owns them.
//!
//! They are hand-written rather than ethos-declared because `sema-engine`'s
//! `EngineStoredValue` requires the rkyv surface, and `ethos-zero` 8.0.1
//! emits rkyv derives only for a `Signal` root — a `Library` or `Sema` root
//! receives datom-codec derives alone. No `Datomic` implementation is
//! hand-written here: these types are not ethos-declared at all.

use signal_terminal::{Response, TerminalGeneration, TerminalName, TerminalOperationKind, WirePath};

/// Monotonic position of an observation within its table.
pub type TerminalObservationSequence = i64;

/// Where a named session stands in its lifecycle.
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalSessionState {
    Ready,
    Draining,
    Closed,
}

/// Where one delivery attempt stands.
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalDeliveryAttemptState {
    Started,
    Delivered,
    Rejected,
}

/// Where one viewer attachment stands.
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalViewerAttachmentState {
    Attached,
    Detached,
    Replaced,
}

/// Why a session left the live registry.
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalSessionArchiveState {
    Archived,
    Purged,
}

/// One named session: its two socket planes and its lifecycle position.
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, PartialEq)]
pub struct TerminalSessionObservation {
    pub terminal: TerminalName,
    pub control_socket_path: WirePath,
    pub data_socket_path: WirePath,
    pub state: TerminalSessionState,
    pub generation: TerminalGeneration,
    pub transcript_sequence: i64,
}

impl TerminalSessionObservation {
    /// A session that has bound both planes and is serving.
    pub fn ready(
        terminal: TerminalName,
        control_socket_path: impl Into<WirePath>,
        data_socket_path: impl Into<WirePath>,
    ) -> Self {
        Self {
            terminal,
            control_socket_path: control_socket_path.into(),
            data_socket_path: data_socket_path.into(),
            state: TerminalSessionState::Ready,
            generation: 1,
            transcript_sequence: 0,
        }
    }

    pub fn terminal(&self) -> &TerminalName {
        &self.terminal
    }

    pub fn control_socket_path(&self) -> &str {
        self.control_socket_path.as_str()
    }

    pub fn data_socket_path(&self) -> &str {
        self.data_socket_path.as_str()
    }

    pub fn state(&self) -> TerminalSessionState {
        self.state
    }

    pub fn generation(&self) -> TerminalGeneration {
        self.generation
    }

    pub fn transcript_sequence(&self) -> i64 {
        self.transcript_sequence
    }
}

/// The health a session reported at its last observation.
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, PartialEq)]
pub struct TerminalSessionHealthObservation {
    pub terminal: TerminalName,
    pub state: TerminalSessionState,
    pub generation: TerminalGeneration,
}

impl TerminalSessionHealthObservation {
    pub fn new(
        terminal: TerminalName,
        state: TerminalSessionState,
        generation: TerminalGeneration,
    ) -> Self {
        Self {
            terminal,
            state,
            generation,
        }
    }

    pub fn terminal(&self) -> &TerminalName {
        &self.terminal
    }

    pub fn state(&self) -> TerminalSessionState {
        self.state
    }

    pub fn generation(&self) -> TerminalGeneration {
        self.generation
    }
}

/// One attempt to deliver a request to a named session.
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, PartialEq)]
pub struct TerminalDeliveryAttemptObservation {
    pub sequence: TerminalObservationSequence,
    pub terminal: TerminalName,
    pub operation: TerminalOperationKind,
    pub state: TerminalDeliveryAttemptState,
}

impl TerminalDeliveryAttemptObservation {
    /// An attempt recorded at the moment it was dispatched.
    pub fn started(
        sequence: TerminalObservationSequence,
        terminal: TerminalName,
        operation: TerminalOperationKind,
    ) -> Self {
        Self {
            sequence,
            terminal,
            operation,
            state: TerminalDeliveryAttemptState::Started,
        }
    }

    pub fn sequence(&self) -> TerminalObservationSequence {
        self.sequence
    }

    pub fn terminal(&self) -> &TerminalName {
        &self.terminal
    }

    pub fn operation(&self) -> &TerminalOperationKind {
        &self.operation
    }

    pub fn state(&self) -> TerminalDeliveryAttemptState {
        self.state
    }
}

/// One reply the owner observed and recorded for a named session.
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, PartialEq)]
pub struct TerminalEventObservation {
    pub sequence: TerminalObservationSequence,
    pub terminal: TerminalName,
    pub event: Response,
}

impl TerminalEventObservation {
    pub fn new(
        sequence: TerminalObservationSequence,
        terminal: TerminalName,
        event: Response,
    ) -> Self {
        Self {
            sequence,
            terminal,
            event,
        }
    }

    pub fn sequence(&self) -> TerminalObservationSequence {
        self.sequence
    }

    pub fn terminal(&self) -> &TerminalName {
        &self.terminal
    }

    pub fn event(&self) -> &Response {
        &self.event
    }
}

/// One viewer attaching to, or leaving, a named session's data plane.
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, PartialEq)]
pub struct TerminalViewerAttachmentObservation {
    pub sequence: TerminalObservationSequence,
    pub terminal: TerminalName,
    pub viewer: String,
    pub state: TerminalViewerAttachmentState,
}

impl TerminalViewerAttachmentObservation {
    pub fn new(
        sequence: TerminalObservationSequence,
        terminal: TerminalName,
        viewer: impl Into<String>,
        state: TerminalViewerAttachmentState,
    ) -> Self {
        Self {
            sequence,
            terminal,
            viewer: viewer.into(),
            state,
        }
    }

    pub fn sequence(&self) -> TerminalObservationSequence {
        self.sequence
    }

    pub fn terminal(&self) -> &TerminalName {
        &self.terminal
    }

    pub fn viewer(&self) -> &str {
        self.viewer.as_str()
    }

    pub fn state(&self) -> TerminalViewerAttachmentState {
        self.state
    }
}

/// A session that has left the live registry, and why.
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, PartialEq)]
pub struct TerminalSessionArchiveObservation {
    pub terminal: TerminalName,
    pub reason: String,
    pub state: TerminalSessionArchiveState,
}

impl TerminalSessionArchiveObservation {
    /// A session retired in the ordinary way, carrying the reason recorded
    /// at retirement.
    pub fn archived(terminal: TerminalName, reason: impl Into<String>) -> Self {
        Self {
            terminal,
            reason: reason.into(),
            state: TerminalSessionArchiveState::Archived,
        }
    }

    pub fn terminal(&self) -> &TerminalName {
        &self.terminal
    }

    pub fn reason(&self) -> &str {
        self.reason.as_str()
    }

    pub fn state(&self) -> TerminalSessionArchiveState {
        self.state
    }
}
