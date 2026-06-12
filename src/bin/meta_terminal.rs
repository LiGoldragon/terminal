use terminal::meta::MetaTerminalCommandLine;

fn main() -> terminal::Result<()> {
    MetaTerminalCommandLine::from_env().run(std::io::stdout())
}
