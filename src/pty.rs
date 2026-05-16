use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;

use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size as terminal_size};
use kameo::actor::{ActorRef, Spawn};
use signal_core::RequestPayload;
use signal_hook::consts::signal::SIGWINCH;
use signal_hook::iterator::Signals;
use signal_persona_terminal as terminal_signal;
use terminal_cell::{
    InputSource, SignalSocketRequest, SocketReplyWriter, SocketRequest, SocketRequestReader,
    TerminalCell, TerminalCellError, TerminalCellSocketClient, TerminalCommand, TerminalInput,
    TerminalInputGateLease, TerminalInputPort, TerminalLaunch, TerminalOutputPort, TerminalSize,
    TerminalViewerLease, TerminalWorkerKind, TerminalWorkerLifecycle,
    TerminalWorkerLifecycleSubscriptionRequest, TerminalWorkerObservationRequest,
    TerminalWorkerStop, TranscriptSnapshotRequest, TranscriptSubscriptionRequest,
    WaitForTerminalExit, WaitForTranscriptText,
};
use tokio::runtime::{Builder, Handle};

use crate::error::{Error, Result};
use crate::registry::SessionRegistration;
use crate::signal_control::{TerminalSignalControl, TerminalSignalControlRequest};
use crate::socket::SocketMode;
use crate::tables::StoreLocation;

const DEFAULT_CONTROL_SOCKET: &str = "/tmp/persona-terminal.control.sock";
const DEFAULT_DATA_SOCKET: &str = "/tmp/persona-terminal.data.sock";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonRequest {
    control_socket: PathBuf,
    data_socket: PathBuf,
    command: TerminalCommand,
    registration: Option<SessionRegistration>,
    socket_mode: Option<SocketMode>,
}

impl DaemonRequest {
    pub fn from_environment() -> Self {
        let arguments = DaemonArguments::from_environment();
        arguments.into_request()
    }

    pub fn run(self) -> Result<()> {
        let runtime = Builder::new_multi_thread().enable_all().build()?;
        runtime.block_on(
            TerminalCellDaemon::new(
                self.control_socket,
                self.data_socket,
                TerminalLaunch::new(self.command, TerminalSize::new(24, 80)),
                self.registration,
                self.socket_mode,
            )
            .run(),
        )
    }
}

struct TerminalCellDaemon {
    control_socket: PathBuf,
    data_socket: PathBuf,
    launch: TerminalLaunch,
    registration: Option<SessionRegistration>,
    socket_mode: Option<SocketMode>,
}

impl TerminalCellDaemon {
    fn new(
        control_socket: PathBuf,
        data_socket: PathBuf,
        launch: TerminalLaunch,
        registration: Option<SessionRegistration>,
        socket_mode: Option<SocketMode>,
    ) -> Self {
        Self {
            control_socket,
            data_socket,
            launch,
            registration,
            socket_mode,
        }
    }

    async fn run(self) -> Result<()> {
        let session = TerminalCell::spawn_session(self.launch);
        let terminal = session.actor();
        let input_port = session.input_port();
        let output_port = session.output_port();
        let signal_control = TerminalSignalControl::spawn(TerminalSignalControl::new(
            terminal.clone(),
            input_port.clone(),
        ));
        terminal
            .wait_for_startup_result()
            .await
            .map_err(|error| Error::TerminalCell {
                detail: format!("terminal cell startup failed: {error}"),
            })?;

        TerminalSocketFile::new(self.control_socket.as_path()).prepare()?;
        let control_listener = UnixListener::bind(&self.control_socket)?;
        TerminalSocketFile::new(self.data_socket.as_path()).prepare()?;
        let data_listener = UnixListener::bind(&self.data_socket)?;
        if let Some(socket_mode) = self.socket_mode {
            socket_mode.apply_to(&self.control_socket)?;
            socket_mode.apply_to(&self.data_socket)?;
        }
        if let Some(registration) = &self.registration {
            registration.record()?;
        }
        let runtime = Handle::current();

        println!(
            "persona-terminal-daemon control-socket={} data-socket={}",
            self.control_socket.display(),
            self.data_socket.display()
        );
        io::stdout().flush()?;

        let control_loop = TerminalControlPlaneLoop::new(
            control_listener,
            terminal.clone(),
            input_port.clone(),
            signal_control,
            runtime.clone(),
        );
        let data_loop = TerminalDataPlaneLoop::new(
            data_listener,
            terminal,
            input_port,
            output_port,
            runtime,
        );

        let control_task = tokio::task::spawn_blocking(move || control_loop.run());
        let data_task = tokio::task::spawn_blocking(move || data_loop.run());

        let (control_result, data_result) = tokio::join!(control_task, data_task);
        control_result.map_err(|error| Error::TerminalCell {
            detail: format!("terminal control loop task failed: {error}"),
        })??;
        data_result.map_err(|error| Error::TerminalCell {
            detail: format!("terminal data loop task failed: {error}"),
        })??;
        Ok(())
    }
}

