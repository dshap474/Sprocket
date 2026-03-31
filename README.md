# Sprocket

Sprocket is a Rust binary for bootstrapping AI-native repos and keeping their managed surfaces healthy over time.

The product direction is:

- one installable binary
- first-class support for Codex
- opinionated Python baseline first
- self-healing docs and repo control surfaces
- future backend adapters for Claude Code and other agent runtimes
- future language profiles for TypeScript/React and Rust

## Current Scaffold

This repository currently contains:

- the Rust binary shell
- the first-pass product positioning
- architecture notes for the runtime shape
- brand notes for the `Sprocket` identity

## Product Thesis

Sprocket should feel like a machine you drop into a repo:

- it installs the baseline
- it generates the managed docs
- it wires the agent control plane
- it keeps the important surfaces in sync
- it expands later through adapters and profiles instead of one-off scripts

## Early Product Shape

The first version is planned around:

1. a local CLI binary
2. a Codex adapter
3. a Python profile
4. managed docs generation and milestone refresh
5. repo-aware install, migrate, validate, and repair workflows

## Naming

`Sprocket` is the working product name.

The intended brand feel is:

- mechanical
- memorable
- slightly strange
- not playful in a toy-like way
- credible as a serious developer tool

The mascot direction is a mechanical lobster built from rivets, brass, and articulated workshop parts.
