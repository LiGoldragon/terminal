//! Which operation a contract value names, and which terminal it concerns.
//!
//! The retired stack attached `operation_kind()` to the generated request
//! and reply enums. ethos-zero emits plain data — the type says what each
//! position holds and nothing more — so the projection from a value onto
//! its `TerminalOperationKind`, and onto the terminal it addresses, lives
//! here, beside the component that needs it.

use meta_signal_terminal::{
    MetaTerminalOperationKind, Query as MetaQuery, Response as MetaResponse,
};
use signal_terminal::{Query, Response, TerminalName, TerminalOperationKind};

/// The operation a `signal-terminal` query names.
pub fn query_operation_kind(query: &Query) -> TerminalOperationKind {
    match query {
        Query::TerminalConnection(_) => TerminalOperationKind::TerminalConnection,
        Query::TerminalInput(_) => TerminalOperationKind::TerminalInput,
        Query::TerminalResize(_) => TerminalOperationKind::TerminalResize,
        Query::TerminalDetachment(_) => TerminalOperationKind::TerminalDetachment,
        Query::TerminalCapture(_) => TerminalOperationKind::TerminalCapture,
        Query::RegisterPromptPattern(_) => TerminalOperationKind::RegisterPromptPattern,
        Query::UnregisterPromptPattern(_) => TerminalOperationKind::UnregisterPromptPattern,
        Query::ListPromptPatterns(_) => TerminalOperationKind::ListPromptPatterns,
        Query::AcquireInputGate(_) => TerminalOperationKind::AcquireInputGate,
        Query::ReleaseInputGate(_) => TerminalOperationKind::ReleaseInputGate,
        Query::WriteInjection(_) => TerminalOperationKind::WriteInjection,
        Query::SubscribeTerminalWorkerLifecycle(_) => {
            TerminalOperationKind::SubscribeTerminalWorkerLifecycle
        }
        Query::TerminalWorkerLifecycleRetraction(_) => {
            TerminalOperationKind::TerminalWorkerLifecycleRetraction
        }
        Query::ListSessions(_) => TerminalOperationKind::ListSessions,
        Query::ResolveSession(_) => TerminalOperationKind::ResolveSession,
    }
}

/// The terminal a `signal-terminal` query addresses.
///
/// `ListSessions` is a registry-wide query and names no terminal, so it
/// yields nothing rather than a placeholder name.
pub fn query_terminal(query: &Query) -> Option<&TerminalName> {
    let terminal = match query {
        Query::TerminalConnection(request) => &request.terminal,
        Query::TerminalInput(request) => &request.terminal,
        Query::TerminalResize(request) => &request.terminal,
        Query::TerminalDetachment(request) => &request.terminal,
        Query::TerminalCapture(request) => &request.terminal,
        Query::RegisterPromptPattern(request) => &request.terminal,
        Query::UnregisterPromptPattern(request) => &request.terminal,
        Query::ListPromptPatterns(request) => &request.terminal,
        Query::AcquireInputGate(request) => &request.terminal,
        Query::ReleaseInputGate(request) => &request.terminal,
        Query::WriteInjection(request) => &request.terminal,
        Query::SubscribeTerminalWorkerLifecycle(request) => &request.terminal,
        Query::TerminalWorkerLifecycleRetraction(token) => &token.terminal,
        Query::ResolveSession(request) => &request.name,
        Query::ListSessions(_) => return None,
    };
    Some(terminal)
}

/// The terminal a `signal-terminal` response concerns.
///
/// A session listing concerns every terminal and so names none; a streamed
/// event carries its terminal inside its own payload.
pub fn response_terminal(response: &Response) -> Option<&TerminalName> {
    let terminal = match response {
        Response::TerminalReady(reply) => &reply.terminal,
        Response::TerminalInputAccepted(reply) => &reply.terminal,
        Response::TranscriptDelta(reply) => &reply.terminal,
        Response::TerminalResized(reply) => &reply.terminal,
        Response::TerminalCaptured(reply) => &reply.terminal,
        Response::TerminalDetached(reply) => &reply.terminal,
        Response::TerminalExited(reply) => &reply.terminal,
        Response::TerminalRejected(reply) => &reply.terminal,
        Response::PromptPatternRegistered(reply) => &reply.terminal,
        Response::PromptPatternUnregistered(reply) => &reply.terminal,
        Response::PromptPatternList(reply) => &reply.terminal,
        Response::GateAcquired(reply) => &reply.terminal,
        Response::GateBusy(reply) => &reply.terminal,
        Response::GateReleased(reply) => &reply.terminal,
        Response::InjectionAck(reply) => &reply.terminal,
        Response::InjectionRejected(reply) => &reply.terminal,
        Response::TerminalWorkerLifecycleSnapshot(reply) => &reply.terminal,
        Response::SubscriptionRetracted(reply) => &reply.token.terminal,
        Response::SessionResolved(reply) => &reply.name,
        Response::Event(signal_terminal::TerminalEvent::TerminalWorkerLifecycleEvent(payload)) => {
            &payload.terminal
        }
        Response::SessionList(_) => return None,
    };
    Some(terminal)
}

/// The operation a `meta-signal-terminal` query names.
pub fn meta_query_operation_kind(query: &MetaQuery) -> MetaTerminalOperationKind {
    match query {
        MetaQuery::CreateSession(request) => {
            MetaTerminalOperationKind::CreateSession(request.clone())
        }
        MetaQuery::RetireSession(name) => MetaTerminalOperationKind::RetireSession(name.clone()),
    }
}

/// The terminal a `meta-signal-terminal` query addresses.
pub fn meta_query_terminal(query: &MetaQuery) -> &TerminalName {
    match query {
        MetaQuery::CreateSession(request) => &request.terminal_name,
        MetaQuery::RetireSession(name) => name,
    }
}

/// The terminal a `meta-signal-terminal` response concerns.
pub fn meta_response_terminal(response: &MetaResponse) -> &TerminalName {
    match response {
        MetaResponse::SessionCreated(reply) => &reply.terminal_name,
        MetaResponse::SessionRetired(reply) => &reply.terminal_name,
        MetaResponse::MetaTerminalRequestUnimplemented(reply) => &reply.terminal_name,
    }
}
