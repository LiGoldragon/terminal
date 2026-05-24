use terminal::registry::SessionResolveRequest;

fn main() -> terminal::Result<()> {
    SessionResolveRequest::from_environment().run(std::io::stdout())
}
