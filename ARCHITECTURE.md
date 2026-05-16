# persona-terminal — architecture

*Persona-facing terminal session owner built around terminal-cell. Control
plane only — raw viewer bytes flow viewer ↔ terminal-cell directly.*

`persona-terminal` owns the Persona-facing surface around named terminal
sessions: typed control over Signal, component Sema registry, session
metadata, viewer-adapter launch policy. The control plane lives here; the
raw byte path lives in `terminal-cell`.

`terminal-cell` is the low-level cell primitive: one child process group, one
PTY, raw input ports, transcript replay, worker lifecycle observation, and
one active viewer attachment. Each cell exposes two sockets — a `control.sock`
for Signal control plus the byte-tag CLI protocol, and a `data.sock` for raw
attached-viewer bytes. This repo is the Persona-facing owner around those
cells: names, registry policy, typed terminal requests/events, component Sema
metadata, and viewer-adapter launch policy.

Terminal-brand mux helpers are retired. Viewer and compositor behavior lives
behind this same `persona-terminal` owner and must not become a repository
boundary.

---

## 0 · TL;DR

This repo carries the Persona control plane for terminals. It does not
understand Persona message semantics, routing policy, provider quota policy,
slash-command meaning, or authorization, and it does not move raw viewer
bytes.

```mermaid
flowchart LR
    harness["persona-harness"]
    pt["persona-terminal"]
    cell["terminal-cell daemon"]
    viewer["visible viewer"]
    pty["child PTY"]

    harness -- "Signal: prompt, gate, inject, capture" --> pt
    pt -- "control.sock: Signal" --> cell
    cell -- "control.sock: Signal events back" --> pt
    pt -- "Signal events back" --> harness
    viewer == "data.sock: raw bidirectional bytes" ==> cell
    cell == "data.sock: live PTY output" ==> viewer
    cell --> pty
```

The control path (`persona-harness` → `persona-terminal` → terminal-cell
`control.sock`) is typed Signal end to end. The data path (visible viewer ↔
terminal-cell `data.sock`) is raw bytes; `persona-terminal` is not on it.

## 1 · Component Surface

`persona-terminal` exposes:

- durable PTY daemon binary;
- visible viewer binary;
- raw input sender binary;
- signal terminal request client;
- output scrollback replay;
- resize propagation;
- terminal-cell control/data socket adapter;
- terminal Signal control actor for prompt patterns, input-gate leases,
  prompt-state checks, and injection decisions;
- component Sema table for named terminal sessions;
- read-only session inspection CLIs;
- `signal-persona-terminal` request/event adapter.

## 1.5 · Supervision-relation reception, prompt-pattern lifecycle, gate forwarding, message-landing endpoint

**Control plane split.** `persona-terminal` forwards control-plane Signal
frames (`RegisterPromptPattern`, `AcquireInputGate`, `WriteInjection`,
`ReleaseInputGate`, subscription frames, `TerminalCapture`, and the rest of
`signal-persona-terminal`) to the registered terminal-cell `control.sock`.
Raw attached-viewer bytes flow direct viewer ↔ terminal-cell `data.sock` and
never traverse `persona-terminal`. The supervisor stays on the control plane
only: it resolves names, forwards typed frames, relays typed events, and
records delivery; it does not pump raw bytes.

**Supervision relation**. The engine-facing binary is
`persona-terminal-daemon`. It owns `signal-persona::SpawnEnvelope` handling
and the `signal-persona::SupervisionRequest` answer surface — a canonical
`SupervisionPhase` Kameo actor sits alongside `TerminalSignalControl`. The
daemon reads its spawn envelope at startup, binds `terminal.sock` at mode
0600 by applying the `PERSONA_SOCKET_MODE` value from the envelope, and
proceeds. The supervisor is the daemon's internal control-plane library: it
owns name resolution, frame forwarding, and component-Sema bookkeeping, and
the daemon wraps it with supervisor semantics. Unbuilt domain operations
reply `TerminalEvent::TerminalRequestUnimplemented`.

**Prompt-pattern lifecycle**. `persona-harness` registers a per-adapter
`PromptPattern` with the supervisor at session-create time via
`signal-persona-terminal::RegisterPromptPattern`. The supervisor
forwards the registration to the relevant terminal-cell `control.sock`; the cell
returns a typed `PromptPatternId` which the supervisor stores keyed by
harness identity. Later `AcquireInputGate { pattern_id }` requests reference
that id.

