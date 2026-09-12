//! End-to-end witness for the two component CLIs.
//!
//! Each CLI takes one inline Datom value, sends it as a `Signal` frame over
//! its socket, and prints the typed reply as Datom text. The fake server is
//! a real socket peer reading and writing the same frames the daemon does.
//!
//! This ran behind the retired `nota-text` feature and so never ran at all.
//! The feature is gone and the witness now runs on every build.

use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use meta_signal_terminal::{Query as MetaQuery, Response as MetaResponse, SessionRetired};
use signal_terminal::{Query, Response, TerminalConnectionRequest, TerminalReadyReply};
use terminal::{datom_text, frame};

#[derive(Debug)]
struct CliSocketFixture {
    root: PathBuf,
}

impl CliSocketFixture {
    fn new(name: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after Unix epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("terminal-cli-{name}-{}-{now}", std::process::id()));
        std::fs::create_dir_all(&root).expect("create terminal cli fixture directory");
        Self { root }
    }

    fn socket(&self) -> PathBuf {
        self.root.join("terminal.sock")
    }
}

impl Drop for CliSocketFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn terminal_cli_reaches_working_socket_and_prints_typed_reply() {
    let fixture = CliSocketFixture::new("working");
    let listener = UnixListener::bind(fixture.socket()).expect("fake terminal socket binds");
    let server = thread::spawn(move || {
        let (mut stream, _address) = listener.accept().expect("terminal cli connects");
        let request = read_terminal_query(&mut stream);
        assert_eq!(
            request,
            Query::TerminalConnection(TerminalConnectionRequest {
                terminal: "operator".to_string(),
            })
        );
        frame::terminal::write_response(
            &mut stream,
            &Response::TerminalReady(TerminalReadyReply {
                terminal: "operator".to_string(),
                generation: 1,
            }),
        )
        .expect("fake terminal server writes its reply");
    });

    let request = datom_text::textualize(&Query::TerminalConnection(TerminalConnectionRequest {
        terminal: "operator".to_string(),
    }));
    let output = Command::new(env!("CARGO_BIN_EXE_terminal"))
        .env("TERMINAL_SOCKET", fixture.socket())
        .arg(request)
        .output()
        .expect("run terminal cli");

    assert!(
        output.status.success(),
        "terminal cli failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("terminal cli stdout is utf8");
    assert!(
        stdout.contains("TerminalReady"),
        "unexpected stdout: {stdout}"
    );
    assert!(stdout.contains("operator"), "unexpected stdout: {stdout}");
    server.join().expect("fake terminal server exits");
}

#[test]
fn meta_terminal_cli_reaches_policy_socket_and_prints_typed_reply() {
    let fixture = CliSocketFixture::new("meta");
    let listener = UnixListener::bind(fixture.socket()).expect("fake meta-terminal socket binds");
    let server = thread::spawn(move || {
        let (mut stream, _address) = listener.accept().expect("meta-terminal cli connects");
        let request = read_meta_query(&mut stream);
        assert_eq!(request, MetaQuery::RetireSession("operator".to_string()));
        frame::meta::write_response(
            &mut stream,
            &MetaResponse::SessionRetired(SessionRetired {
                terminal_name: "operator".to_string(),
                selected_exit_status: None,
            }),
        )
        .expect("fake meta-terminal server writes its reply");
    });

    let request = datom_text::textualize(&MetaQuery::RetireSession("operator".to_string()));
    let output = Command::new(env!("CARGO_BIN_EXE_meta-terminal"))
        .env("TERMINAL_META_SOCKET", fixture.socket())
        .arg(request)
        .output()
        .expect("run meta-terminal cli");

    assert!(
        output.status.success(),
        "meta-terminal cli failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("meta-terminal cli stdout is utf8");
    assert!(
        stdout.contains("SessionRetired"),
        "unexpected stdout: {stdout}"
    );
    assert!(stdout.contains("operator"), "unexpected stdout: {stdout}");
    server.join().expect("fake meta-terminal server exits");
}

fn read_terminal_query(stream: &mut UnixStream) -> Query {
    frame::terminal::read_query(stream).expect("terminal cli sends one Signal query frame")
}

fn read_meta_query(stream: &mut UnixStream) -> MetaQuery {
    frame::meta::read_query(stream).expect("meta-terminal cli sends one Signal query frame")
}
