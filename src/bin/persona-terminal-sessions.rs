use persona_terminal::registry::SessionListRequest;

fn main() -> persona_terminal::Result<()> {
    SessionListRequest::from_environment().run(std::io::stdout())
}
