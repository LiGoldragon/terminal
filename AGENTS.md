# Agent Instructions - Terminal

You MUST read lore's `AGENTS.md` and the primary workspace
orchestration protocol before editing this repository.

## Repo Role

Terminal owns the terminal-harness control layer for Persona. It
contains the library and binaries that spawn durable terminal cells, attach
visible viewers, send input frames, resize harnesses, and keep terminal output
available for later capture. `terminal-cell` is the low-level PTY/transcript
primitive; this repo is the Persona-facing owner around it.

## Boundaries

This repo owns terminal transport, viewer attachment, and terminal session
metadata. It does not own Persona message records, authorization, agent
identity, harness quota interpretation, or the Persona state reducer.
Terminal-brand mux helpers are retired; the owner noun is `terminal`
around `terminal-cell`.

## Version Control

This is a Git-backed colocated Jujutsu repository. Use `jj` for local history
work and keep Git as the remote/storage compatibility layer.

## Rust

Follow lore's Rust discipline: domain values are typed, behavior lives on the
types that own the data, errors use this crate's typed error enum, and reusable
verbs are methods rather than free functions.
