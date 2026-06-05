# INTENT — terminal

*What the psyche has explicitly intended for this project. Synthesised
from psyche statements and the applicable workspace constraints; not
embellished. Maintenance: `primary/skills/repo-intent.md`.*

`terminal` is the Persona-facing terminal session owner: named terminal
sessions, the typed Signal communication surface, the viewer-adapter
launch policy, and the component Sema metadata around `terminal-cell`.
It is the boundary component that transports terminal bytes without
interpreting their meaning. Paired with the contract repos
`signal-terminal` (ordinary terminal transport vocabulary) and
`owner-signal-terminal` (owner-only session lifecycle mutation).

## Repo-scope only

This file carries daemon-side intent for `terminal`. Wire vocabulary
stays in `signal-terminal/INTENT.md` and
`owner-signal-terminal/INTENT.md`. Workspace-shape intent stays in
`primary/INTENT.md`. The low-level PTY primitive is `terminal-cell`.

## Goals

- Own the Persona terminal **communication plane** — typed Signal
  sessions, prompt-pattern lifecycle, input-gate leasing, and
  injection decisions — as one component daemon with internal
  data-bearing session actors around `terminal-cell`.
- Make named terminal sessions durable component state recorded in the
  component's Sema database, not in registry JSON, text manifests, or
  viewer-specific state files.

## Constraints

- **Transport bytes, do not interpret semantics.** The terminal owner
  carries raw terminal input to the child PTY without Persona-message
  parsing, shell parsing, slash-command parsing, or provider-quota
  interpretation. Quota and harness-prompt meaning belong in
  `persona-harness`; routing belongs in `router`; OS focus belongs in
  `system`; authorization is not owned here.
- **Communication plane and data plane are separate sockets.** Ordinary
  `signal-terminal` frames flow on the component communication socket;
  raw attached-viewer bytes flow viewer ↔ session data socket ↔
  `terminal-cell` and never traverse the communication socket. A single
  socket that changes role by mode, message kind, or connection phase
  is not a valid shape.
- **Session-lifecycle mutation is owner-only.** `CreateSession` and
  `RetireSession` arrive only through `owner-signal-terminal`; ordinary
  terminal Signal can only **read** the registry (`ListSessions`,
  `ResolveSession`).
- **Inter-component traffic is Signal; NOTA renders only at edges.**
  The single-argument NOTA rule governs the CLI and config surfaces;
  the daemon's external surface is signal-frame frames. No NOTA on the
  wire between components.
- **State-bearing runtime is actors, not shared mutable state.**
  `TerminalSignalControl` is a Kameo actor owning prompt-pattern
  registry, input-gate leases, and injection decisions; production
  terminal-control state does not use shared `Arc<Mutex<_>>`.
- **Push, do not poll.** Readiness, transcript, resize, detach,
  capture, exit, and rejection are pushed events; subscriptions are
  streams closed by a typed retract, not by raw socket close.

## Anti-patterns

- Terminal-brand mux helpers (panes, tabs, status bars, copy-mode,
  prefix keys, application-level input grammar) are **retired**. Viewer
  and compositor behaviour stays adapter-local behind this same owner
  and must not become a repository boundary.
- The standalone `terminal-cell-daemon` is a development/test harness,
  not the production Persona runtime boundary; production consumes
  `terminal-cell` as a library inside `terminal-daemon`.

## See also

- `ARCHITECTURE.md` — communication/data split, prompt-pattern
  lifecycle, gate-and-acquire execution, registry tables, witnesses.
- `../terminal-cell/INTENT.md` — the low-level PTY/transcript cell.
- `../signal-terminal/INTENT.md` — ordinary terminal transport contract.
- `../owner-signal-terminal/INTENT.md` — owner-only session lifecycle.
- `primary/skills/component-triad.md` — triad structure and wire layers.