struct DaemonArguments {
    control_socket: PathBuf,
    data_socket: PathBuf,
    command: TerminalCommand,
    store: Option<StoreLocation>,
    terminal: Option<signal_persona_terminal::TerminalName>,
}

impl DaemonArguments {
    fn from_environment() -> Self {
        let mut arguments = env::args_os().skip(1);
        let mut control_socket = None;
        let mut data_socket = None;
        let mut store = None;
        let mut terminal = None;
        let mut command = Vec::new();

        while let Some(argument) = arguments.next() {
            match argument.to_string_lossy().as_ref() {
                "--" => {
                    command.extend(arguments.map(|value| value.to_string_lossy().into_owned()));
                    break;
                }
                "--control-socket" => control_socket = arguments.next().map(PathBuf::from),
                "--data-socket" => data_socket = arguments.next().map(PathBuf::from),
                "--store" => store = arguments.next().map(StoreLocation::new),
                "--name" | "--terminal" => {
                    terminal = arguments.next().map(|value| {
                        signal_persona_terminal::TerminalName::new(value.to_string_lossy())
                    })
                }
                value => {
                    command.push(value.to_string());
                    command.extend(arguments.map(|value| value.to_string_lossy().into_owned()));
                    break;
                }
            }
        }

        Self {
            control_socket: control_socket.unwrap_or_else(default_control_socket),
            data_socket: data_socket.unwrap_or_else(default_data_socket),
            command: Self::command_from(command),
            store,
            terminal,
        }
    }

    fn into_request(self) -> DaemonRequest {
        let registration = self.terminal.map(|terminal| {
            SessionRegistration::ready(
                self.store.unwrap_or_else(StoreLocation::from_environment),
                terminal,
                self.control_socket.clone(),
            )
        });
        DaemonRequest {
            control_socket: self.control_socket,
            data_socket: self.data_socket,
            command: self.command,
            registration,
            socket_mode: SocketMode::from_environment(),
        }
    }

    fn command_from(command: Vec<String>) -> TerminalCommand {
        let mut command = command.into_iter();
        match command.next() {
            Some(program) => TerminalCommand::new(program, command.collect::<Vec<_>>()),
            None => default_command(),
        }
    }
}

struct TerminalSocketFile<'path> {
    path: &'path Path,
}

impl<'path> TerminalSocketFile<'path> {
    fn new(path: &'path Path) -> Self {
        Self { path }
    }

    fn prepare(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }

        match fs::symlink_metadata(self.path) {
            Ok(metadata) if metadata.file_type().is_socket() => fs::remove_file(self.path),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "refusing to replace non-socket path {}",
                    self.path.display()
                ),
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

struct TerminalControlPlaneLoop {
    listener: UnixListener,
    terminal: ActorRef<TerminalCell>,
    input_port: TerminalInputPort,
    signal_control: ActorRef<TerminalSignalControl>,
    runtime: Handle,
}

impl TerminalControlPlaneLoop {
    fn new(
        listener: UnixListener,
        terminal: ActorRef<TerminalCell>,
        input_port: TerminalInputPort,
        signal_control: ActorRef<TerminalSignalControl>,
        runtime: Handle,
    ) -> Self {
        Self {
            listener,
            terminal,
            input_port,
            signal_control,
            runtime,
        }
    }

