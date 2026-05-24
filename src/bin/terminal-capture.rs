use terminal::pty::CaptureRequest;

fn main() -> terminal::Result<()> {
    CaptureRequest::from_environment().run(std::io::stdout())
}
