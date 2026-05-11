# persona-terminal skill

Work here when the change concerns durable PTYs, viewer adapters, raw input
delivery, resize delivery, or scrollback replay.

Rules for work here:

- `persona-terminal` is the owner noun. Do not create or revive
  terminal-brand repository names for viewer implementations.
- `terminal-cell` is the low-level PTY/transcript primitive. Persona-facing
  naming, registry policy, component Sema metadata, and Signal adaptation live
  here.
- WezTerm/Ghostty/Niri behavior is viewer-adapter behavior around the terminal
  owner. WezTerm code is currently shelved adapter code, not a required runtime.
- Keep harness processes durable. Closing a viewer must not kill the child
  harness process.
- Keep Persona message semantics out of this repo.
- Keep attached keyboard input as a raw byte path. Control behavior uses typed
  socket or Signal requests; it does not use a prefix-key grammar in the hot
  input path.
- Name repeatable stateful workflows under `scripts/` and expose them from
  `flake.nix`.
