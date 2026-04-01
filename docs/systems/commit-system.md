# Sprocket Hidden Checkpoint System

This document describes the current Sprocket checkpoint engine after the hidden-only authority refactors.

It is the public contract for the current implementation, not a future design sketch.

## Purpose

Sprocket provides a bounded, local-only checkpoint system for Codex-driven workflows.

The current engine is intentionally narrow:

- attached branch streams only
- hidden checkpoints only
- no automatic visible promotion
- no detached `HEAD`, sequencer, sparse checkout, gitlinks, or `.gitattributes` repos

Within that envelope, the system aims to be crash-safe and restart-safe.

## Authority Model

Sprocket has two authoritative surfaces:

- committed checkpoint authority: the hidden ref tip under `refs/sprocket/checkpoints/...`
- in-flight checkpoint authority: the append-only intent log under the stream runtime

Everything else is a cache:

- `manager.json`
- cached manifests under `manifests/`
- journal summaries

If cache files are missing or stale, Sprocket rebuilds them from the hidden ref tip plus the intent log. Cache corruption must not be able to redefine the current committed anchor.

## Runtime Layout

Shared repo policy lives under:

```text
repo/.sprocket/policy.toml
```

Machine-local runtime lives under:

```text
git rev-parse --git-path sprocket
```

Current layout:

```text
<git-path sprocket>/
  local.toml
  checkpoint.lock
  turns/
    <encoded-session>/<encoded-turn>.json
  streams/
    <stream-id>/
      manager.json
      sessions/
      manifests/
      intents/events.ndjson
      journal/events.ndjson
```

Turn files are global to the runtime. Manager, intent, journal, and manifest caches are stream-local.

## Stream Model

Automatic checkpointing supports one stream class:

- `Branch { symref }`

Stream identity is derived from:

- worktree identity
- attached branch symbolic ref

Detached `HEAD` is explicitly rejected in automatic mode.

## Checkpoint Transaction

The checkpoint transaction is:

1. resolve stream and support gate
2. acquire the repo lock
3. reconcile hidden ref, intents, and caches
4. load the turn
5. capture the current snapshot under the lock
6. classify against the current anchor using the materialized fingerprint
7. if materializing:
   - build the hidden commit object
   - append `Prepared`
   - CAS update the hidden ref
   - append `RefUpdated`
   - rebuild and save caches from the hidden ref tip
   - append `Finalized`
   - delete the turn
   - append the stop decision

There is no automatic visible promotion in this flow.

## Intent Phases

Checkpoint intents are append-only records with these phases:

- `Prepared`
- `RefUpdated`
- `Finalized`
- `Aborted`

Recovery rules:

- `Prepared` with no matching hidden ref move becomes `Aborted`
- `Prepared` or `RefUpdated` whose commit is the hidden ref tip become `Finalized`
- repeated recovery passes are idempotent

Intent identity is keyed by the hidden checkpoint commit OID plus an intent UUID.

## Checkpoint Metadata

Each hidden checkpoint commit must include these footers:

- `Sprocket-Generation`
- `Sprocket-Policy-Epoch`
- `Sprocket-Stream-Class`
- `Sprocket-Observed-Head-Ref`
- `Sprocket-Observed-Head-Oid`
- `Sprocket-Materialized-Fingerprint`
- optional `Sprocket-Observed-Fingerprint`

Required footer parse failures are recovery failures. Sprocket does not silently substitute runtime cache values for missing checkpoint metadata.

## Exactness Contract

Sprocket tracks two notions of fingerprint:

- `materialized_fingerprint`
  - authoritative
  - derived from `(path, mode, git_oid)` of the checkpoint tree
  - used for convergence, anchor identity, and recovery
- `observed_fingerprint`
  - diagnostic only
  - derived from raw worktree bytes
  - never used as the sole recovery key

This means checkpoint authority is Git-materialized, not raw-byte-defined.

## Support Envelope

Sprocket currently rejects these states early with journaled no-ops:

- detached `HEAD`
- merge in progress
- rebase in progress
- cherry-pick in progress
- sequencer state
- sparse checkout
- owned gitlinks / submodules
- any `.gitattributes` file in the repo
- non-hidden-only checkpoint modes

## Failure Injection

For tests, Sprocket supports deterministic injected failures through:

```text
SPROCKET_FAIL_AT=<phase>
```

Supported phases:

- `after_commit_object`
- `after_prepared`
- `after_hidden_ref_cas`
- `after_ref_updated`
- `after_cache_save`
- `after_finalized`
- `after_turn_delete`

When unset, production behavior is unaffected.

## Current Non-Goals

These are intentionally out of scope for the current engine:

- manual or automatic visible promotion
- detached / rewrite stream modeling
- sparse checkout support
- gitlink/submodule checkpointing
- filter-aware `.gitattributes` support
- Windows portability
