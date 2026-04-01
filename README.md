# Sprocket

Sprocket is a Rust CLI that installs a Codex adapter and maintains hidden Git checkpoints for Codex-driven repositories.

> Status: early-stage, source-install only, and intentionally narrow in scope. The supported public surface today is `sprocket install codex`; the `hook codex ...` commands are internal hooks that Sprocket installs for Codex to call.

## Installation

Sprocket is not packaged for Homebrew, crates.io, or system package managers yet.

Install it from a local checkout:

```bash
cargo install --path .
```

Or run it without installing:

```bash
cargo run -- install codex --target-repo /path/to/repo
```

## Quick Start

Install the Codex adapter into a target repository:

```bash
sprocket install codex --target-repo /path/to/repo
```

That command currently:

- creates `.sprocket/policy.toml` if it does not exist
- merges Sprocket-managed entries into `.codex/hooks.json`
- writes machine-local runtime state under `git rev-parse --git-path sprocket`
- installs a `prepare-commit-msg` hook that prevents direct commits unless explicitly allowed

Once installed, Codex calls Sprocket through hooks:

1. `session-start` bootstraps the stream and hidden anchor state.
2. `baseline` captures the turn baseline.
3. `pre-tool-use` blocks direct `git add`, `git commit`, `git push`, destructive resets, and similar mutations.
4. `checkpoint` evaluates the turn and materializes a hidden checkpoint commit when policy thresholds are met.

## Command Surface

Public command:

```bash
sprocket install codex [--target-repo <path>]
```

Internal hook commands installed for Codex:

```bash
sprocket hook codex session-start
sprocket hook codex baseline
sprocket hook codex pre-tool-use
sprocket hook codex checkpoint
```

Reserved but not implemented yet:

```text
init
migrate
doctor
repair
validate
```

Use `sprocket --help` for the current CLI surface.

## Configuration

Shared repo policy lives in `.sprocket/policy.toml`. The default install creates a policy with hidden-only checkpoints and conservative thresholds.

Key defaults from the generated policy:

```toml
version = 2

[checkpoint]
mode = "hidden_only"
turn_threshold = 2
file_threshold = 4
age_minutes = 20
default_area = "core"
message_template = "checkpoint({area}): save current work [auto]"
lock_timeout_seconds = 300

[guard]
codex_pretool = true
git_prepare_commit_msg = true
```

The generated policy also includes owned-path, promotion, and compatibility sections. Machine-local runtime state is stored under the Git directory, not in the working tree.

## Current Support Envelope

Sprocket is designed to be conservative. The current checkpoint engine is intentionally limited to:

- attached branch streams
- hidden checkpoints only
- Codex-driven flows with the managed hook path installed

The current implementation rejects or no-ops on several repo states, including:

- detached `HEAD`
- merge, rebase, cherry-pick, or other sequencer state
- sparse checkout
- repos with gitlinks or submodules
- repos containing `.gitattributes`

There is no automatic visible promotion in the current flow.

## Development

Build:

```bash
cargo build
```

Run the test suite:

```bash
just test
```

For the full checkpoint-system contract, see [docs/systems/commit-system.md](docs/systems/commit-system.md).