**Gate-and-acquire forwarding**. When the supervisor receives
`AcquireInputGate`, it does **not** answer locally —
it forwards the request to the named terminal-cell `control.sock`, awaits the cell's
typed `GateAcquired { lease, prompt_state }` reply, and relays it. The
`prompt_state` carries `Clean | Dirty | NotChecked` per
`signal-persona-terminal::PromptState`. Prototype default: dirty state
defers injection (`InjectionRejected { reason:
DirtyPrompt }`); clean-then-inject machinery is deferred.

**Message-landing endpoint**. The prototype's live
message path terminates here. `persona-harness` calls
`AcquireInputGate { pattern_id }` on the supervisor → forwarded to the
terminal-cell control socket → cell returns `GateAcquired { lease, prompt_state }` →
if `Clean`, harness calls `WriteInjection { lease, bytes,
injection_sequence }` → supervisor forwards to the cell control socket → cell writes bytes
to child PTY → returns `InjectionAck { sequence }` → supervisor relays
back through harness → router commits delivery. The bytes appear in the
fixture cell's transcript; the prototype's witness reads the transcript
to verify the end-to-end path.

## 2 · State and Ownership

The terminal cell owns the child process and PTY. Viewers are disposable
clients. Closing a viewer does not kill the harness.

The production `persona-terminal` supervisor owns the registry around terminal
cells: named sessions, session health, socket paths, viewer attachments, and
Sema-backed durable terminal metadata. The low-level `terminal-cell` session
owns one child process group and one PTY. The supervisor chooses and launches
viewer adapters; the adapters draw windows and forward raw terminal bytes over
the cell's `data.sock`.

The current daemon writes a named session record into the component Sema after
the terminal-cell sockets are bound. The `persona-terminal-sessions` and
`persona-terminal-resolve` binaries are read-only inspection clients for that
Sema state; effect-bearing input, capture, attach, and resize clients still
talk to the terminal socket.

`persona-terminal-signal` is the current contract witness client. It constructs
`signal-persona-terminal` requests, sends them as length-prefixed Signal frames
to a terminal control socket, and renders the resulting terminal event.

This repo ships two distinct daemon binaries with clearly different scopes:

`persona-terminal-daemon` is a **PTY-owning daemon**. It embeds the
`terminal_cell` library to spawn a `TerminalCell` actor, hosts a
`TerminalSignalControl` Kameo actor for prompt-pattern, gate, and injection
control state, and binds **two** sockets — `--control-socket` for the
byte-tag CLI protocol and Signal frames, `--data-socket` for raw
attached-viewer bytes. On startup with a `--name`, it writes a
`SessionRegistration` into the component Sema recording both the control and
data socket paths as separate typed fields. The witness scripts under
`scripts/` use this daemon because they need a real PTY behind the Sema
registry entry.

`persona-terminal-supervisor` is a **registry frontend**, not a PTY owner.
It binds one `signal-persona-terminal` socket (the `PERSONA_SOCKET_PATH`),
answers `SupervisionRequest` traffic on the supervision socket, resolves
named terminals from the component Sema, reads the resolved session's
`control_socket_path`, and **forwards** Signal frames to that terminal-cell
control socket. It records `delivery_attempts` and `terminal_events`. It is
the production engine-facing surface for sending typed control to terminals
owned by separate cell daemons. It does not connect to data sockets.

Both binaries apply `PERSONA_SOCKET_MODE` to the sockets they bind.

The Sema session record stores both terminal-cell socket paths as typed
fields: `control_socket_path` for Signal control requests and
`data_socket_path` for attached viewer byte transport. `persona-terminal`
sends typed Signal control requests only to the control socket; viewer
adapters attach only to the data socket. A single terminal-cell socket that
changes role by mode, message kind, or connection phase is not a valid
shape.

`TerminalSignalControl` is the first Kameo actor in this repo's supervisor
direction. It owns prompt-pattern registry state, signal input-gate leases,
prompt cleanliness checks, and the decision to accept or reject programmatic
injection. The surrounding daemon still owns the socket accept loop and
terminal-cell session shell; future supervisor work should continue splitting
those runtime planes into named actors instead of growing helper methods.

