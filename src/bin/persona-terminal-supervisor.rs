use nota_config::ConfigurationSource;
use persona_terminal::supervisor::{TerminalSupervisorCommandLine, TerminalSupervisorDaemon};
use signal_persona_terminal::TerminalDaemonConfiguration;

fn main() -> persona_terminal::Result<()> {
    // The supervised production launch passes a typed
    // `TerminalDaemonConfiguration` as argv[1]. The same binary also
    // serves the legacy `--socket --store` CLI surface; pick the
    // typed path when argv looks like a configuration source.
    if first_argument_is_typed_configuration_source() {
        let configuration: TerminalDaemonConfiguration =
            ConfigurationSource::from_argv()?.decode()?;
        return TerminalSupervisorDaemon::from_configuration(configuration).run();
    }
    TerminalSupervisorCommandLine::from_environment().run()
}

fn first_argument_is_typed_configuration_source() -> bool {
    let Some(argument) = std::env::args_os().nth(1) else {
        return false;
    };
    let lossy = argument.to_string_lossy();
    if lossy.starts_with('(') {
        return true;
    }
    let path = std::path::Path::new(&argument);
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("nota") | Some("rkyv")
    )
}
