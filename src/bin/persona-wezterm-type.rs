use persona_wezterm::pty::TypeRequest;

fn main() -> persona_wezterm::Result<()> {
    TypeRequest::from_environment().run()
}