## 3 · Boundaries

This repo owns:

- terminal session registry policy;
- PTY lifecycle;
- viewer attachment;
- raw input and resize frames;
- output scrollback replay;
- terminal transport request/event adaptation.

This repo does not own:

- Persona messages (`persona-message`);
- routing decisions (`persona-router`);
- harness domain identity (`persona-harness`);
- harness provider-usage interpretation (`persona-harness`);
- OS focus policy (`persona-system`);
- authorization.

`persona-harness` is a sibling engine component and a client of this repo's
terminal contract. `persona-terminal` is not a subcomponent of harness; the
engine manager supervises both and pushes their peer socket paths at spawn.

Production registry state lives in `persona-terminal`'s component Sema, not in
viewer-specific files and not in `terminal-cell`. Runtime-directory metadata
remains a convenience cache; the typed terminal registry is the durable source
of truth. The table value record shapes for inspectable terminal state are
owned by `signal-persona-terminal`'s introspection module; this component owns
the redb file, table declarations, write sequencing, and read consistency.

## 4 · Constraints

Each line is an obligation; each load-bearing constraint has a witness in §5.

### 4.1 · Lifecycle and ownership

- The terminal session owner owns one child process group and its PTY for the
  lifetime of the session.
- Viewer attach, detach, close, crash, or replacement never owns or kills the
  child process.
- Terminal transcript is append-only truth. Every output byte read from the
  PTY master receives a terminal generation and sequence before any viewer
  replay, screen projection, or capture result.

### 4.2 · Control plane vs data plane

- `persona-terminal` owns the control plane only. Typed Signal frames flow
  `persona-terminal` ↔ terminal-cell `control.sock`.
- Raw attached-viewer bytes flow viewer ↔ terminal-cell `data.sock` and never
  traverse `persona-terminal`.
- The terminal registry records the terminal-cell control and data socket
  paths as separate typed fields. Signal control clients dial
  `control_socket_path`; viewer adapters dial `data_socket_path`.
- Viewer adapters never connect to the Signal control socket. Signal clients
  never carry live attached-viewer bytes.
- `persona-terminal-daemon` binds **both** listeners. The control listener
  serves the byte-tag CLI protocol and Signal frames; the data listener
  accepts only an attach handshake followed by raw bytes. An attach request
  on the control listener is rejected; any non-attach request on the data
  listener is rejected.
- There is no single-socket mode-shift path between terminal-cell control
  and data roles.
- Slow transcript work in `terminal-cell` does not back-pressure into the
  attached viewer's data plane.

### 4.3 · Daemon and supervisor binaries

- `persona-terminal-daemon` is a PTY-owning daemon. It embeds the
  `terminal_cell` library, hosts `TerminalSignalControl`, binds
  `--control-socket` and `--data-socket`, and on `--name` writes a
  `SessionRegistration` into the component Sema.
- `persona-terminal-supervisor` is a registry frontend. It binds one
  `signal-persona-terminal` socket, answers `SupervisionRequest` traffic,
  resolves named terminals from component Sema, and forwards Signal frames
  to the registered terminal-cell control socket. It does not own a PTY.
- Both binaries apply `PERSONA_SOCKET_MODE` (mode 0600 by default) to every
  socket they bind.
- The supervisor binary accepts explicit `--socket` / `--store` overrides
  for tests; the engine path reads `PERSONA_SOCKET_PATH` and
  `PERSONA_STATE_PATH` from the Persona spawn envelope, not ambient
  environment.

### 4.4 · Reattach and viewer

- Reattach is sequence-based. A viewer reconnects from a known terminal
  sequence, receives replayed transcript bytes, then receives live deltas
  from the same stream.
- Screen state and scrollback views are derived projections. They may use
  `vt100`, `termwiz`, or viewer-native state, but they are never the source
  of truth.

### 4.5 · Wire and registry

- Named terminal sessions are component state. The daemon records them in
  `persona-terminal`'s component Sema; no registry JSON, text manifest, or
  viewer-specific state file is the source of truth.
