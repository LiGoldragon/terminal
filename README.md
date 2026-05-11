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

## Named sessions

`persona-terminal-daemon --store <terminal.redb> --name <terminal> --socket
<socket> -- <command> [args...]` starts a terminal cell and records the named
session in the component Sema database after the socket is bound.

`persona-terminal-sessions --store <terminal.redb>` prints the registered
sessions. `persona-terminal-resolve --store <terminal.redb> <terminal>` prints
the socket path for one registered session. These are read-only inspection
clients for testing and operations; effect-bearing input and capture still go
through the terminal socket.

`nix run .#test-named-session-registry` starts a named daemon, resolves the
socket through the Sema-backed registry, sends input through the resolved
socket, and captures the transcript artifact. It is a stateful host-PTY witness,
so it is exposed as a flake app rather than a pure builder check.

## Signal contract witness

`persona-terminal-signal --socket <socket> --terminal <terminal> connect`
builds a `signal-persona-terminal` request, round-trips it through a
`signal-core` frame, sends it through the Persona terminal transport binding,
round-trips the returned event through a `signal-core` reply frame, and prints
one event line.

`nix run .#test-terminal-signal` starts a real terminal-cell-backed
`persona-terminal-daemon`, resolves the terminal socket from the Sema-backed
registry, sends a `TerminalInput` request through `persona-terminal-signal`,
and captures the resulting transcript with a `TerminalCapture` request.
