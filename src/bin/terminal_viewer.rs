use terminal::pty::ViewerRequest;

fn main() -> terminal::Result<()> {
    ViewerRequest::from_environment().run()
}
