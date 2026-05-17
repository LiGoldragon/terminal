use persona_terminal::capture_validator::CaptureValidatorCommandLine;

fn main() -> persona_terminal::Result<()> {
    CaptureValidatorCommandLine::from_environment().run()
}
