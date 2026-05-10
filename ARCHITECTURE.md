# persona-wezterm — architecture

*Durable PTY and detachable WezTerm viewer transport for Persona harnesses.*

`persona-wezterm` owns terminal byte transport. It keeps harness child
processes alive in durable PTYs, lets visible WezTerm viewers attach and detach,
and moves raw input/resize/output frames between clients and the PTY. Its
Persona-facing boundary is the typed `signal-persona-terminal` contract.

---

## 0 · TL;DR

This repo moves terminal bytes. It does not understand Persona message
semantics, routing policy, or authorization.

```mermaid
flowchart LR
    "Codex or Claude harness" --- "durable PTY daemon"
    "durable PTY daemon" -->|"output bytes + scrollback"| "visible viewer"
    "visible viewer" -->|"keyboard + resize frames"| "durable PTY daemon"
    "persona-harness" -->|"raw terminal request"| "durable PTY daemon"
```

## 1 · Component Surface

`persona-wezterm` exposes:

- durable PTY daemon binary;
- visible viewer binary;
- raw input sender binary;
- output scrollback replay;
- resize propagation;
- WezTerm mux/socket attachment helpers.
- `signal-persona-terminal` request/event adapter.

## 2 · State and Ownership

The daemon owns the child process and PTY. Viewers are disposable clients.
Closing a viewer does not kill the harness.

## 3 · Boundaries

This repo owns:

- PTY lifecycle;
- WezTerm viewer attachment;
- raw input and resize frames;
- output scrollback replay.
- terminal transport request/event adaptation.

This repo does not own:

- Persona messages (`persona-message`);
- routing decisions (`persona-router`);
- harness domain identity (`persona-harness`);
- OS focus policy (`persona-system`);
- authorization.

## 4 · Invariants

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