    fn run(self) -> io::Result<()> {
        let _ = self
            .terminal
            .tell(TerminalWorkerLifecycle::Started(
                TerminalWorkerKind::SocketAcceptLoop,
            ))
            .try_send();

        for incoming in self.listener.incoming() {
            let stream = match incoming {
                Ok(stream) => stream,
                Err(error) => {
                    let _ = self
                        .terminal
                        .tell(TerminalWorkerLifecycle::Stopped {
                            worker: TerminalWorkerKind::SocketAcceptLoop,
                            reason: TerminalWorkerStop::SocketAcceptFailed(error.to_string()),
                        })
                        .try_send();
                    return Err(error);
                }
            };
            let terminal = self.terminal.clone();
            let input_port = self.input_port.clone();
            let signal_control = self.signal_control.clone();
            let runtime = self.runtime.clone();
            thread::Builder::new()
                .name("persona-terminal-control-connection".to_string())
                .spawn(move || {
                    if let Err(error) = TerminalControlConnection::new(
                        stream,
                        terminal,
                        input_port,
                        signal_control,
                        runtime,
                    )
                    .run()
                    {
                        eprintln!("persona terminal control connection failed: {error}");
                    }
                })?;
        }
        Ok(())
    }
}

struct TerminalDataPlaneLoop {
    listener: UnixListener,
    terminal: ActorRef<TerminalCell>,
    input_port: TerminalInputPort,
    output_port: TerminalOutputPort,
    runtime: Handle,
}

impl TerminalDataPlaneLoop {
    fn new(
        listener: UnixListener,
        terminal: ActorRef<TerminalCell>,
        input_port: TerminalInputPort,
        output_port: TerminalOutputPort,
        runtime: Handle,
    ) -> Self {
        Self {
            listener,
            terminal,
            input_port,
            output_port,
            runtime,
        }
    }

    fn run(self) -> io::Result<()> {
        for incoming in self.listener.incoming() {
            let stream = incoming?;
            let terminal = self.terminal.clone();
            let input_port = self.input_port.clone();
            let output_port = self.output_port.clone();
            let runtime = self.runtime.clone();
            thread::Builder::new()
                .name("persona-terminal-data-connection".to_string())
                .spawn(move || {
                    if let Err(error) = TerminalDataConnection::new(
                        stream,
                        terminal,
                        input_port,
                        output_port,
                        runtime,
                    )
                    .run()
                    {
                        eprintln!("persona terminal data connection failed: {error}");
                    }
                })?;
        }
        Ok(())
    }
}

/// A control-plane connection. Carries every kind of request *except*
/// `Attach`: an attach request on the control plane is an
/// architectural-truth violation and is explicitly rejected so the wire
/// boundary stays clean.
struct TerminalControlConnection {
    stream: UnixStream,
    terminal: ActorRef<TerminalCell>,
    input_port: TerminalInputPort,
    signal_control: ActorRef<TerminalSignalControl>,
    runtime: Handle,
}

impl TerminalControlConnection {
    fn new(
        stream: UnixStream,
        terminal: ActorRef<TerminalCell>,
        input_port: TerminalInputPort,
        signal_control: ActorRef<TerminalSignalControl>,
        runtime: Handle,
    ) -> Self {
        Self {
            stream,
            terminal,
            input_port,
            signal_control,
            runtime,
        }
    }

    fn run(&mut self) -> io::Result<()> {
        let request = SocketRequestReader::new(&mut self.stream).read_request()?;
        match request {
            SocketRequest::Capture => self.write_snapshot(),
            SocketRequest::SubscribeFromBeginning => self.stream_subscription(),
            SocketRequest::Attach => self.reject_attach_on_control_plane(),
            SocketRequest::Input(input) => self.write_input(input),
            SocketRequest::CloseHumanInput => self.close_human_input(),
            SocketRequest::OpenHumanInput(lease) => self.open_human_input(lease),
            SocketRequest::Resize(size) => self.write_resize(size),
            SocketRequest::Wait(wait) => self.wait_for_text(wait),
            SocketRequest::WaitExit => self.wait_for_exit(),
            SocketRequest::WorkerObservation => self.write_worker_observation(),
            SocketRequest::Signal(request) => self.handle_signal_request(request),
        }
    }

