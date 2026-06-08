# terminal skill

Work here when the change concerns the Persona terminal component:
typed Signal communication over the component communication socket, the
named-session registry in component Sema, the consolidated daemon,
prompt-pattern lifecycle, and viewer-adapter launch policy.

## Communication/data discipline

- This repo owns the component communication plane. Typed Signal flows over
  `terminal`'s communication socket; supervision uses a separate
  supervision socket.
- Ordinary terminal Signal uses `signal-terminal`. Meta-only session
  lifecycle mutation uses the terminal meta signal contract; do not put
  `CreateSession` / `RetireSession` back into the ordinary contract.
- Raw viewer bytes flow viewer ↔ session data path directly. They do not
  cross the component communication socket.
- The Sema session registry records two typed fields per cell:
  `control_socket_path` for Signal control, `data_socket_path` for viewer
  attach while transitional standalone terminal-cell paths still exist.
  Production component clients do not discover or dial those cell paths.
- Local viewer adapters and CLIs may ride on the terminal-cell client, which
  exposes `new(control, data)` for full clients and
  `for_control_only(control)` for local control-only clients.

## Component daemon

- `terminal-daemon` is the production component daemon. It binds a
  communication socket and a supervision socket, owns component Sema, and
  owns terminal session actors built on the `terminal_cell` library.
- Daemon startup takes exactly one signal-encoded/rkyv
  `TerminalDaemonConfiguration` file. Inline NOTA and `.nota`
  configuration files are CLI/deploy-tool material and are rejected before
  the daemon opens its runtime.
- The terminal meta surface is part of the same component owner. It is not
  a separate daemon; it is the authority-limited request vocabulary used by
  the orchestrate/harness chain to create or retire terminal sessions.
- `terminal-supervisor` now uses the generated async task-backed daemon process
  shell for ordinary and meta listeners while still routing to the existing
  supervisor actor. The old one-PTY `terminal-daemon` behavior remains a
  transitional implementation step. Keep their witnesses useful while folding
  their behavior into the consolidated component daemon.
- Every engine-bound socket applies `PERSONA_SOCKET_MODE` (mode 0600 default)
  before accepting traffic.

## Registry and storage

- Session registry state lives in this repo's component Sema. Do not add
  registry JSON, text manifests, or viewer-owned state files for terminal
  names.
- Inspectable terminal table values use `signal-terminal`'s
  introspection record shapes. This repo owns the Sema database, table
  declarations, reducers, and consistency policy.
- Session registration also writes a `session_health` ready-state row.

## Scope

- `terminal` is the owner noun. Do not create or revive
  terminal-brand repository names for viewer implementations.
- Viewer and compositor behavior is adapter-local around the terminal owner.
  Do not revive terminal-brand mux helpers as runtime paths.
- Keep harness processes durable. Closing a viewer must not kill the child
  harness process.
- Keep Persona message semantics out of this repo.
- Keep `harness` as a sibling client over `signal-terminal`;
  do not fold terminal ownership into the harness abstraction.
- Keep attached keyboard input as a raw byte path. Control behavior uses
  typed socket or Signal requests; it does not use a prefix-key grammar in
  the hot input path.

## Subscriptions

- Subscription close is a typed retract/close request on the communication plane.
  The supervisor forwards the retract; the server emits a final
  acknowledgement event; the stream ends. Raw socket close is not semantic
  protocol.

## Testing and workflows

- Name repeatable stateful workflows under `scripts/` and expose them from
  `flake.nix`.
- Keep session inspection CLIs read-only. Effect-bearing commands use the
  daemon/socket path until the component communication socket owns the full
  command surface.
