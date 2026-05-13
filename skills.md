# persona-terminal skill

Work here when the change concerns durable PTYs, viewer adapters, raw input
delivery, resize delivery, or scrollback replay.

Rules for work here:

- `persona-terminal` is the owner noun. Do not create or revive
  terminal-brand repository names for viewer implementations.
- `terminal-cell` is the low-level PTY/transcript primitive. Persona-facing
  naming, registry policy, component Sema metadata, and Signal adaptation live
  here.
- Session registry state lives in this repo's component Sema. Do not add
  registry JSON, text manifests, or viewer-owned state files for terminal names.
- Inspectable terminal table values use `signal-persona-terminal`'s
  introspection record shapes. This repo still owns the Sema database, table
  declarations, reducers, and consistency policy.
- Viewer and compositor behavior is adapter-local around the terminal owner.
  Do not revive terminal-brand mux helpers as runtime paths.
- Keep harness processes durable. Closing a viewer must not kill the child
  harness process.
- Keep Persona message semantics out of this repo.
- Keep `persona-harness` as a sibling client over
  `signal-persona-terminal`; do not fold terminal ownership into the harness
  abstraction.
- Keep attached keyboard input as a raw byte path. Control behavior uses typed
  socket or Signal requests; it does not use a prefix-key grammar in the hot
  input path.
- Name repeatable stateful workflows under `scripts/` and expose them from
  `flake.nix`.
- Keep session inspection CLIs read-only. Effect-bearing commands use the
  daemon/socket path until the supervisor control socket owns the full command
  surface.
