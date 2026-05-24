use terminal::registry::SessionListRequest;

fn main() -> terminal::Result<()> {
    SessionListRequest::from_environment().run(std::io::stdout())
}