    fn reject_attach_on_control_plane(&mut self) -> io::Result<()> {
        SocketReplyWriter::new(&mut self.stream).write_attach_rejected(
            "control socket does not accept viewer attach; use the data socket",
        )?;
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "attach request arrived on persona-terminal control socket",
        ))
    }

    fn write_snapshot(&mut self) -> io::Result<()> {
        let snapshot = self.snapshot()?;
        SocketReplyWriter::new(&mut self.stream).write_snapshot(snapshot.bytes())
    }

    fn stream_subscription(&mut self) -> io::Result<()> {
        let mut subscription = self.subscription()?;
        self.stream.write_all(&subscription.replay_bytes())?;
        self.stream.flush()?;
        while let Some(delta) = subscription.blocking_next_live_delta() {
            if self.stream.write_all(delta.bytes()).is_err() {
                break;
            }
            if self.stream.flush().is_err() {
                break;
            }
        }
        Ok(())
    }

    fn write_input(&mut self, input: TerminalInput) -> io::Result<()> {
        let _acceptance = self
            .input_port
            .accept(input)
            .map_err(Self::terminal_error)?;
        SocketReplyWriter::new(&mut self.stream).write_acceptance()
    }

    fn close_human_input(&mut self) -> io::Result<()> {
        let lease = self
            .input_port
            .close_human_input()
            .map_err(Self::terminal_error)?;
        SocketReplyWriter::new(&mut self.stream).write_gate_lease(lease)
    }

    fn open_human_input(&mut self, lease: TerminalInputGateLease) -> io::Result<()> {
        let release = self
            .input_port
            .open_human_input(lease)
            .map_err(Self::terminal_error)?;
        SocketReplyWriter::new(&mut self.stream).write_gate_release(release)
    }

    fn write_resize(&mut self, size: TerminalSize) -> io::Result<()> {
        self.runtime
            .block_on(async { self.terminal.ask(size).await })
            .map_err(Self::actor_error)?;
        SocketReplyWriter::new(&mut self.stream).write_acceptance()
    }

    fn wait_for_text(&mut self, wait: WaitForTranscriptText) -> io::Result<()> {
        let matched = self
            .runtime
            .block_on(async { self.terminal.ask(wait).await })
            .map_err(Self::actor_error)?;
        if matched {
            SocketReplyWriter::new(&mut self.stream).write_wait_satisfied()
        } else {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "terminal transcript waiter ended without a match",
            ))
        }
    }

    fn snapshot(&self) -> io::Result<terminal_cell::TranscriptSnapshot> {
        self.runtime
            .block_on(async { self.terminal.ask(TranscriptSnapshotRequest).await })
            .map_err(Self::actor_error)
    }

    fn wait_for_exit(&mut self) -> io::Result<()> {
        let exit = self
            .runtime
            .block_on(async { self.terminal.ask(WaitForTerminalExit).await })
            .map_err(Self::actor_error)?;
        SocketReplyWriter::new(&mut self.stream).write_exit_status(exit.status())
    }

    fn write_worker_observation(&mut self) -> io::Result<()> {
        let observation = self
            .runtime
            .block_on(async { self.terminal.ask(TerminalWorkerObservationRequest).await })
            .map_err(Self::actor_error)?;
        SocketReplyWriter::new(&mut self.stream).write_snapshot(observation.to_text().as_bytes())
    }

    fn handle_signal_request(&mut self, request: SignalSocketRequest) -> io::Result<()> {
        if let terminal_signal::TerminalRequest::SubscribeTerminalWorkerLifecycle(subscription) =
            request.payload()
        {
            return self.stream_signal_worker_lifecycle(subscription.clone());
        }

        let event = self
            .runtime
            .block_on(async {
                self.signal_control
                    .ask(TerminalSignalControlRequest::new(request.into_payload()))
                    .await
            })
            .map_err(Self::actor_error)?;
        SocketReplyWriter::new(&mut self.stream).write_signal_event(event)
    }

    fn stream_signal_worker_lifecycle(
        &mut self,
        subscription: terminal_signal::SubscribeTerminalWorkerLifecycle,
    ) -> io::Result<()> {
        let mut lifecycle = self
            .runtime
            .block_on(async {
                self.terminal
                    .ask(TerminalWorkerLifecycleSubscriptionRequest)
                    .await
            })
            .map_err(Self::actor_error)?;
        SocketReplyWriter::new(&mut self.stream).write_signal_event(
            terminal_signal::TerminalWorkerLifecycleSnapshot {
                terminal: subscription.terminal.clone(),
                observations: lifecycle
                    .replay()
                    .iter()
                    .cloned()
                    .map(TerminalSignalControl::worker_lifecycle)
                    .collect(),
            }
            .into(),
        )?;

        while let Some(event) = lifecycle.blocking_next_live_event() {
            // Per /176 §1 + /177 §3, TerminalWorkerLifecycleEvent now
            // belongs to the streaming TerminalEvent enum, not the
            // direct-reply TerminalReply enum. terminal-cell exposes
            // `write_signal_subscription_event` for this path; the
            // persona-terminal supervisor is a passthrough so it
            // wraps the same way.
            SocketReplyWriter::new(&mut self.stream).write_signal_subscription_event(
                terminal_signal::TerminalWorkerLifecycleEvent {
                    terminal: subscription.terminal.clone(),
                    observation: TerminalSignalControl::worker_lifecycle(event),
                }
                .into(),
            )?;
        }
        Ok(())
    }

    fn subscription(&self) -> io::Result<terminal_cell::TranscriptSubscription> {
        self.runtime
            .block_on(async {
                self.terminal
                    .ask(TranscriptSubscriptionRequest::from_beginning())
                    .await
            })
            .map_err(Self::actor_error)
    }

    fn actor_error(error: impl std::fmt::Display) -> io::Error {
        io::Error::new(io::ErrorKind::BrokenPipe, error.to_string())
    }

    fn terminal_error(error: TerminalCellError) -> io::Error {
        io::Error::new(io::ErrorKind::BrokenPipe, error.to_string())
    }
}