- The supervisor socket resolves terminal names through component Sema
  before terminal effects. Callers send `signal-persona-terminal` frames to
  `persona-terminal`, not directly to stored terminal-cell sockets.
- Supervisor-request state is committed around the terminal effect:
  `delivery_attempts` before forwarding, `terminal_events` after the typed
  event returns. Viewer attachments, session health, and session archive
  records are first-class component Sema tables.
- Session registration records both the named terminal session (with typed
  control and data socket paths) and the ready-state session-health row in
  component Sema.

### 4.6 · Subscriptions

- Subscription requests are streams, not one-shot lookups. The supervisor
  resolves the named terminal once, forwards the typed subscription frame
  to the registered terminal control socket, relays the initial state and
  each live delta, and records every typed event it observes.
- Subscription close is a typed retract/close request on the control plane.
  The supervisor forwards the retract; the server emits a final
  acknowledgement event; the stream ends. Raw socket close is not semantic
  protocol.

### 4.7 · Input

- Terminal input is raw byte transport. `TerminalInputBytes` reach the PTY
  without Persona-message parsing, shell parsing, slash-command parsing, or
  provider quota semantics in the terminal owner.
- Programmatic input and viewer keyboard input enter through the same
  terminal input port and produce the same accepted/rejected terminal event
  shape.
- Programmatic injection and human keypresses are serialized through one
  PTY writer per cell; the input gate is the writer-side arbitrator.
- Harness slash-command usage probes are harness-adapter behavior. The
  terminal owner may carry bytes such as `/usage\r`, but quota
  interpretation belongs in `persona-harness` or a harness contract.

### 4.8 · Push and scope

- The terminal owner pushes readiness, transcript, resize, detach, capture,
  exit, and rejection events. Polling is not the steady-state observation
  mechanism.
- The terminal owner provides no pane, tab, status-bar, copy-mode,
  prefix-key, or application-level input grammar. Out-of-band control uses
  typed socket or Signal requests; attached keyboard bytes pass to the PTY
  as terminal input.

## 5 · Witnesses

### 5.1 · Lifecycle and viewer

- **Durable owner**: spawn a child that writes after the viewer exits;
  reattach and prove the child is still alive and the detached output is
  replayed.
- **Sequence replay**: attach at sequence N, detach, emit output, reconnect
  from N, and assert replay starts at N+1 before live deltas.

### 5.2 · Control plane vs data plane

- **Two-socket registration**: starting `persona-terminal-daemon` with
  `--name` writes a `SessionRegistration` whose typed
  `control_socket_path` and `data_socket_path` fields point at the daemon's
  bound listeners. Reading the row back through the registry returns both.
- **Supervisor uses control socket**: the supervisor resolves a named
  terminal, reads `control_socket_path` from the Sema session row, and
  forwards Signal frames only to that socket. The data socket is not
  opened by the supervisor.
- **Plane rejection (terminal-cell)**: the underlying terminal-cell daemon
  rejects an `Attach` request on `control.sock` and rejects every
  non-`Attach` request on `data.sock`; the supervisor's typed errors
  reflect these rejections.

### 5.3 · Input

- **Raw pass-through**: send bytes containing escape sequences,
  bracketed-paste markers, and `/usage\r` to a fixture process; assert the
  transcript shows the exact byte path and the terminal crate contains no
  quota or slash parser.
- **Shared input port**: send equivalent bytes through viewer keyboard
  frames and programmatic `TerminalInput`; assert both produce the same
  terminal event path.
- **Harness-owned quota**: a fake harness adapter maps a usage probe to
  raw terminal input and parses a fixture transcript into a harness
  observation; terminal transport contains only byte transport.

### 5.4 · Registry and Sema

- **Component Sema registry**: register a named terminal session, read it
  back with the session inspection CLI, and prove both socket paths came
  from the Sema table. The same witness sets `PERSONA_SOCKET_MODE=600`
  before launching `persona-terminal-daemon` and verifies the
  terminal-cell socket metadata. Exposed as
  `nix run .#test-named-session-registry`.
- **Session-health registration**: register a named terminal session
  through `SessionRegistration`, then read `session_health` and prove a
  ready row with generation 1 exists. Exposed as
  `nix flake check .#terminal-registration-writes-session-health`.
