use terminal::pty::SendRequest;

fn main() -> terminal::Result<()> {
    SendRequest::from_environment().run()
}