/// A data-plane connection. Carries only an attach handshake followed
/// by raw bidirectional bytes between the viewer and the child PTY. The
/// connection rejects every kind of request other than `Attach` so the
/// wire boundary stays clean.
struct TerminalDataConnection {
    stream: UnixStream,
    terminal: ActorRef<TerminalCell>,
    input_port: TerminalInputPort,
    output_port: TerminalOutputPort,
    runtime: Handle,
}

impl TerminalDataConnection {
    fn new(
        stream: UnixStream,
        terminal: ActorRef<TerminalCell>,
        input_port: TerminalInputPort,
        output_port: TerminalOutputPort,
        runtime: Handle,
    ) -> Self {
        Self {
            stream,
            terminal,
            input_port,
            output_port,
            runtime,
        }
    }

    fn run(&mut self) -> io::Result<()> {
        let request = SocketRequestReader::new(&mut self.stream).read_request()?;
        match request {
            SocketRequest::Attach => self.attach_viewer(),
            _ => self.reject_non_attach_on_data_plane(),
        }
    }

    fn reject_non_attach_on_data_plane(&mut self) -> io::Result<()> {
        SocketReplyWriter::new(&mut self.stream).write_attach_rejected(
            "data socket only accepts viewer attach; use the control socket",
        )?;
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "non-attach request arrived on persona-terminal data socket",
        ))
    }

    fn attach_viewer(&mut self) -> io::Result<()> {
        let lease = match self.output_port.reserve_viewer() {
            Ok(lease) => lease,
            Err(TerminalCellError::ViewerAlreadyAttached) => {
                SocketReplyWriter::new(&mut self.stream)
                    .write_attach_rejected("terminal cell already has an attached viewer")?;
                return Ok(());
            }
            Err(error) => return Err(Self::terminal_error(error)),
        };

        let result = self.complete_viewer_attach(lease);
        if result.is_err() {
            let _ = self.output_port.detach(lease);
        }
        result
    }

    fn complete_viewer_attach(&mut self, lease: TerminalViewerLease) -> io::Result<()> {
        SocketReplyWriter::new(&mut self.stream).write_attach_accepted()?;

        let snapshot = self.snapshot()?;
        if !snapshot.bytes().is_empty() {
            self.stream.write_all(snapshot.bytes())?;
            self.stream.flush()?;
        }

        self.output_port
            .activate_viewer(lease, self.stream.try_clone()?)
            .map_err(Self::terminal_error)?;

        self.record_worker_started(TerminalWorkerKind::AttachConnectionPump);
        let result = self.pump_viewer_input();
        let reason = match &result {
            Ok(()) => TerminalWorkerStop::AttachConnectionClosed,
            Err(error) => TerminalWorkerStop::AttachConnectionFailed(error.to_string()),
        };
        self.record_worker_stopped(TerminalWorkerKind::AttachConnectionPump, reason);
        let _ = self.output_port.detach(lease);
        result
    }

    fn pump_viewer_input(&mut self) -> io::Result<()> {
        let mut buffer = [0_u8; 8192];
        loop {
            let count = self.stream.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            self.input_port
                .accept(TerminalInput::new(
                    buffer[..count].to_vec(),
                    InputSource::Viewer,
                ))
                .map_err(Self::terminal_error)?;
        }
        Ok(())
    }

    fn snapshot(&self) -> io::Result<terminal_cell::TranscriptSnapshot> {
        self.runtime
            .block_on(async { self.terminal.ask(TranscriptSnapshotRequest).await })
            .map_err(Self::actor_error)
    }

    fn record_worker_started(&self, worker: TerminalWorkerKind) {
        let _ = self
            .terminal
            .tell(TerminalWorkerLifecycle::Started(worker))
            .try_send();
    }

    fn record_worker_stopped(&self, worker: TerminalWorkerKind, reason: TerminalWorkerStop) {
        let _ = self
            .terminal
            .tell(TerminalWorkerLifecycle::Stopped { worker, reason })
            .try_send();
    }

    fn actor_error(error: impl std::fmt::Display) -> io::Error {
        io::Error::new(io::ErrorKind::BrokenPipe, error.to_string())
    }

    fn terminal_error(error: TerminalCellError) -> io::Error {
        io::Error::new(io::ErrorKind::BrokenPipe, error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewerRequest {
    control_socket: PathBuf,
    data_socket: PathBuf,
    mode: ViewMode,
    ready_file: Option<PathBuf>,
}

impl ViewerRequest {
    pub fn from_environment() -> Self {
        let mut arguments = env::args().skip(1);
        let mut control_socket = None;
        let mut data_socket = None;
        let mut mode = ViewMode::Interactive;
        let mut ready_file = None;

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--control-socket" => control_socket = arguments.next().map(PathBuf::from),
                "--data-socket" => data_socket = arguments.next().map(PathBuf::from),
                "--once" => mode = ViewMode::Snapshot,
                "--ready-file" => ready_file = arguments.next().map(PathBuf::from),
                _ => {}
            }
        }

        Self {
            control_socket: control_socket.unwrap_or_else(default_control_socket),
            data_socket: data_socket.unwrap_or_else(default_data_socket),
            mode,
            ready_file,
        }
    }

    pub fn run(self) -> Result<()> {
        TerminalViewer::new(self.control_socket, self.data_socket, self.mode, self.ready_file)
            .run()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Interactive,
    Snapshot,
}

struct TerminalViewer {
    client: TerminalCellSocketClient,
    mode: ViewMode,
    readiness: TerminalViewerReadiness,
}

impl TerminalViewer {
    fn new(
        control_socket: PathBuf,
        data_socket: PathBuf,
        mode: ViewMode,
        ready_file: Option<PathBuf>,
    ) -> Self {
        Self {
            client: TerminalCellSocketClient::new(control_socket, data_socket),
            mode,
            readiness: TerminalViewerReadiness::new(ready_file),
        }
    }

    fn run(&self) -> Result<()> {
        match self.mode {
            ViewMode::Interactive => self.attach(),
            ViewMode::Snapshot => self.print_snapshot(),
        }
    }

    fn print_snapshot(&self) -> Result<()> {
        let bytes = self.client.capture()?;
        io::stdout().write_all(&bytes)?;
        Ok(())
    }

    fn attach(&self) -> Result<()> {
        let mut resize_watcher = TerminalResizeWatcher::new(self.client.clone());
        resize_watcher.resize_now()?;
        let _resize_thread = resize_watcher.spawn()?;
        let mut attach_stream = self.client.open_attach_stream()?;
        let mut output_stream = attach_stream.try_clone()?;
        self.readiness.confirm_control_plane(&self.client)?;
        self.readiness.announce()?;
        let output = thread::Builder::new()
            .name("persona-terminal-view-output".to_string())
            .spawn(move || -> io::Result<()> {
                let mut stdout = io::stdout();
                let mut buffer = [0_u8; 8192];
                loop {
                    let count = output_stream.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    stdout.write_all(&buffer[..count])?;
                    stdout.flush()?;
                }
                Ok(())
            })?;

        let _raw_mode = TerminalRawMode::enter()?;
        let mut stdin = io::stdin();
        let mut buffer = [0_u8; 4096];
        loop {
            let count = stdin.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            attach_stream.write_all(&buffer[..count])?;
        }

        output.join().map_err(|_| Error::TerminalCell {
            detail: "terminal view output thread panicked".to_string(),
        })??;
        Ok(())
    }
}

struct TerminalResizeWatcher {
    client: TerminalCellSocketClient,
    last_size: Option<TerminalSize>,
}

impl TerminalResizeWatcher {
    fn new(client: TerminalCellSocketClient) -> Self {
        Self {
            client,
            last_size: None,
        }
    }

    fn spawn(mut self) -> io::Result<thread::JoinHandle<()>> {
        let mut signals = Signals::new([SIGWINCH])?;
        thread::Builder::new()
            .name("persona-terminal-view-resize".to_string())
            .spawn(move || {
                for _signal in signals.forever() {
                    if self.resize_now().is_err() {
                        break;
                    }
                }
            })
    }

    fn resize_now(&mut self) -> io::Result<()> {
        let size = self.current_attached_terminal_size()?;
        if self.last_size == Some(size) {
            return Ok(());
        }
        self.client.resize(size)?;
        self.last_size = Some(size);
        Ok(())
    }

    fn current_attached_terminal_size(&self) -> io::Result<TerminalSize> {
        let (columns, rows) = terminal_size()?;
        Ok(TerminalSize::new(rows, columns))
    }
}

struct TerminalViewerReadiness {
    ready_file: Option<PathBuf>,
}

impl TerminalViewerReadiness {
    fn new(ready_file: Option<PathBuf>) -> Self {
        Self { ready_file }
    }

    fn announce(&self) -> io::Result<()> {
        if let Some(path) = &self.ready_file {
            fs::write(path, b"persona-terminal-view attached\n")?;
        }
        Ok(())
    }

    fn confirm_control_plane(&self, client: &TerminalCellSocketClient) -> io::Result<()> {
        client.capture().map(|_snapshot| ())
    }
}

struct TerminalRawMode {
    enabled: bool,
}

impl TerminalRawMode {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        Ok(Self { enabled: true })
    }
}

