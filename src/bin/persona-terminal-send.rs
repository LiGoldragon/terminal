use persona_terminal::pty::SendRequest;

fn main() -> persona_terminal::Result<()> {
    SendRequest::from_environment().run()
}
