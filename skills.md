# persona-terminal skill

Work here when the change concerns the Persona control plane for terminals:
typed Signal control over `terminal-cell` `control.sock`, the named-session
registry in component Sema, the supervisor frontend, prompt-pattern lifecycle,
and viewer-adapter launch policy.

## Two-plane discipline

- This repo owns the control plane only. Typed Signal flows
  `persona-terminal` ↔ terminal-cell `control.sock`. Raw viewer bytes flow
  viewer ↔ terminal-cell `data.sock` directly; `persona-terminal` is not on
  that path.
- The Sema session registry records two typed fields per cell:
  `control_socket_path` for Signal control, `data_socket_path` for viewer
  attach. The supervisor reads `control_socket_path` and forwards Signal
  frames there; it does not open data sockets. Viewer adapters read
  `data_socket_path` and connect directly.
- `persona-terminal-daemon` binds both `--control-socket` and
  `--data-socket`. Local viewer adapters and CLIs ride on the terminal-cell
  client, which exposes `new(control, data)` for full clients and
  `for_control_only(control)` for control-only clients.

## Daemon vs supervisor

- `persona-terminal-daemon` is a PTY-owning daemon. It embeds the
  `terminal_cell` library, hosts `TerminalSignalControl`, binds both
  sockets, and on `--name` writes a `SessionRegistration` recording both
  socket paths.
- `persona-terminal-supervisor` is a registry frontend. It binds one
  `signal-persona-terminal` socket, answers `SupervisionRequest` traffic,
  resolves named terminals from Sema, and forwards Signal frames to the
  resolved `control_socket_path`. It does not own a PTY and does not open
  data sockets.
- Both binaries apply `PERSONA_SOCKET_MODE` (mode 0600 default) to every
  socket they bind.

## Registry and storage

- Session registry state lives in this repo's component Sema. Do not add
  registry JSON, text manifests, or viewer-owned state files for terminal
  names.
- Inspectable terminal table values use `signal-persona-terminal`'s
  introspection record shapes. This repo owns the Sema database, table
  declarations, reducers, and consistency policy.
- Session registration also writes a `session_health` ready-state row.

## Scope

- `persona-terminal` is the owner noun. Do not create or revive
  terminal-brand repository names for viewer implementations.
- Viewer and compositor behavior is adapter-local around the terminal owner.
  Do not revive terminal-brand mux helpers as runtime paths.
- Keep harness processes durable. Closing a viewer must not kill the child
  harness process.
- Keep Persona message semantics out of this repo.
- Keep `persona-harness` as a sibling client over `signal-persona-terminal`;
  do not fold terminal ownership into the harness abstraction.
- Keep attached keyboard input as a raw byte path. Control behavior uses
  typed socket or Signal requests; it does not use a prefix-key grammar in
  the hot input path.

## Subscriptions

- Subscription close is a typed retract/close request on the control plane.
  The supervisor forwards the retract; the server emits a final
  acknowledgement event; the stream ends. Raw socket close is not semantic
  protocol.

## Testing and workflows

- Name repeatable stateful workflows under `scripts/` and expose them from
  `flake.nix`.
- Keep session inspection CLIs read-only. Effect-bearing commands use the
  daemon/socket path until the supervisor control socket owns the full
  command surface.
