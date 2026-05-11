use persona_terminal::signal_cli::TerminalSignalRequest;

fn main() -> persona_terminal::Result<()> {
    TerminalSignalRequest::from_environment()?.run(std::io::stdout())
}
