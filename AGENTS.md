# Agent Instructions - Terminal

You MUST read lore's `AGENTS.md` and the primary workspace
orchestration protocol before editing this repository.

## Repo Role

Terminal is archived/inactive until further notice. Do not route V1 harness
Claude/Codex tests through this repo; use `terminal-cell` directly as the
active terminal primitive. The older Persona-facing owner design in this repo
is reference material unless the psyche explicitly reactivates it.

## Boundaries

While archived, this repo owns no active runtime boundary. Its historical
scope was terminal transport, viewer attachment, and terminal session
metadata. It does not own Persona message records, authorization, agent
identity, harness quota interpretation, or the Persona state reducer.
Terminal-brand mux helpers remain retired.

## Version Control

This is a Git-backed colocated Jujutsu repository. Use `jj` for local history
work and keep Git as the remote/storage compatibility layer.

## Rust

Follow lore's Rust discipline: domain values are typed, behavior lives on the
types that own the data, errors use this crate's typed error enum, and reusable
verbs are methods rather than free functions.
