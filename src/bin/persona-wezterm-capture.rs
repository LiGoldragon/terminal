use persona_wezterm::pty::CaptureRequest;

fn main() -> persona_wezterm::Result<()> {
    CaptureRequest::from_environment().run(std::io::stdout())
}
