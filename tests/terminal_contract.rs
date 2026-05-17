use persona_terminal::contract::TerminalTransportBinding;
use persona_terminal::supervisor::TerminalSupervisorFrameCodec;
use signal_persona_terminal::{
    TerminalConnection, TerminalDetached, TerminalDetachment, TerminalDetachmentReason,
    TerminalGeneration, TerminalInput, TerminalInputAccepted, TerminalInputBytes, TerminalName,
    TerminalRejected, TerminalRejectionReason, TerminalReply, TerminalRequest, TerminalSequence,
    TranscriptDelta,
};
use std::os::unix::net::UnixListener;
use std::thread;

fn terminal_name() -> TerminalName {
    TerminalName::new("operator")
}

fn binding() -> TerminalTransportBinding {
    TerminalTransportBinding::from_socket_path(terminal_name(), "/tmp/persona-terminal-test.sock")
}

fn unique_socket_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "persona-terminal-{name}-{}-{}.sock",
        std::process::id(),
        thread::current().name().unwrap_or("test")
    ))
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
fn terminal_contract_input_uses_signal_frame_control_plane() {
    let socket_path = unique_socket_path("signal-input");
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path).expect("test signal listener binds");
    let server_terminal = terminal_name();
    let server = thread::spawn(move || {
        let (mut stream, _address) = listener.accept().expect("signal client connects");
        let codec = TerminalSupervisorFrameCodec::default();
        let request = codec
            .read_request(&mut stream)
            .expect("binding writes a Signal request frame");
        match request {
            TerminalRequest::TerminalInput(input) => {
                assert_eq!(input.terminal, server_terminal);
                assert_eq!(input.bytes.as_slice(), b"typed input");
                codec
                    .write_reply(
                        &mut stream,
                        TerminalReply::TerminalInputAccepted(TerminalInputAccepted {
                            terminal: input.terminal,
                            generation: TerminalGeneration::new(1),
                        }),
                    )
                    .expect("signal reply writes");
            }
            other => panic!("expected TerminalInput request, got {other:?}"),
        }
    });

    let mut binding = TerminalTransportBinding::from_socket_path(terminal_name(), &socket_path);
    let event = binding
        .handle_request(TerminalRequest::TerminalInput(TerminalInput {
            terminal: terminal_name(),
            bytes: TerminalInputBytes::new(b"typed input".to_vec()),
        }))
        .expect("input request travels through Signal control plane");

    assert_eq!(
        event,
        TerminalReply::TerminalInputAccepted(TerminalInputAccepted {
            terminal: terminal_name(),
            generation: TerminalGeneration::new(1),
        })
    );
    server.join().expect("test signal server exits");
    let _ = std::fs::remove_file(socket_path);
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
