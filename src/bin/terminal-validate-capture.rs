use terminal::capture_validator::CaptureValidatorCommandLine;

fn main() -> terminal::Result<()> {
    CaptureValidatorCommandLine::from_environment().run()
}
