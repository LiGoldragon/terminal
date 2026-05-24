use terminal::pty::TypeRequest;

fn main() -> terminal::Result<()> {
    TypeRequest::from_environment().run()
}
