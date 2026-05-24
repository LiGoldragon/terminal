use terminal::signal_cli::TerminalSignalRequest;

fn main() -> terminal::Result<()> {
    TerminalSignalRequest::from_environment()?.run(std::io::stdout())
}
