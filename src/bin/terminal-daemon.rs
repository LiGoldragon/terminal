use terminal::pty::DaemonRequest;

fn main() -> terminal::Result<()> {
    DaemonRequest::from_environment().run()
}
