# Terminal

Terminal harness control for Persona.

The crate provides a durable terminal-cell daemon, detachable viewer, input
sender, capture client, and `signal-terminal` adapter. `terminal-cell`
owns the low-level PTY and transcript machinery. This crate is the
Persona-facing terminal owner.

## PTY input

`terminal-send <socket> <text>` submits a full prompt by writing text and
Enter. `terminal-type <socket> <text>` writes raw text without Enter, for
tests that need an occupied prompt buffer.

## PTY capture

`terminal-capture <socket>` connects to a durable terminal-cell daemon,
requests scrollback replay, writes the current bytes to stdout, and exits. It is
a debugging and guard substrate for higher layers; it does not interpret Persona
messages.

## Named sessions

`terminal-daemon --store <terminal.redb> --name <terminal>
--control-socket <control.sock> --data-socket <data.sock> -- <command>
[args...]` starts a terminal cell and records the named session (pointing at
the control socket) in the component Sema database after both sockets are
bound. The control socket carries Signal frames and the byte-tag CLI
protocol; the data socket carries the attached-viewer raw byte stream.

`terminal-sessions --store <terminal.redb>` prints the registered
sessions. `terminal-resolve --store <terminal.redb> <terminal>` prints
the socket path for one registered session. These are read-only inspection
clients for testing and operations; effect-bearing input and capture still go
through the terminal socket.

`nix run .#test-named-session-registry` starts a named daemon, resolves the
socket through the Sema-backed registry, sends input through the resolved
socket, and captures the transcript artifact. It is a stateful host-PTY witness,
so it is exposed as a flake app rather than a pure builder check.

## Signal contract witness

`terminal-signal --control-socket <control.sock> --terminal <terminal> connect`
builds a `signal-terminal` request, round-trips it through a
`signal-core` frame, sends it through the Persona terminal transport binding,
round-trips the returned event through a `signal-core` reply frame, and prints
one event line.

`nix run .#test-terminal-signal` starts a real terminal-cell-backed
`terminal-daemon`, resolves the terminal socket from the Sema-backed
registry, sends a `TerminalInput` request through `terminal-signal`,
and captures the resulting transcript with a `TerminalCapture` request.
