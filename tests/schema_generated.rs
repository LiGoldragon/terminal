//! The emitted component-daemon surface terminal actually binds.
//!
//! The retired stack also emitted `signal`, `sema` and `nexus` modules into
//! `src/schema/`. Nothing in terminal referenced them — they were 5014 lines
//! of generated code whose only reader was a test built to exercise it — and
//! `signal-terminal` 2.0.1 now carries the contract they duplicated, so they
//! were deleted rather than regenerated.

use terminal::{ComponentDaemon, TerminalProcessDaemon};

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
