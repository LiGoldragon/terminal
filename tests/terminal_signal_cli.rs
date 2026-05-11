use persona_terminal::signal_cli::{TerminalSignalOperation, TerminalSignalRequest};
use signal_persona_terminal::TerminalName;

#[test]
fn terminal_signal_cli_connect_round_trips_request_and_event_frames() {
    let request = TerminalSignalRequest::new(
        "/tmp/persona-terminal-missing.sock",
        TerminalName::new("operator"),
        TerminalSignalOperation::Connect,
    );
    let mut output = Vec::new();

    request
        .run(&mut output)
        .expect("connect does not touch the terminal-cell socket");

    assert_eq!(
        String::from_utf8(output).expect("event line is utf8"),
        "TerminalReady\toperator\t1\n"
    );
}
