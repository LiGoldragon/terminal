use std::fs;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use signal_terminal::{Output, TerminalConnection, TerminalName, TerminalReady};
use terminal::capture_validator::CaptureValidatorCommandLine;
use terminal::signal_cli::{TerminalSignalOperation, TerminalSignalRequest};
use terminal::supervisor::TerminalSupervisorFrameCodec;

struct SignalFixture {
    root: PathBuf,
}

impl SignalFixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pt-sig-{name}-{}-{}",
            std::process::id(),
            Self::stamp()
        ));
        fs::create_dir_all(&root).expect("signal fixture directory is created");
        Self { root }
    }

    fn stamp() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time is after epoch")
            .as_nanos()
    }

    fn socket(&self) -> PathBuf {
        self.root.join("signal.sock")
    }
}

impl Drop for SignalFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn terminal_capture_validator_decodes_terminal_captured_tsv() {
    let fixture = SignalFixture::new("capture-validator");
    let capture_path = fixture.root.join("capture.tsv");
    fs::write(
        &capture_path,
        "TerminalCaptured\tresponder\t1\t6669787475726520726573706f6e6465722074657874\n",
    )
    .expect("capture fixture writes");

    CaptureValidatorCommandLine::from_arguments([
        "--file",
        capture_path.to_str().expect("path is utf8"),
        "--terminal",
        "responder",
        "--contains-text",
        "responder text",
    ])
    .run()
    .expect("validator decodes terminal capture bytes");
}

#[test]
fn terminal_signal_cli_connect_crosses_socket_signal_frame() {
    let fixture = SignalFixture::new("connect-crosses-socket");
    let listener = UnixListener::bind(fixture.socket()).expect("fake signal socket binds");
    let server = thread::spawn(move || {
        let (stream, _address) = listener.accept().expect("client connects");
        let mut stream = std::io::BufReader::new(stream);
        let codec = TerminalSupervisorFrameCodec::default();
        let request = codec
            .read_request(&mut stream)
            .expect("client writes signal request");
        assert_eq!(
            request,
            TerminalConnection::new(TerminalName::new("operator".to_string())).into()
        );
        let stream: &mut UnixStream = stream.get_mut();
        codec
            .write_event(
                stream,
                Output::from(TerminalReady {
                    terminal: TerminalName::new("operator".to_string()),
                    generation: signal_terminal::TerminalGeneration::new(1),
                }),
            )
            .expect("server writes signal event");
    });

    let request = TerminalSignalRequest::new(
        fixture.socket(),
        TerminalName::new("operator".to_string()),
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
    server.join().expect("fake server joins");
}
