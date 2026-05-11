# Persona Terminal

Terminal harness control for Persona.

The crate provides a durable terminal-cell daemon, detachable viewer, input
sender, capture client, and `signal-persona-terminal` adapter. `terminal-cell`
owns the low-level PTY and transcript machinery. This crate is the
Persona-facing terminal owner.

## PTY input

`persona-terminal-send <socket> <text>` submits a full prompt by writing text and
Enter. `persona-terminal-type <socket> <text>` writes raw text without Enter, for
tests that need an occupied prompt buffer.

## PTY capture

`persona-terminal-capture <socket>` connects to a durable terminal-cell daemon,
requests scrollback replay, writes the current bytes to stdout, and exits. It is
a debugging and guard substrate for higher layers; it does not interpret Persona
messages.
