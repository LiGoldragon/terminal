use persona_terminal::pty::CaptureRequest;

fn main() -> persona_terminal::Result<()> {
    CaptureRequest::from_environment().run(std::io::stdout())
}
