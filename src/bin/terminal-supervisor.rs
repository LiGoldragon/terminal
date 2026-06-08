use terminal::{DaemonEntry, TerminalProcessDaemon};

fn main() -> std::process::ExitCode {
    TerminalProcessDaemon::run_to_exit_code()
}
