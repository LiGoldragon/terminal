use persona_terminal::supervisor::TerminalSupervisorCommandLine;

fn main() -> persona_terminal::Result<()> {
    TerminalSupervisorCommandLine::from_environment().run()
}
