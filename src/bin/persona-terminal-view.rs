use persona_terminal::pty::ViewerRequest;

fn main() -> persona_terminal::Result<()> {
    ViewerRequest::from_environment().run()
}
