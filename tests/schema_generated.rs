use terminal::schema::{nexus, sema, signal};
use terminal::{ComponentDaemon, TerminalProcessDaemon};

#[test]
fn generated_terminal_planes_expose_control_lifecycle_and_registry_nouns() {
    let session = signal::SessionRecord {
        session_name: "shell".to_owned(),
        session_identifier: 1,
        socket_path: "/tmp/terminal-cell.sock".to_owned(),
    };

    let injection = signal::WriteInjectionRequest {
        session_name: "shell".to_owned(),
        input_lease_identifier: 2,
        injection_sequence: 3,
        terminal_bytes: vec![b'h'.into(), b'i'.into(), b'\n'.into()],
    };
    let signal_input = signal::Input::write_injection(injection);
    let signal_work = nexus::NexusWork::signal_arrived(signal_input);
    assert!(matches!(signal_work, nexus::NexusWork::SignalArrived(_)));

    let lifecycle = nexus::SessionLifecycleCommand::create_session(session.clone());
    let meta_work = nexus::NexusWork::meta_arrived(lifecycle);
    assert!(matches!(meta_work, nexus::NexusWork::MetaArrived(_)));

    let sema_write = sema::WriteInput::record_session(session);
    let nexus_write = nexus::NexusAction::command_sema_write(sema_write);
    assert!(matches!(
        nexus_write,
        nexus::NexusAction::CommandSemaWrite(_)
    ));

    let cell_command = nexus::TerminalCellCommand::write_injection("shell".to_owned());
    let effect = nexus::NexusEffectCommand::run_terminal_cell(cell_command);
    let nexus_effect = nexus::NexusAction::command_effect(effect);
    assert!(matches!(nexus_effect, nexus::NexusAction::CommandEffect(_)));
}

#[test]
fn generated_terminal_daemon_exposes_working_and_meta_listener_surface() {
    fn accepts_component_daemon<Daemon: ComponentDaemon>() {}

    accepts_component_daemon::<TerminalProcessDaemon>();
    assert_eq!(terminal::ListenerTier::Working.to_string(), "working");
    assert_eq!(terminal::ListenerTier::Meta.to_string(), "meta");
    assert_eq!(
        <TerminalProcessDaemon as ComponentDaemon>::PROCESS_NAME,
        "terminal-supervisor"
    );
}
