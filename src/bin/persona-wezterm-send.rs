use persona_wezterm::pty::SendRequest;

fn main() -> persona_wezterm::Result<()> {
    SendRequest::from_environment().run()
}