- **T6 table coverage**: write and read `delivery_attempts`,
  `terminal_events`, `viewer_attachments`, `session_health`, and
  `session_archive` through `TerminalTables`; the default flake check
  runs this witness.

### 5.5 · Signal control flow

- **Signal-to-terminal-cell**: start a real terminal-cell-backed daemon,
  resolve its named control socket from Sema, send `TerminalConnection`,
  `TerminalInput`, and `TerminalCapture` through the
  `signal-persona-terminal` adapter, and prove captured bytes came from
  the child PTY. Exposed as `nix run .#test-terminal-signal`.
- **Gate-and-cache injection**: register a prompt pattern, acquire an
  input gate with clean prompt state, send viewer bytes while locked,
  prove those bytes do not reach the PTY before release, inject under
  the lease, release the gate, and prove cached human bytes replay
  afterward. Exposed as `nix run .#test-gate-cache`.
- **Dirty prompt defers injection**: type a human draft before acquiring
  the gate, acquire with a prompt pattern, observe `PromptState::Dirty`,
  attempt injection, and prove the bytes are rejected instead of
  reaching the PTY. Exposed as `nix run .#test-dirty-prompt-defers`.
- **Actor-owned signal control**: the pure test suite asserts
  `TerminalSignalControl` is a Kameo actor with typed messages and that
  production terminal-control state does not use shared
  `Arc<Mutex<_>>` state.

### 5.6 · Supervisor routing

- **Supervisor socket routing**: send one `signal-persona-terminal`
  request to the supervisor socket, prove it resolves the named session
  through component Sema, forwards the frame to the registered terminal
  control socket, records the delivery attempt and terminal event, and
  returns the typed terminal event. Exposed as
  `nix flake check .#terminal-supervisor-socket-routes-through-component-sema`.
- **Supervisor subscription routing**: send
  `SubscribeTerminalWorkerLifecycle` to the supervisor socket, prove it
  records the attempt, relays an initial lifecycle snapshot and a
  following lifecycle delta from the registered terminal control socket,
  and persists both typed events.
- **Spawn-envelope startup**: construct `persona-terminal-supervisor`
  without CLI path arguments and prove it resolves its socket and
  component Sema path from `PERSONA_SOCKET_PATH` and
  `PERSONA_STATE_PATH`.
- **Supervisor socket mode**: bind `persona-terminal-supervisor` with an
  explicit managed socket mode and prove the real Unix socket metadata
  is mode 0600 on the primary supervisor socket.
- **Supervisor binary applies mode**: the spawned
  `persona-terminal-supervisor` binary applies `PERSONA_SOCKET_MODE` to
  both its supervision socket and its primary supervisor socket.

## 6 · Invariants

- Harness processes are durable across viewer close.
- Viewer adapter mode is explicit. The byte path stays in `terminal-cell`; any
  viewer or compositor behavior stays adapter-local.
- This repo transports bytes without interpreting message semantics.
- Reusable stateful workflows are scripts or Nix apps.

## Code Map

```text
src/pty.rs                         terminal-cell daemon/view/client adapter
src/contract.rs                    signal-persona-terminal adapter
src/signal_control.rs              Kameo actor for prompt/gate/injection control state
src/supervisor.rs                  engine-facing Signal supervisor socket
src/tables.rs                      component Sema tables over signal-persona-terminal introspection records
src/registry.rs                    session registration + inspection clients
src/bin/persona-terminal-daemon.rs  daemon entry
src/bin/persona-terminal-view.rs    viewer entry
src/bin/persona-terminal-send.rs    raw input sender
src/bin/persona-terminal-sessions.rs read-only session inspection
src/bin/persona-terminal-resolve.rs  read-only session name resolver
src/bin/persona-terminal-signal.rs   signal terminal request client
src/bin/persona-terminal-supervisor.rs supervisor socket entry
scripts/named-session-registry-witness stateful named-session witness
scripts/terminal-signal-witness      stateful signal-to-terminal-cell witness
scripts/gate-cache-witness           stateful gate-and-cache injection witness
scripts/dirty-prompt-defers-witness  stateful dirty-prompt rejection witness
```

## See Also

- `../persona-harness/ARCHITECTURE.md`
- `../persona-message/ARCHITECTURE.md`
- `../persona-router/ARCHITECTURE.md`
