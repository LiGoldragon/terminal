//! Witness that persona-terminal's actor nouns carry data
//! (no public ZST actor markers).
//!
//! The no-shared-locks witness lives in
//! `actor_runtime_truth.rs::terminal_signal_control_state_is_owned_by_a_kameo_actor`;
//! this file adds the companion no-zst-actor witness per
//! `~/primary/skills/actor-systems.md` §"Test actor density".
//!
//! A future refactor that collapses an actor noun to a marker
//! ZST breaks this witness.

use persona_terminal::signal_control::TerminalSignalControl;
use persona_terminal::supervision::SupervisionPhase;
use persona_terminal::supervisor::TerminalSupervisor;

#[test]
fn public_actor_nouns_carry_data() {
    assert!(std::mem::size_of::<TerminalSignalControl>() > 0);
    assert!(std::mem::size_of::<TerminalSupervisor>() > 0);
    assert!(std::mem::size_of::<SupervisionPhase>() > 0);
}
