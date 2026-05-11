use persona_terminal::registry::SessionResolveRequest;

fn main() -> persona_terminal::Result<()> {
    SessionResolveRequest::from_environment().run(std::io::stdout())
}
