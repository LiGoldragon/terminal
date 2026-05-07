# persona-wezterm skill

Work here when the change concerns durable PTYs, WezTerm viewers, raw input
delivery, resize delivery, or scrollback replay.

Rules for work here:

- Keep harness processes durable. Closing a viewer must not kill the child
  harness process.
- Keep Persona message semantics out of this repo.
- Use scrollback viewer mode when the human needs terminal scrollback.
- Use application viewer mode when the harness owns the full terminal surface.
- Name repeatable stateful workflows under `scripts/` and expose them from
  `flake.nix`.

