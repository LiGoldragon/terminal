use persona_wezterm::pty::ViewerRequest;

fn main() -> persona_wezterm::Result<()> {
    ViewerRequest::from_environment().run()
}