impl Drop for TerminalRawMode {
    fn drop(&mut self) {
        if self.enabled {
            let _ = disable_raw_mode();
            self.enabled = false;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendRequest {
    control_socket: PathBuf,
    text: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeRequest {
    control_socket: PathBuf,
    text: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureRequest {
    control_socket: PathBuf,
}

/// Control-plane client around a single terminal cell. Every verb this
/// type exposes is a control verb. Attach lives in `TerminalCellSocketClient`
/// and the `terminal-cell-view` / `persona-terminal-view` binaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSocket {
    client: TerminalCellSocketClient,
}

impl TerminalSocket {
    pub fn from_control_socket(path: impl Into<PathBuf>) -> Self {
        Self {
            client: TerminalCellSocketClient::for_control_only(path),
        }
    }

    pub fn send_prompt(&self, text: &str) -> Result<()> {
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(b'\r');
        self.send_bytes(&bytes)
    }

    pub fn send_text(&self, text: &str) -> Result<()> {
        self.send_bytes(text.as_bytes())
    }

    pub fn send_bytes(&self, bytes: &[u8]) -> Result<()> {
        self.client.send_programmatic_input(bytes)?;
        Ok(())
    }

    pub fn send_viewer_bytes(&self, bytes: &[u8]) -> Result<()> {
        self.client.send_viewer_input(bytes)?;
        Ok(())
    }

    pub fn resize(&self, rows: u16, columns: u16) -> Result<()> {
        self.client.resize(TerminalSize::new(rows, columns))?;
        Ok(())
    }

    pub fn capture(&self) -> Result<TerminalSnapshot> {
        Ok(TerminalSnapshot::from_bytes(self.client.capture()?))
    }

    pub fn signal(
        &self,
        request: terminal_signal::TerminalRequest,
    ) -> Result<terminal_signal::TerminalReply> {
        Ok(self
            .client
            .send_signal_request(request.signal_verb(), request)?)
    }
}

impl SendRequest {
    pub fn from_environment() -> Self {
        let (control_socket, text) = control_socket_and_text_from_environment();
        Self {
            control_socket,
            text,
        }
    }

    pub fn run(self) -> Result<()> {
        TerminalSocket::from_control_socket(self.control_socket)
            .send_prompt(&String::from_utf8_lossy(&self.text))
    }
}

impl TypeRequest {
    pub fn from_environment() -> Self {
        let (control_socket, text) = control_socket_and_text_from_environment();
        Self {
            control_socket,
            text,
        }
    }

    pub fn run(self) -> Result<()> {
        TerminalSocket::from_control_socket(self.control_socket).send_viewer_bytes(&self.text)
    }
}

impl CaptureRequest {
    pub fn from_environment() -> Self {
        let control_socket = env::args_os()
            .nth(1)
            .map(PathBuf::from)
            .unwrap_or_else(default_control_socket);
        Self { control_socket }
    }

    pub fn run(self, mut output: impl Write) -> Result<()> {
        let snapshot = TerminalSocket::from_control_socket(self.control_socket).capture()?;
        match CapturePresentation::from_environment() {
            CapturePresentation::Raw => output.write_all(snapshot.as_bytes())?,
            CapturePresentation::Screen { rows, columns } => {
                output.write_all(snapshot.visible_text(rows, columns).as_bytes())?;
            }
        }
        output.flush()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapturePresentation {
    Raw,
    Screen { rows: u16, columns: u16 },
}

impl CapturePresentation {
    fn from_environment() -> Self {
        if !matches!(
            env::var("PERSONA_TERMINAL_CAPTURE_MODE").as_deref(),
            Ok("screen")
        ) {
            return Self::Raw;
        }
        let rows = env::var("PERSONA_TERMINAL_CAPTURE_ROWS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(32);
        let columns = env::var("PERSONA_TERMINAL_CAPTURE_COLUMNS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(120);
        Self::Screen { rows, columns }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSnapshot {
    bytes: Vec<u8>,
}

impl TerminalSnapshot {
    fn from_bytes(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    pub fn to_string_lossy(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }

    pub fn visible_text(&self, rows: u16, columns: u16) -> String {
        self.screen(rows, columns).visible_text
    }

    pub fn screen(&self, rows: u16, columns: u16) -> TerminalScreenSnapshot {
        let mut parser = vt100::Parser::new(rows, columns, 0);
        parser.process(&self.bytes);
        let screen = parser.screen();
        let (cursor_row, cursor_column) = screen.cursor_position();
        let lines = screen.rows(0, columns).collect::<Vec<_>>();
        let cursor_line = lines.get(cursor_row as usize).cloned().unwrap_or_default();
        TerminalScreenSnapshot {
            visible_text: screen.contents(),
            cursor_row,
            cursor_column,
            cursor_line,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalScreenSnapshot {
    visible_text: String,
    cursor_row: u16,
    cursor_column: u16,
    cursor_line: String,
}

impl TerminalScreenSnapshot {
    pub fn visible_text(&self) -> &str {
        self.visible_text.as_str()
    }

    pub fn cursor_row(&self) -> u16 {
        self.cursor_row
    }

    pub fn cursor_column(&self) -> u16 {
        self.cursor_column
    }

    pub fn cursor_line(&self) -> &str {
        self.cursor_line.as_str()
    }
}

fn default_control_socket() -> PathBuf {
    PathBuf::from(DEFAULT_CONTROL_SOCKET)
}

fn default_data_socket() -> PathBuf {
    PathBuf::from(DEFAULT_DATA_SOCKET)
}

fn default_command() -> TerminalCommand {
    TerminalCommand::new(env::var("SHELL").unwrap_or_else(|_| "bash".to_string()), [])
}

fn control_socket_and_text_from_environment() -> (PathBuf, Vec<u8>) {
    let mut arguments = env::args_os().skip(1);
    let control_socket = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(default_control_socket);
    let text = arguments
        .next()
        .map(|value| value.to_string_lossy().into_owned().into_bytes())
        .unwrap_or_default();
    (control_socket, text)
}
