use std::collections::VecDeque;
use std::env;
use std::fs;
use std::io::{Read, Write, stdin, stdout};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, queue};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonRequest {
    socket: PathBuf,
    command: Vec<String>,
}

impl DaemonRequest {
    pub fn from_environment() -> Self {
        let mut arguments = env::args().skip(1);
        let socket = arguments
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp/persona-wezterm.sock"));
        let command = arguments.collect::<Vec<_>>();
        let command = if command.is_empty() {
            vec![env::var("SHELL").unwrap_or_else(|_| "bash".to_string())]
        } else {
            command
        };
        Self { socket, command }
    }

    pub fn run(self) -> Result<()> {
        if let Some(parent) = self.socket.parent() {
            fs::create_dir_all(parent)?;
        }
        let _ = fs::remove_file(&self.socket);
        let listener = UnixListener::bind(&self.socket)?;
        let session = PtySession::spawn(self.command)?;
        eprintln!("persona-wezterm-daemon socket={}", self.socket.display());
        session.accept_clients(listener)
    }
}

struct PtySession {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    _child: Box<dyn Child + Send + Sync>,
    clients: Clients,
    scrollback: Scrollback,
}

impl PtySession {
    fn spawn(command: Vec<String>) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 32,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(PtyError::into_io_error)?;
        let mut builder = CommandBuilder::new(&command[0]);
        for argument in command.iter().skip(1) {
            builder.arg(argument);
        }
        builder.env("TERM", "xterm-256color");
        builder.env("COLORTERM", "truecolor");
        builder.env("CLICOLOR", "1");
        builder.env("FORCE_COLOR", "1");
        builder.env_remove("NO_COLOR");
        let child = pair
            .slave
            .spawn_command(builder)
            .map_err(PtyError::into_io_error)?;
        if let Some(pid) = child.process_id() {
            eprintln!("persona-wezterm-daemon child_pid={pid}");
        }
        drop(pair.slave);

        let clients = Clients::default();
        let scrollback = Scrollback::default();
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(PtyError::into_io_error)?;
        clients.clone().broadcast_from(reader, scrollback.clone());

        Ok(Self {
            master: Arc::new(Mutex::new(pair.master)),
            _child: child,
            clients,
            scrollback,
        })
    }

    fn accept_clients(self, listener: UnixListener) -> Result<()> {
        let writer = Arc::new(Mutex::new(
            self.master
                .lock()
                .expect("pty master lock")
                .take_writer()
                .map_err(PtyError::into_io_error)?,
        ));
        for stream in listener.incoming() {
            let mut stream = stream?;
            let handshake = ClientHandshake::read_from(&mut stream)?;
            if handshake.replay_scrollback() && self.scrollback.write_to(&stream).is_err() {
                continue;
            }
            let Ok(client_stream) = stream.try_clone() else {
                continue;
            };
            self.clients.add(client_stream);
            ClientInput {
                stream,
                first_tag: handshake.into_first_tag(),
                writer: writer.clone(),
                master: self.master.clone(),
            }
            .spawn();
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
struct Clients {
    inner: Arc<Mutex<Vec<UnixStream>>>,
}

impl Clients {
    fn add(&self, stream: UnixStream) {
        self.inner.lock().expect("clients lock").push(stream);
    }

    fn broadcast_from(&self, mut reader: Box<dyn Read + Send>, scrollback: Scrollback) {
        let clients = self.clone();
        thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                let Ok(count) = reader.read(&mut buffer) else {
                    break;
                };
                if count == 0 {
                    break;
                }
                let bytes = &buffer[..count];
                scrollback.push(bytes);
                clients.write_all(bytes);
            }
        });
    }

    fn write_all(&self, bytes: &[u8]) {
        self.inner
            .lock()
            .expect("clients lock")
            .retain_mut(|stream| stream.write_all(bytes).is_ok() && stream.flush().is_ok());
    }
}

#[derive(Clone)]
struct Scrollback {
    bytes: Arc<Mutex<VecDeque<u8>>>,
    limit: usize,
}

impl Default for Scrollback {
    fn default() -> Self {
        Self {
            bytes: Arc::new(Mutex::new(VecDeque::new())),
            limit: 8 * 1024 * 1024,
        }
    }
}

impl Scrollback {
    fn push(&self, incoming: &[u8]) {
        let mut bytes = self.bytes.lock().expect("scrollback lock");
        bytes.extend(incoming.iter().copied());
        while bytes.len() > self.limit {
            bytes.pop_front();
        }
    }

    fn write_to(&self, mut stream: &UnixStream) -> std::io::Result<()> {
        let bytes = self.bytes.lock().expect("scrollback lock");
        let contiguous = bytes.iter().copied().collect::<Vec<_>>();
        stream.write_all(&contiguous)?;
        stream.flush()
    }
}

struct ClientInput {
    stream: UnixStream,
    first_tag: Option<u8>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
}

