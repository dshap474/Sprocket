# Product Positioning

## Problem

Setting up an AI-native repo still feels manual and fragile.

Teams end up with:

- scattered scripts
- partially installed control planes
- stale docs
- inconsistent hook behavior
- agent-specific setup that does not migrate cleanly

## Product

Sprocket is a single binary that installs, upgrades, and maintains an opinionated repo system.

## First Wedge

The first wedge is narrow on purpose:

- Codex backend
- Python profile
- managed docs
- milestone-time maintenance

## Expansion Path

After the first wedge is solid:

- add Claude Code backend support
- add TS/React and Rust profiles
- add stronger migration and repair workflows
- keep the runtime model unified instead of cloning logic per ecosystem
