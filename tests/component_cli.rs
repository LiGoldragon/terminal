#![cfg(feature = "nota-text")]

use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use meta_signal_terminal::{
    MetaTerminalFrame, MetaTerminalFrameBody, MetaTerminalReply, MetaTerminalRequest,
    RetireSession, SessionRetired,
};
use nota_next::NotaEncode;
use signal_frame::{NonEmpty, Reply, SubReply};
use signal_terminal::{Frame, FrameBody, Input, Output, TerminalConnection, TerminalName};
use triad_runtime::{FrameBody as RuntimeFrameBody, LengthPrefixedCodec};

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
        let (exchange, request) = TerminalCliServer::read_request(&mut stream);
        assert_eq!(
            request,
            Input::TerminalConnection(TerminalConnection::new(
                TerminalName::new("operator".to_string()).into()
            ))
        );
        TerminalCliServer::write_reply(
            &mut stream,
            exchange,
            Output::TerminalReady(signal_terminal::TerminalReady {
                terminal: TerminalName::new("operator".to_string()).into(),
                generation: signal_terminal::TerminalGeneration::new(1).into(),
            }),
        );
    });

    let request = Input::TerminalConnection(TerminalConnection::new(
        TerminalName::new("operator".to_string()).into(),
    ))
    .to_string();
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
        let (exchange, request) = MetaTerminalCliServer::read_request(&mut stream);
        assert_eq!(
            request,
            MetaTerminalRequest::RetireSession(RetireSession {
                name: TerminalName::new("operator".to_string()),
            })
        );
        MetaTerminalCliServer::write_reply(
            &mut stream,
            exchange,
            MetaTerminalReply::SessionRetired(SessionRetired {
                name: TerminalName::new("operator".to_string()),
                exit_status: None,
            }),
        );
    });

    let request = MetaTerminalRequest::RetireSession(RetireSession {
        name: TerminalName::new("operator".to_string()),
    })
    .to_nota();
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

#[derive(Debug)]
struct TerminalCliServer;

impl TerminalCliServer {
    fn read_request(stream: &mut UnixStream) -> (signal_frame::ExchangeIdentifier, Input) {
        let body = RuntimeFrame::read(stream);
        match Frame::decode(body.bytes())
            .expect("decode terminal signal frame")
            .into_body()
        {
            FrameBody::Request { exchange, request } => {
                let (payload, tail) = request.payloads.into_head_and_tail();
                assert!(tail.is_empty(), "terminal cli should send one payload");
                (exchange, payload)
            }
            other => panic!("expected terminal request frame, got {other:?}"),
        }
    }

    fn write_reply(
        stream: &mut UnixStream,
        exchange: signal_frame::ExchangeIdentifier,
        output: Output,
    ) {
        let frame = Frame::new(FrameBody::Reply {
            exchange,
            reply: Reply::committed(NonEmpty::single(SubReply::Ok(output))),
        });
        RuntimeFrame::write(stream, frame.encode().expect("encode terminal reply"));
    }
}

#[derive(Debug)]
struct MetaTerminalCliServer;

impl MetaTerminalCliServer {
    fn read_request(
        stream: &mut UnixStream,
    ) -> (signal_frame::ExchangeIdentifier, MetaTerminalRequest) {
        let body = RuntimeFrame::read(stream);
        match MetaTerminalFrame::decode(body.bytes())
            .expect("decode meta-terminal signal frame")
            .into_body()
        {
            MetaTerminalFrameBody::Request { exchange, request } => {
                let (payload, tail) = request.payloads.into_head_and_tail();
                assert!(tail.is_empty(), "meta-terminal cli should send one payload");
                (exchange, payload)
            }
            other => panic!("expected meta-terminal request frame, got {other:?}"),
        }
    }

    fn write_reply(
        stream: &mut UnixStream,
        exchange: signal_frame::ExchangeIdentifier,
        reply: MetaTerminalReply,
    ) {
        let frame = MetaTerminalFrame::new(MetaTerminalFrameBody::Reply {
            exchange,
            reply: Reply::committed(NonEmpty::single(SubReply::Ok(reply))),
        });
        RuntimeFrame::write(stream, frame.encode().expect("encode meta-terminal reply"));
    }
}

#[derive(Debug)]
struct RuntimeFrame;

impl RuntimeFrame {
    fn read(stream: &mut UnixStream) -> RuntimeFrameBody {
        LengthPrefixedCodec::default()
            .read_body(stream)
            .expect("read runtime frame body")
    }

    fn write(stream: &mut UnixStream, bytes: Vec<u8>) {
        LengthPrefixedCodec::default()
            .write_body(stream, &RuntimeFrameBody::new(bytes))
            .expect("write runtime frame body");
    }
}
