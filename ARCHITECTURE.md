# persona-wezterm — architecture

*Durable PTY and detachable WezTerm viewer transport for Persona harnesses.*

`persona-wezterm` is the current owner of terminal byte transport. It keeps
harness child processes alive in durable PTYs, lets visible viewers attach and
detach, records terminal transcript truth, and moves raw input/resize/output
frames between clients and the PTY. Its Persona-facing boundary is the typed
`signal-persona-terminal` contract.

---

## 0 · TL;DR

This repo moves terminal bytes. It does not understand Persona message
semantics, routing policy, provider quota policy, slash-command meaning, or
authorization.

```mermaid
flowchart LR
    "Codex or Claude harness" --- "durable PTY daemon"
    "durable PTY daemon" -->|"sequenced transcript + live bytes"| "visible viewer"
    "visible viewer" -->|"raw keyboard + resize frames"| "durable PTY daemon"
    "persona-harness" -->|"programmatic input bytes"| "durable PTY daemon"
    "persona-harness" -->|"raw terminal request"| "durable PTY daemon"
```

## 1 · Component Surface

`persona-wezterm` exposes:

- durable PTY daemon binary;
- visible viewer binary;
- raw input sender binary;
- output scrollback replay;
- resize propagation;
- WezTerm mux/socket attachment helpers;
- `signal-persona-terminal` request/event adapter.

## 2 · State and Ownership

The daemon owns the child process and PTY. Viewers are disposable clients.
Closing a viewer does not kill the harness.

## 3 · Boundaries

This repo owns:

- PTY lifecycle;
- WezTerm viewer attachment;
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

## 4 · Constraints

- The terminal session owner owns one child process group and its PTY for the
  lifetime of the session.
- Viewer attach, detach, close, crash, or replacement never owns or kills the
  child process.
- Terminal transcript is append-only truth. Every output byte read from the PTY
  master receives a terminal generation and sequence before any viewer replay,
  screen projection, or capture result.
- Reattach is sequence-based. A viewer reconnects from a known terminal
  sequence, receives replayed transcript bytes, then receives live deltas from
  the same stream.
- Screen state and scrollback views are derived projections. They may use
  `vt100`, `termwiz`, or viewer-native state, but they are never the source of
  truth.
- Terminal input is raw byte transport. `TerminalInputBytes` are written to the
  PTY without Persona-message parsing, shell parsing, slash-command parsing, or
  provider quota semantics in the terminal owner.
- Programmatic input and viewer keyboard input enter through the same terminal
  input port and produce the same accepted/rejected terminal event shape.
- Harness slash-command usage probes are harness-adapter behavior. The terminal
  owner may carry bytes such as `/usage\r`, but quota interpretation belongs in
  `persona-harness` or a harness contract.
- The terminal owner pushes readiness, transcript, resize, detach, capture,
  exit, and rejection events. Polling is not the steady-state observation
  mechanism.
- The terminal owner provides no pane, tab, status-bar, copy-mode, prefix-key,
  or application-level input grammar. Out-of-band control uses typed socket or
  Signal requests; attached keyboard bytes pass to the PTY as terminal input.

## 5 · Witnesses

- Durable owner: spawn a child that writes after the viewer exits; reattach and
  prove the child is still alive and the detached output is replayed.
- Sequence replay: attach at sequence N, detach, emit output, reconnect from N,
  and assert replay starts at N+1 before live deltas.
- Raw pass-through: send bytes containing escape sequences, bracketed-paste
  markers, and `/usage\r` to a fixture process; assert the transcript shows the
  exact byte path and the terminal crate contains no quota or slash parser.
- Shared input port: send equivalent bytes through viewer keyboard frames and
  programmatic `TerminalInput`; assert both produce the same terminal event
  path.
- Harness-owned quota: fake harness adapter maps a usage probe to raw terminal
  input and parses a fixture transcript into a harness observation; terminal
  transport contains only byte transport.

## 6 · Invariants

- Harness processes are durable across viewer close.
- Viewer mode is explicit: scrollback mode or application mode.
- This repo transports bytes without interpreting message semantics.
- Reusable stateful workflows are scripts or Nix apps.

## Code Map

```text
src/pty.rs                         PTY daemon model
src/terminal.rs                    terminal frame records
src/contract.rs                    signal-persona-terminal adapter
src/bin/persona-wezterm-daemon.rs  daemon entry
src/bin/persona-wezterm-view.rs    viewer entry
src/bin/persona-wezterm-send.rs    raw input sender
```

## See Also

- `../persona-harness/ARCHITECTURE.md`
- `../persona-message/ARCHITECTURE.md`
- `../persona-router/ARCHITECTURE.md`
- `reports/1-terminal-backend-survey.md`
