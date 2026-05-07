# Skill - persona-wezterm

*How to work on Persona's terminal harness control layer.*

---

## What this repo owns

`persona-wezterm` owns terminal transport for Persona harnesses: durable PTY
daemons, visible WezTerm viewer attachment, raw input delivery, resize delivery,
and output scrollback replay.

It does not own Persona messages, NOTA schemas, authorization, or agent
identity. Message-shaped behavior belongs in `persona-message`; this repo only
moves bytes between a harness PTY and clients.

---

## Working rules

Keep harness processes durable. A viewer window is disposable; closing it must
not kill Codex, Claude, or another harness child.

Use `scrollback` viewer mode when the human needs to inspect long transcripts
with terminal scrollback. Use `application` viewer mode when the harness UI
itself needs the full terminal screen and mouse capture.

Stateful commands belong in scripts and Nix apps. If a debugging command becomes
part of the workflow, name it under `scripts/` and expose it from `flake.nix`.

---

## See also

- `ARCHITECTURE.md` - component shape and protocol.
- `persona-message`'s `skills.md` - how the message shim uses this terminal
  layer in real harness tests.