impl ClientInput {
    fn spawn(mut self) {
        thread::spawn(move || {
            while let Ok(frame) = self.read_frame() {
                match frame {
                    ClientFrame::Input(bytes) => {
                        let mut writer = self.writer.lock().expect("pty writer lock");
                        let _ = writer.write_all(&bytes);
                        let _ = writer.flush();
                    }
                    ClientFrame::Resize(size) => {
                        eprintln!(
                            "persona-wezterm-daemon resize rows={} cols={}",
                            size.rows, size.cols
                        );
                        let _ = self.master.lock().expect("pty master lock").resize(size);
                    }
                }
            }
        });
    }

    fn read_frame(&mut self) -> std::io::Result<ClientFrame> {
        match self.first_tag.take() {
            Some(tag) => ClientFrame::read_from_tag(tag, &mut self.stream),
            None => ClientFrame::read_from(&mut self.stream),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewerRequest {
    socket: PathBuf,
    presentation: ViewerPresentation,
}

impl ViewerRequest {
    pub fn from_environment() -> Self {
        let socket = env::args()
            .nth(1)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp/persona-wezterm.sock"));
        let presentation = ViewerPresentation::from_environment();
        Self {
            socket,
            presentation,
        }
    }

    pub fn run(self) -> Result<()> {
        let mut stream = UnixStream::connect(&self.socket)?;
        ViewerHandshake::from_environment().write_to(&mut stream)?;
        let reader = stream.try_clone()?;
        TerminalSession::new(stream, reader, self.presentation).run()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerPresentation {
    Scrollback,
    Application,
}

impl ViewerPresentation {
    fn from_environment() -> Self {
        match env::var("PERSONA_WEZTERM_VIEW_MODE").as_deref() {
            Ok("application") => Self::Application,
            _ => Self::Scrollback,
        }
    }
}

struct TerminalSession {
    writer: Arc<Mutex<UnixStream>>,
    reader: UnixStream,
    presentation: ViewerPresentation,
}

impl TerminalSession {
    fn new(writer: UnixStream, reader: UnixStream, presentation: ViewerPresentation) -> Self {
        Self {
            writer: Arc::new(Mutex::new(writer)),
            reader,
            presentation,
        }
    }

    fn run(mut self) -> Result<()> {
        terminal::enable_raw_mode()?;
        self.presentation.enter()?;
        if let Ok(title) = env::var("PERSONA_WEZTERM_VIEW_TITLE") {
            write!(stdout(), "\x1b]0;{title}\x07")?;
            stdout().flush()?;
        }
        self.send_resize()?;
        self.spawn_output_thread();
        let done = Arc::new(AtomicBool::new(false));
        self.spawn_input_thread(done.clone());
        let result = self.forward_resize(done);
        let _ = self.presentation.leave();
        let _ = terminal::disable_raw_mode();
        result
    }

    fn spawn_output_thread(&self) {
        let mut reader = self.reader.try_clone().expect("viewer reader clones");
        thread::spawn(move || {
            let mut out = stdout();
            let mut buffer = [0_u8; 8192];
            loop {
                let Ok(count) = reader.read(&mut buffer) else {
                    break;
                };
                if count == 0 {
                    break;
                }
                let _ = out.write_all(&buffer[..count]);
                let _ = out.flush();
            }
        });
    }

    fn spawn_input_thread(&self, done: Arc<AtomicBool>) {
        let writer = self.writer.clone();
        thread::spawn(move || {
            let mut input = stdin();
            let mut buffer = [0_u8; 4096];
            loop {
                let Ok(count) = input.read(&mut buffer) else {
                    break;
                };
                if count == 0 {
                    break;
                }
                if buffer[..count].contains(&0x1d) {
                    done.store(true, Ordering::SeqCst);
                    break;
                }
                let mut stream = writer.lock().expect("viewer writer lock");
                if SendFrame::input(&buffer[..count])
                    .write_to(&mut *stream)
                    .is_err()
                {
                    break;
                }
            }
        });
    }

    fn forward_resize(&mut self, done: Arc<AtomicBool>) -> Result<()> {
        let mut last = terminal::size()?;
        while !done.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(250));
            let size = terminal::size()?;
            if size != last {
                self.send_resize_to(size.1, size.0)?;
                last = size;
            }
        }
        Ok(())
    }

    fn send_resize(&mut self) -> Result<()> {
        let (columns, rows) = terminal::size()?;
        self.send_resize_to(rows, columns)
    }

    fn send_resize_to(&mut self, rows: u16, columns: u16) -> Result<()> {
        let mut writer = self.writer.lock().expect("viewer writer lock");
        SendFrame::resize(rows, columns).write_to(&mut *writer)
    }
}

impl ViewerPresentation {
    fn enter(self) -> Result<()> {
        match self {
            Self::Scrollback => Ok(()),
            Self::Application => {
                queue!(stdout(), EnterAlternateScreen, EnableMouseCapture)?;
                stdout().flush()?;
                Ok(())
            }
        }
    }

    fn leave(self) -> Result<()> {
        match self {
            Self::Scrollback => Ok(()),
            Self::Application => {
                execute!(stdout(), DisableMouseCapture, LeaveAlternateScreen)?;
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendRequest {
    socket: PathBuf,
    text: Vec<u8>,
}

impl SendRequest {
    pub fn from_environment() -> Self {
        let mut arguments = env::args_os().skip(1);
        let socket = arguments
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp/persona-wezterm.sock"));
        let text = arguments
            .next()
            .map(|value| value.to_string_lossy().into_owned().into_bytes())
            .unwrap_or_default();
        Self { socket, text }
    }

    pub fn run(self) -> Result<()> {
        let mut stream = UnixStream::connect(&self.socket)?;
        stream.set_nonblocking(true)?;
        if !self.text.is_empty() {
            SendFrame::input(&self.text).write_to(&mut stream)?;
            thread::sleep(Duration::from_millis(100));
        }
        SendFrame::input(b"\r").write_to(&mut stream)?;
        let deadline = std::time::Instant::now() + Duration::from_millis(500);
        let mut buffer = [0_u8; 8192];
        while std::time::Instant::now() < deadline {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(20));
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

enum ClientFrame {
    Input(Vec<u8>),
    Resize(PtySize),
}

impl ClientFrame {
    fn read_from(reader: &mut impl Read) -> std::io::Result<Self> {
        let mut tag = [0_u8; 1];
        reader.read_exact(&mut tag)?;
        Self::read_from_tag(tag[0], reader)
    }

    fn read_from_tag(tag: u8, reader: &mut impl Read) -> std::io::Result<Self> {
        match tag {
            b'I' => {
                let mut length = [0_u8; 4];
                reader.read_exact(&mut length)?;
                let length = u32::from_be_bytes(length) as usize;
                let mut bytes = vec![0_u8; length];
                reader.read_exact(&mut bytes)?;
                Ok(Self::Input(bytes))
            }
            b'R' => {
                let mut payload = [0_u8; 4];
                reader.read_exact(&mut payload)?;
                Ok(Self::Resize(PtySize {
                    rows: u16::from_be_bytes([payload[0], payload[1]]),
                    cols: u16::from_be_bytes([payload[2], payload[3]]),
                    pixel_width: 0,
                    pixel_height: 0,
                }))
            }
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unknown persona wezterm client frame",
            )),
        }
    }
}

struct ClientHandshake {
    replay: bool,
    first_tag: Option<u8>,
}

impl ClientHandshake {
    fn read_from(stream: &mut UnixStream) -> std::io::Result<Self> {
        stream.set_read_timeout(Some(Duration::from_millis(50)))?;
        let mut tag = [0_u8; 1];
        let read = stream.read_exact(&mut tag);
        stream.set_read_timeout(None)?;
        match read {
            Ok(()) if tag[0] == b'H' => {
                let mut mode = [0_u8; 1];
                stream.read_exact(&mut mode)?;
                Ok(Self {
                    replay: mode[0] == b'R',
                    first_tag: None,
                })
            }
            Ok(()) => Ok(Self {
                replay: true,
                first_tag: Some(tag[0]),
            }),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                Ok(Self {
                    replay: true,
                    first_tag: None,
                })
            }
            Err(error) => Err(error),
        }
    }

    fn replay_scrollback(&self) -> bool {
        self.replay
    }

    fn into_first_tag(self) -> Option<u8> {
        self.first_tag
    }
}

struct ViewerHandshake {
    enabled: bool,
    replay: bool,
}

impl ViewerHandshake {
    fn from_environment() -> Self {
        let enabled = matches!(
            env::var("PERSONA_WEZTERM_HANDSHAKE").as_deref(),
            Ok("1" | "true")
        );
        let replay = matches!(
            env::var("PERSONA_WEZTERM_REPLAY").as_deref(),
            Ok("1" | "true")
        );
        Self { enabled, replay }
    }

    fn write_to(&self, writer: &mut impl Write) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        writer.write_all(&[b'H'])?;
        writer.write_all(&[if self.replay { b'R' } else { b'N' }])?;
        writer.flush()?;
        Ok(())
    }
}

enum SendFrame<'bytes> {
    Input(&'bytes [u8]),
    Resize { rows: u16, columns: u16 },
}

impl<'bytes> SendFrame<'bytes> {
    fn input(bytes: &'bytes [u8]) -> Self {
        Self::Input(bytes)
    }

    fn resize(rows: u16, columns: u16) -> Self {
        Self::Resize { rows, columns }
    }

    fn write_to(&self, writer: &mut impl Write) -> Result<()> {
        match self {
            Self::Input(bytes) => {
                writer.write_all(&[b'I'])?;
                writer.write_all(&((*bytes).len() as u32).to_be_bytes())?;
                writer.write_all(bytes)?;
            }
            Self::Resize { rows, columns } => {
                writer.write_all(&[b'R'])?;
                writer.write_all(&rows.to_be_bytes())?;
                writer.write_all(&columns.to_be_bytes())?;
            }
        }
        writer.flush()?;
        Ok(())
    }
}

struct PtyError;

impl PtyError {
    fn into_io_error(error: impl std::fmt::Display) -> std::io::Error {
        std::io::Error::other(error.to_string())
    }
}
