use std::os::unix::net::UnixListener;
use std::thread;

use signal_terminal::{
    Query, Response, TerminalConnectionRequest, TerminalDetachedReply, TerminalDetachmentReason,
    TerminalDetachmentRequest, TerminalInputAcceptedReply, TerminalInputRequest, TerminalName,
    TerminalRejectedReply, TerminalRejectionReason, TranscriptDeltaReply,
};
use terminal::contract::{TerminalTransportBinding, widen_bytes};
use terminal::supervisor::TerminalSupervisorFrameCodec;

fn terminal_name() -> TerminalName {
    "operator".to_string()
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
        .handle_query(Query::TerminalConnection(TerminalConnectionRequest {
            terminal: terminal_name(),
        }))
        .expect("connection does not touch the socket");

    assert_eq!(event, binding.ready_event());
}

#[test]
fn terminal_contract_rejects_other_terminal_before_socket_io() {
    let mut binding = binding();
    let other_terminal = "designer".to_string();
    let event = binding
        .handle_query(Query::TerminalInput(TerminalInputRequest {
            terminal: other_terminal.clone(),
            input_bytes: widen_bytes(b"ignored"),
        }))
        .expect("terminal mismatch is local");

    assert_eq!(
        event,
        Response::TerminalRejected(TerminalRejectedReply {
            terminal: other_terminal,
            terminal_rejection_reason: TerminalRejectionReason::NotConnected,
        })
    );
}

#[test]
fn terminal_contract_input_crosses_the_signal_frame_control_plane() {
    let socket_path = unique_socket_path("signal-input");
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path).expect("test signal listener binds");
    let server_terminal = terminal_name();
    let server = thread::spawn(move || {
        let (mut stream, _address) = listener.accept().expect("signal client connects");
        let codec = TerminalSupervisorFrameCodec::new();
        let request = codec
            .read_request(&mut stream)
            .expect("binding writes a Signal request frame");
        match request {
            Query::TerminalInput(input) => {
                assert_eq!(input.terminal, server_terminal);
                assert_eq!(input.input_bytes, widen_bytes(b"typed input"));
                codec
                    .write_reply(
                        &mut stream,
                        &Response::TerminalInputAccepted(TerminalInputAcceptedReply {
                            terminal: input.terminal,
                            generation: 1,
                        }),
                    )
                    .expect("signal reply writes");
            }
            other => panic!("expected TerminalInput request, got {other:?}"),
        }
    });

    let mut binding = TerminalTransportBinding::from_socket_path(terminal_name(), &socket_path);
    let event = binding
        .handle_query(Query::TerminalInput(TerminalInputRequest {
            terminal: terminal_name(),
            input_bytes: widen_bytes(b"typed input"),
        }))
        .expect("input request travels through the Signal control plane");

    assert_eq!(
        event,
        Response::TerminalInputAccepted(TerminalInputAcceptedReply {
            terminal: terminal_name(),
            generation: 1,
        })
    );
    server.join().expect("test signal server exits");
    let _ = std::fs::remove_file(socket_path);
}

#[test]
fn terminal_contract_detachment_is_typed_event() {
    let mut binding = binding();
    let event = binding
        .handle_query(Query::TerminalDetachment(TerminalDetachmentRequest {
            terminal: terminal_name(),
            terminal_detachment_reason: TerminalDetachmentReason::HarnessStopped,
        }))
        .expect("detachment acknowledgement does not touch the socket");

    assert_eq!(
        event,
        Response::TerminalDetached(TerminalDetachedReply {
            terminal: terminal_name(),
            generation: binding.generation(),
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
        Response::TranscriptDelta(TranscriptDeltaReply {
            terminal: terminal_name(),
            sequence: 1,
            transcript_bytes: widen_bytes(b"first"),
        })
    );
    assert_eq!(
        second,
        Response::TranscriptDelta(TranscriptDeltaReply {
            terminal: terminal_name(),
            sequence: 2,
            transcript_bytes: widen_bytes(b"second"),
        })
    );
}

#[test]
fn terminal_contract_refuses_registry_queries() {
    let mut binding = binding();
    assert!(
        binding
            .handle_query(Query::ListSessions(signal_terminal::ListSessionsRequest {}))
            .is_err(),
        "the registry belongs to the consolidated daemon, not one terminal's transport"
    );
}
