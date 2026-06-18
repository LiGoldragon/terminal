use signal_terminal::{
    Input, InputBytes, Output, TerminalConnection, TerminalDetached, TerminalDetachment,
    TerminalDetachmentReason, TerminalGeneration, TerminalInput, TerminalInputAccepted,
    TerminalInputBytes, TerminalName, TerminalRejected, TerminalRejectionReason, TerminalSequence,
    TerminalTranscriptBytes, TranscriptBytes, TranscriptDelta,
};
use std::os::unix::net::UnixListener;
use std::thread;
use terminal::contract::TerminalTransportBinding;
use terminal::supervisor::TerminalSupervisorFrameCodec;

/// Widen a `u8` byte literal into the schema-emitted `Integer` (`u64`)
/// byte vector the signal-terminal contract carries on its byte-bearing
/// fields.
fn signal_bytes(bytes: &[u8]) -> Vec<u64> {
    bytes.iter().map(|byte| u64::from(*byte)).collect()
}

fn terminal_name() -> TerminalName {
    TerminalName::new("operator".to_string())
}

fn binding() -> TerminalTransportBinding {
    TerminalTransportBinding::from_socket_path(terminal_name(), "/tmp/terminal-test.sock")
}

fn unique_socket_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "terminal-{name}-{}-{}.sock",
        std::process::id(),
        thread::current().name().unwrap_or("test")
    ))
}

#[test]
fn terminal_contract_connection_returns_ready_event() {
    let mut binding = binding();
    let event = binding
        .handle_request(Input::TerminalConnection(TerminalConnection::new(
            terminal_name().into(),
        )))
        .expect("connection does not touch the socket");

    assert_eq!(event, binding.ready_event());
}

#[test]
fn terminal_contract_rejects_other_terminal_before_socket_io() {
    let mut binding = binding();
    let other_terminal = TerminalName::new("designer".to_string());
    let event = binding
        .handle_request(Input::TerminalInput(TerminalInput {
            terminal: other_terminal.clone().into(),
            input_bytes: InputBytes::new(TerminalInputBytes::new(signal_bytes(b"ignored"))),
        }))
        .expect("terminal mismatch is local");

    assert_eq!(
        event,
        Output::TerminalRejected(TerminalRejected {
            terminal: other_terminal.into(),
            terminal_rejection_reason: TerminalRejectionReason::NotConnected,
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
            Input::TerminalInput(input) => {
                assert_eq!(input.terminal, server_terminal.into());
                assert_eq!(
                    input.input_bytes.payload().payload().as_slice(),
                    signal_bytes(b"typed input").as_slice()
                );
                codec
                    .write_reply(
                        &mut stream,
                        Output::TerminalInputAccepted(TerminalInputAccepted {
                            terminal: input.terminal,
                            generation: TerminalGeneration::new(1).into(),
                        }),
                    )
                    .expect("signal reply writes");
            }
            other => panic!("expected TerminalInput request, got {other:?}"),
        }
    });

    let mut binding = TerminalTransportBinding::from_socket_path(terminal_name(), &socket_path);
    let event = binding
        .handle_request(Input::TerminalInput(TerminalInput {
            terminal: terminal_name().into(),
            input_bytes: TerminalInputBytes::new(signal_bytes(b"typed input")).into(),
        }))
        .expect("input request travels through Signal control plane");

    assert_eq!(
        event,
        Output::TerminalInputAccepted(TerminalInputAccepted {
            terminal: terminal_name().into(),
            generation: TerminalGeneration::new(1).into(),
        })
    );
    server.join().expect("test signal server exits");
    let _ = std::fs::remove_file(socket_path);
}

#[test]
fn terminal_contract_detachment_is_typed_event() {
    let mut binding = binding();
    let event = binding
        .handle_request(Input::TerminalDetachment(TerminalDetachment {
            terminal: terminal_name().into(),
            terminal_detachment_reason: TerminalDetachmentReason::HarnessStopped,
        }))
        .expect("detachment acknowledgement does not touch the socket");

    assert_eq!(
        event,
        Output::TerminalDetached(TerminalDetached {
            terminal: terminal_name().into(),
            generation: binding.generation().into(),
            terminal_detachment_reason: TerminalDetachmentReason::HarnessStopped,
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
        Output::TranscriptDelta(TranscriptDelta {
            terminal: terminal_name().into(),
            sequence: TerminalSequence::new(1).into(),
            transcript_bytes: TranscriptBytes::new(TerminalTranscriptBytes::new(signal_bytes(
                b"first",
            ))),
        })
    );
    assert_eq!(
        second,
        Output::TranscriptDelta(TranscriptDelta {
            terminal: terminal_name().into(),
            sequence: TerminalSequence::new(2).into(),
            transcript_bytes: TerminalTranscriptBytes::new(signal_bytes(b"second")).into(),
        })
    );
}
