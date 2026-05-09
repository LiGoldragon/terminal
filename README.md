# Persona WezTerm

Terminal harness control for Persona.

The crate provides a durable PTY daemon, detachable WezTerm viewer, input sender,
and WezTerm mux delivery helpers.

## PTY input

`persona-wezterm-send <socket> <text>` submits a full prompt by writing text and
Enter. `persona-wezterm-type <socket> <text>` writes raw text without Enter, for
tests that need an occupied prompt buffer.

## PTY capture

`persona-wezterm-capture <socket>` connects to a durable PTY daemon, requests
scrollback replay, writes the current bytes to stdout, and exits. It is a
debugging and guard substrate for higher layers; it does not interpret Persona
messages.
