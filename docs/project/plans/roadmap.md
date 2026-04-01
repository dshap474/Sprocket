# Roadmap

## Current Status

Sprocket now has a working hidden-checkpoint core for a narrow, safety-first envelope:

- Codex hook integration
- attached-branch streams only
- hidden-only checkpointing
- hidden ref tip as committed authority
- append-only intent log for in-flight recovery
- rebuildable manager and manifest caches
- hermetic integration coverage, including crash/restart recovery paths

This is the current v1 wedge. It is intentionally narrower than the earlier repo-management and profile-expansion plans.

## Phase 1: Trusted Hidden-Only Core

- finish operator-facing `validate`, `doctor`, and `repair` commands
- keep the hidden-only contract explicit in docs and CLI behavior
- harden recovery reporting so users can inspect anchor, intent, and cache state directly
- keep the current support envelope stable while the core earns trust

## Phase 2: Resolve Deferred Algorithm Questions

- decide whether detached `HEAD`, rebase, and sequencer flows stay rejected or get a separate stream model
- decide whether policy epoching needs more explicit user-facing lifecycle or tooling
- design manual visible promotion as a separate explicit command, not part of hook-driven checkpointing
- decide which currently rejected repo features are permanent non-goals versus later support targets

## Phase 3: Controlled Expansion

- add support only after the hidden-only core and repair tooling are trusted
- candidates for later support:
  - manual promotion from hidden checkpoints to visible commits
  - broader repo compatibility after exactness rules are clear
  - backend expansion beyond Codex
  - ecosystem/profile expansion only if it does not fork the runtime model

## Near-Term Non-Goals

- automatic visible promotion
- detached/rewrite workflow support without a separate stream design
- sparse checkout, gitlinks, and `.gitattributes` support in the current engine
- broad profile/backend expansion before repair and validation tooling exist
