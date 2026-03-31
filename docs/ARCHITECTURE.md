# Sprocket Architecture

## Intent

Sprocket is a local-first repo bootstrap and maintenance engine.

The long-term shape is not "a pile of setup scripts." It is a single runtime with three extensibility axes:

- agent backend adapters
- language/framework profiles
- managed document/control-plane surfaces

## Core Runtime Layers

### CLI

The CLI is the stable user entry point.

Early command families should likely include:

- `init`
- `install`
- `migrate`
- `doctor`
- `repair`
- `validate`

### Engine

The engine owns:

- repo discovery
- file generation
- install planning
- migration logic
- managed-surface diffing
- validation orchestration

This is the product core and should stay backend-agnostic.

### Backends

Backends connect Sprocket to agent runtimes.

Planned order:

1. Codex
2. Claude Code
3. future adapters if they justify the maintenance burden

Backends should define:

- control-plane file locations
- hook/event wiring
- worker invocation mechanics
- backend-specific rules/config generation

### Profiles

Profiles define opinionated repo baselines.

Planned order:

1. Python
2. TypeScript/React
3. Rust

Profiles should define:

- baseline file set
- validation toolchain
- docs expectations
- generated templates
- migration rules from older profile revisions

### Managed Surfaces

Managed surfaces are files Sprocket understands and may regenerate, repair, or refresh.

Examples:

- agent control-plane files
- architecture docs
- project index docs
- validation config

The key product rule is that managed surfaces must be explicit. Sprocket should know what it owns, what it seeds once, and what it only updates on milestones.

## Initial Module Direction

An initial Rust layout should likely grow toward:

```text
src/
  main.rs
  cli/
  engine/
  repo/
  backends/
  profiles/
  docs/
  validation/
```

Keep the first implementation tight. Avoid abstract plugin systems before the Python + Codex path is real.
