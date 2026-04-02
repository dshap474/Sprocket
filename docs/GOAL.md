# Sprocket Goal

The product goal is stricter than generic checkpointing.

Sprocket should let each Codex thread, from launch to archive, maintain its own record of which files it changed over time so that it can later auto-commit only the work that thread actually owns.

That means the system should support:

- thread-scoped change ownership
- a persistent thread baseline
- an evolving set of files touched by that thread
- detection of overlap or interference from other Codex threads on the same branch or worktree
- a final safe commit set that contains only that thread's work

This is different from merely detecting that the repo is dirty or cutting hidden checkpoints of overall repo state. The stronger requirement is per-thread attribution strong enough to support narrow auto-commits in a shared branch workflow.
