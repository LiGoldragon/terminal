use persona_terminal::contract::TerminalTransportBinding;
use signal_persona_terminal::{
    TerminalConnection, TerminalDetached, TerminalDetachment, TerminalDetachmentReason,
    TerminalInput, TerminalInputBytes, TerminalName, TerminalRejected, TerminalRejectionReason,
    TerminalReply, TerminalRequest, TerminalSequence, TranscriptDelta,
};

fn terminal_name() -> TerminalName {
    TerminalName::new("operator")
}

fn binding() -> TerminalTransportBinding {
    TerminalTransportBinding::from_socket_path(terminal_name(), "/tmp/persona-terminal-test.sock")
}

#[test]
fn terminal_contract_connection_returns_ready_event() {
    let mut binding = binding();
    let event = binding
        .handle_request(TerminalRequest::TerminalConnection(TerminalConnection {
            terminal: terminal_name(),
        }))
        .expect("connection does not touch the socket");

    assert_eq!(event, binding.ready_event());
}

#[test]
fn terminal_contract_rejects_other_terminal_before_socket_io() {
    let mut binding = binding();
    let other_terminal = TerminalName::new("designer");
    let event = binding
        .handle_request(TerminalRequest::TerminalInput(TerminalInput {
            terminal: other_terminal.clone(),
            bytes: TerminalInputBytes::new(b"ignored".to_vec()),
        }))
        .expect("terminal mismatch is local");

    assert_eq!(
        event,
        TerminalReply::TerminalRejected(TerminalRejected {
            terminal: other_terminal,
            reason: TerminalRejectionReason::NotConnected,
        })
    );
}

#[test]
fn terminal_contract_detachment_is_typed_event() {
    let mut binding = binding();
    let event = binding
        .handle_request(TerminalRequest::TerminalDetachment(TerminalDetachment {
            terminal: terminal_name(),
            reason: TerminalDetachmentReason::HarnessStopped,
        }))
        .expect("detachment acknowledgement does not touch the socket");

    assert_eq!(
        event,
        TerminalReply::TerminalDetached(TerminalDetached {
            terminal: terminal_name(),
            generation: binding.generation(),
            reason: TerminalDetachmentReason::HarnessStopped,
        })
    );
}

#[test]
fn terminal_contract_transcript_delta_increments_sequence() {
    let mut binding = binding();
    let first = binding.transcript_event(b"first".to_vec());
    let second = binding.transcript_event(b"second".to_vec());

    assert_eq!(
        first,
        TerminalReply::TranscriptDelta(TranscriptDelta {
            terminal: terminal_name(),
            sequence: TerminalSequence::new(1),
            bytes: signal_persona_terminal::TerminalTranscriptBytes::new(b"first".to_vec()),
        })
    );
    assert_eq!(
        second,
        TerminalReply::TranscriptDelta(TranscriptDelta {
            terminal: terminal_name(),
            sequence: TerminalSequence::new(2),
            bytes: signal_persona_terminal::TerminalTranscriptBytes::new(b"second".to_vec()),
        })
    );
}
