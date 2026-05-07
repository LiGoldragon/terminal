# Agent Instructions - Persona WezTerm

You MUST read lore's `AGENTS.md` and the primary workspace
orchestration protocol before editing this repository.

## Repo Role

Persona WezTerm owns the terminal-harness control layer for Persona. It
contains the library and binaries that spawn durable PTYs, attach visible
WezTerm viewers, send input frames, resize harnesses, and keep terminal output
available for later capture.

## Boundaries

This repo owns terminal transport and presentation. It does not own Persona
message records, authorization, agent identity, or the Persona state reducer.
Those layers depend on this crate when they need terminal delivery.

## Version Control

This is a Git-backed colocated Jujutsu repository. Use `jj` for local history
work and keep Git as the remote/storage compatibility layer.

## Rust

Follow lore's Rust discipline: domain values are typed, behavior lives on the
types that own the data, errors use this crate's typed error enum, and reusable
verbs are methods rather than free functions.
