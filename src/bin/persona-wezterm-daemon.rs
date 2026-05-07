use persona_wezterm::pty::DaemonRequest;

fn main() -> persona_wezterm::Result<()> {
    DaemonRequest::from_environment().run()
}
