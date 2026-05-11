use persona_terminal::pty::DaemonRequest;

fn main() -> persona_terminal::Result<()> {
    DaemonRequest::from_environment().run()
}
