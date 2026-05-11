use persona_terminal::pty::TypeRequest;

fn main() -> persona_terminal::Result<()> {
    TypeRequest::from_environment().run()
}
