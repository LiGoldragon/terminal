use terminal::{Result, TerminalSupervisorDaemonCommand};

fn main() -> Result<()> {
    TerminalSupervisorDaemonCommand::from_environment().run()
}
