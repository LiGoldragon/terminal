use terminal::client::TerminalCommandLine;

fn main() -> terminal::Result<()> {
    TerminalCommandLine::from_env().run(std::io::stdout())
}
