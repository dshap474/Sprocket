Verdict: Sprocket is a promising v1 checkpoint core, but it is not yet the universal self-healing algorithm you described. As written, it is strongest as a conservative local autosave layer for bounded paths in a controlled repo. To become “drop into any codebase, multiple agents, same branch, eventually safe,” you need a tighter convergence contract and a few structural changes.

The prior art breaks into a few clear camps.

Aider is the closest mainstream prior art on the Git side. It auto-commits whenever it edits a file, can first commit preexisting dirty changes to keep user edits separate, and generates commit messages from diffs and chat history. That simplicity buys strong rollback, but it also shows the downside of eager commits: Aider has had user-reported cases where unrelated dirty changes were swept into a commit with an inappropriate message, and later reports show that even its git-mode flags can interact in surprising ways. ([Aider][1])

Another camp treats checkpointing as session recovery rather than Git history. Claude Code says it captures the state before each edit, creates a new checkpoint on every user prompt, persists checkpoints across sessions, but explicitly does not track bash-side file changes or most external concurrent changes, and says checkpoints are not a replacement for version control. Cursor’s docs describe checkpoints as snapshots of the codebase created before significant changes. Replit similarly creates logical checkpoints, captures much more than code, and still recommends Git commits for long-term collaboration. ([Claude][2])

The multi-agent systems mostly dodge the hardest part of your problem by isolating work. GitHub Copilot coding agent runs in its own ephemeral development environment and automates branch creation, commits, pushing, and PR creation; OpenHands’ workflow examples likewise create feature branches and pull requests rather than letting several agents mutate one dirty local branch directly. ([GitHub Docs][3])

A fourth camp keeps provenance separate from the user’s visible commit stream. Entire stores session metadata on a separate `entire/checkpoints/v1` branch and explicitly says it never creates commits on your active branch. Git AI has agents mark checkpoints while they work, then condenses that into an authorship log attached to the eventual commit via Git notes, with explicit support for surviving rebases, merges, and cherry-picks. ([GitHub][4])

On the research side, your intuition is valid. EvoGit treats Git-like branching as the coordination substrate for decentralized multi-agent coding, and GCC treats `COMMIT`, `BRANCH`, `MERGE`, and `CONTEXT` as the primitives for long-horizon agent memory and cross-session reuse. The important detail is that both lean into versioned branching structures, not a single always-dirty shared trunk. ([arXiv][5])

What is already good in your design:

* The canonical-home vs adapter split is correct. Keeping `/.sprocket/` as product state and `/.codex/` as adapter surface is a good long-term architecture.
* The turn-baseline vs last-checkpoint-baseline distinction is the right core idea. That is what lets you avoid “commit on every stop” and instead commit only when meaningful repo state changed.
* Bounded staging through `owned_paths` is the correct safety instinct. Repo-wide auto-commit is how these systems become untrustworthy.
* The bootstrap baseline prevents the bogus “first run commits the whole repo” failure.
* Locking plus soft-fail semantics are conservative, which is the right default for an autosave system.

What is wrong or incomplete against your stated goal:

* Your stated goal and the current spec do not match. You explicitly omit startup dirty-state adoption, session registry, stale-session handling, and milestone logic, but those are exactly the features required for “prior session dirt,” “multiple agents on one branch,” and “eventual healing.”
* There is a time-of-check/time-of-use bug in the checkpoint path. In your spec, the current snapshot is taken before the repo lock is acquired. If another agent or human edits files after that snapshot but before staging/commit, you can classify one state and commit another. The authoritative snapshot has to be taken inside the lock.
* The early-exit rule `snapshot == turn baseline => no-op` leaves inherited dirty state unhealed. If prior-session work already exists when a new turn starts, and that turn makes no further code edits, your system will keep no-op’ing forever even though the repo is still dirty. You do bootstrap the very first run, but you do not have a general later-session adoption path.
* Your field naming is inconsistent on dirty bootstrap. `last_checkpoint_commit = current_head()` does not actually correspond to `last_checkpoint_fingerprint` when the working tree is dirty, because no commit exists for that fingerprint. That field is really “observed head at baseline,” not “last checkpoint commit.”
* Same-branch multi-agent operation will mix authorship. Because your decision diff is global (`last_checkpoint_manifest` vs current snapshot), one agent can end up committing another agent’s or a human’s edits if they land between baseline and stop.
* `owned_paths = ["src","tests"]` is not drop-in for arbitrary repos. Real repos concentrate meaningful files in `app`, `lib`, `cmd`, `packages`, `crates`, `services`, root configs, lockfiles, migrations, infra, and docs.
* Your repo-local config model is too invasive for a supposedly drop-in local tool. A machine-specific absolute `binary_path` in `.sprocket/sprocket.toml` is not portable if tracked, and noisy if untracked. Modifying the repo’s `.gitignore` is also heavy-handed for a local-only install.

There is also a hard platform constraint here: Codex hooks are currently experimental, Windows is disabled, multiple matching hooks run concurrently, multiple hook files are all loaded, and `PreToolUse` only intercepts Bash. OpenAI’s own docs say the model can work around that by writing a script and then running it with Bash, so it is only a guardrail, not a real enforcement boundary. Unsupported `PreToolUse` outputs also fail open. That means “Sprocket is the sole commit path” is not enforceable with your current Codex-hook strategy alone. ([OpenAI Developers][6])

The most important design correction is this: do not promise perfection on an unconstrained shared branch. Promise convergence under a declared safety envelope.

A workable convergence contract would sound like this: given finite external mutations, no history rewrite during a checkpoint attempt, a complete owned surface, and successful lock acquisition, Sprocket guarantees eventual creation of a local checkpoint whose owned-path tree matches the working tree. That is honest and defensible. “Always solves any messy repo under arbitrary concurrent mutation” is not.

What I would change first, in order:

1. Split state into three classes: committed policy, local adapter state, and ephemeral runtime state. Machine-local binary paths and hook wiring should be ignored local state, not repo-portable config.

2. Recompute the authoritative snapshot inside the lock, then stage from that exact manifest, then verify the staged/index fingerprint before committing, and finally update manager state from the committed result rather than a pre-lock observation.

3. Add an explicit adoption/repair state machine. You already have enough information to distinguish:

   * no delta vs checkpoint,
   * inherited dirty state present at turn start,
   * new work introduced during this turn,
   * mixed state.

   Those should not all map to the same `checkpoint` outcome. You want at least `noop`, `adopt`, `checkpoint`, and `repair/quarantine`.

4. Use `last_checkpoint_commit` properly for drift detection. On every baseline/stop, compare current `HEAD` to the last known checkpoint ancestry. If the branch changed, rebased, or diverged, do not blindly continue normal checkpoint logic.

5. Stop putting every autosave directly onto the user-visible branch. This is the single biggest strategic improvement. Create actual Git commits, but store them on a hidden ref or side branch such as `refs/sprocket/checkpoints/...`, then promote to the visible branch only when a commit is coherent enough. That gives you Git-native restore without polluting normal history, and it aligns with what Entire and Git AI are doing in different forms. ([GitHub][4])

6. Strengthen commit ownership beyond `PreToolUse`. If you really want Sprocket to own commits, use Git-level enforcement too: a local `pre-commit` or `commit-msg` hook that only allows commits when Sprocket presents a token/env marker. Codex-side command blocking is still useful, but it is not sufficient.

7. Use `session_id + turn_id` as the turn key, not only `turn_id`. Codex surfaces both, and that gives you a safer namespace for concurrent sessions. Also consider using `SessionStart` for inherited-dirty adoption and stale-turn cleanup, because the hook substrate now supports session lifecycle events in addition to turn hooks. ([OpenAI Developers][6])

8. Add a validator loop. Codex `Stop` hooks can continue the agent by returning a continuation reason, which gives you a deterministic place to say “tests failed; fix them before checkpointing.” That is a strong path toward your “safe over time” goal. ([OpenAI Developers][6])

My blunt assessment is:

Sprocket v1 is a good conservative checkpoint engine for a single working tree. It is not yet a general multi-agent convergence system.

The best version of Sprocket is not “Aider but smarter thresholds.” It is:

* Git-native hidden checkpoints,
* exact-manifest bounded commits,
* explicit adopt/repair states,
* drift detection,
* stronger commit ownership,
* optional promotion from hidden checkpoints to visible branch commits.

That puts you in a real product gap. Aider proves eager commit. Claude/Cursor/Replit prove checkpoint UX. Copilot/OpenHands prove that serious multi-agent workflows want isolation. Entire/Git AI prove that metadata should often live beside commits, not inside them. Sprocket can combine those into something distinct: local, repo-agnostic, agent-aware, Git-native recovery with policy-driven promotion to clean history. ([Aider][1])

If you want, the next useful step is for me to turn this into a concrete v2 design: state machine, invariants, recovery rules, and commit/adoption/repair pseudocode.

[1]: https://aider.chat/docs/git.html "https://aider.chat/docs/git.html"
[2]: https://code.claude.com/docs/en/checkpointing "https://code.claude.com/docs/en/checkpointing"
[3]: https://docs.github.com/copilot/concepts/agents/coding-agent/about-coding-agent "https://docs.github.com/copilot/concepts/agents/coding-agent/about-coding-agent"
[4]: https://github.com/entireio/cli "https://github.com/entireio/cli"
[5]: https://arxiv.org/abs/2506.02049 "https://arxiv.org/abs/2506.02049"
[6]: https://developers.openai.com/codex/hooks/ "https://developers.openai.com/codex/hooks/"

---

The main design correction is this:

v1 treats the unit of work as “did this turn change relative to its own baseline?”
v2 needs to treat the unit of work as “is the workspace dirty relative to the last materialized anchor?”

Turn-local deltas still matter, but only for provenance and urgency. The actual convergence loop must be anchor-relative. That single change fixes the biggest starvation bug in v1: inherited dirty state can no longer sit around forever just because the current turn made no further edits.

Two external constraints shape the design. Codex hooks are currently experimental; matching hooks from multiple files all run, multiple matching command hooks for the same event are launched concurrently, `SessionStart` plus turn-scoped hooks are available, `Stop` can continue the model by returning a block reason, and `PreToolUse` only intercepts Bash and is explicitly not a complete enforcement boundary. Git, meanwhile, gives you exactly the primitives you need for a safer checkpoint core: refs that can live under their own namespace, safe ref movement via `update-ref`, tree creation from an index via `write-tree`, commit creation via `commit-tree`, worktrees with separate per-worktree `HEAD` and `index`, and hook installation under `$GIT_DIR/hooks` or a configured `core.hooksPath`. `prepare-commit-msg` is not suppressed by `--no-verify`, while `pre-commit` and `commit-msg` are. ([OpenAI Developers][1])

## Hard call: make hidden checkpoints the primitive

Do **not** make “visible commit on the checked-out branch” the primitive.

Make this the primitive instead:

* exact checkpoint commit under `refs/sprocket/checkpoints/...`
* built from an anchor-relative owned-surface snapshot
* without touching the user’s main index
* with optional later promotion to visible history

That is the only version that can get close to “drop into any codebase, shared branch, overlapping sessions, eventual healing” without taking ownership of the user’s staging area.

Reason: a visible commit on the checked-out branch is not just a commit operation. It is a commit **plus index synchronization** problem. If you create a visible commit without also reconciling the main index, `git status` becomes inconsistent. If you do reconcile the main index, you are now mutating user staging state. Hidden checkpoints avoid that trap completely.

## Target guarantee

You cannot honestly guarantee convergence under arbitrary external mutation forever. You **can** guarantee this:

Given finite external mutations, a stable owned surface, successful lock acquisition, and no repository corruption, Sprocket will eventually materialize a checkpoint commit whose owned-path tree matches a real observed workspace state, and inherited dirty state will not starve.

That is the right contract.

---

## Opinionated codebase architecture

Use a strict split between pure decision logic and side-effecting adapters.

```text
src/
  lib.rs
  main.rs
  cli.rs

  app/
    install_codex.rs
    session_start.rs
    baseline.rs
    checkpoint.rs
    pre_tool_use.rs

  domain/
    policy.rs
    ids.rs
    snapshot.rs
    manifest.rs
    delta.rs
    manager.rs
    turn.rs
    session.rs
    decision.rs
    errors.rs

  engine/
    observe.rs
    classify.rs
    materialize_hidden.rs
    promote_visible.rs
    repair.rs

  infra/
    git.rs
    git_plumbing.rs
    refs.rs
    temp_index.rs
    lock.rs
    fs_store.rs
    manifest_store.rs
    journal.rs
    clock.rs

  codex/
    payload.rs
    responses.rs
    hooks_json.rs
```

Best-practice rules:

* `main.rs` stays thin.
* `domain/*` is pure and deterministic.
* `engine/*` orchestrates workflows but does not shell out directly.
* all Git interaction goes through one typed `Git` adapter
* all state writes are atomic temp-file-plus-rename writes
* never parse human-oriented Git output
* use `-z` machine formats everywhere
* never trust mtimes for correctness
* never mutate the user’s main index in the hidden-checkpoint core
* keep a small append-only NDJSON journal for every decision

I would also stop storing machine-local runtime under `.sprocket/state` in the working tree. Git linked worktrees have private per-worktree `HEAD` and `index`, shared refs, and `git rev-parse --git-path` resolves the correct path depending on whether the thing is per-worktree or shared. Use that for runtime state instead of editing `.gitignore` and putting machine-local data under repo-visible paths. Keep only shared policy under `.sprocket/`. ([Git][2])

Recommended layout:

```text
repo/
  .sprocket/
    policy.toml              # shared repo policy, optional to commit
  .codex/
    hooks.json               # Codex adapter file
  <git-path sprocket>/
    local.toml               # machine-local install/runtime config
    manager.json
    sessions/
    turns/
    manifests/
    journal/
    checkpoint.lock
```

`local.toml` should contain `binary_path`. Do not put absolute binary paths in shared policy.

---

## Step-by-step implementation spec

## 1. Split policy from runtime

### Shared policy

Create `.sprocket/policy.toml`.

This is the only repo-visible Sprocket config.

Example:

```toml
version = 2

[owned]
include = ["."]
exclude = [
  ".git",
  ".sprocket",
  "node_modules",
  "target",
  "dist",
  "build",
  ".next",
  "coverage",
  ".venv"
]

[checkpoint]
mode = "hidden_only"         # hidden_only | hidden_then_promote | visible_direct
turn_threshold = 2
file_threshold = 4
age_minutes = 20
message_template = "checkpoint({area}): save current work [auto]"
default_area = "core"

[promotion]
enabled = false
validators = []
continue_on_failure = true

[guard]
codex_pretool = true
git_prepare_commit_msg = true
```

### Local runtime

Resolve runtime root with `git rev-parse --git-path sprocket`.

Store:

* binary path
* install version
* worktree id
* state files

This removes the need to patch `.gitignore`.

### Why

Your current `.sprocket/state/` design is workable, but it is the wrong home for machine-local runtime. It creates repo noise and gets awkward under linked worktrees.

---

## 2. Make the anchor a hidden ref, not “last visible commit”

Use a hidden ref per worktree + head stream.

Suggested format:

```text
refs/sprocket/checkpoints/v2/<worktree-id>/<stream-key>
```

Where:

* `worktree-id` = stable hash of canonical worktree root
* `stream-key` = sanitized symbolic HEAD ref, or `detached-<short-oid>`

Examples:

```text
refs/sprocket/checkpoints/v2/7b1d.../refs-heads-main
refs/sprocket/checkpoints/v2/7b1d.../detached-a1b2c3d
```

Why hidden refs:

* safe Git-native restore point
* no user-visible branch pollution
* survives branch rewrites better than manager-only metadata
* lets you keep checkpoint history independent from visible history

Git refs live in the ref namespace and should be updated through `git update-ref` rather than hand-editing files. ([Git][3])

---

## 3. Replace the manager model

Rename the core notion from “last checkpoint” to “anchor”.

`anchor` means: the last Sprocket-materialized checkpoint commit and its corresponding owned-surface fingerprint.

Use this shape:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerState {
    pub version: u32,
    pub worktree_id: String,
    pub generation: u64,
    pub anchor: AnchorState,
    pub pending: Option<PendingEpisode>,
    pub last_seen: Option<Observation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorState {
    pub hidden_ref: String,
    pub checkpoint_commit_oid: String,
    pub observed_head_oid: Option<String>,
    pub observed_head_ref: Option<String>,
    pub fingerprint: String,
    pub manifest_id: String,
    pub materialized_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingEpisode {
    pub epoch_id: String,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    pub first_seen_fingerprint: String,
    pub latest_fingerprint: String,
    pub latest_manifest_id: String,
    pub pending_turn_count: u32,
    pub source: PendingSource,
    pub touched_sessions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub fingerprint: String,
    pub manifest_id: String,
    pub seen_at: i64,
    pub observed_head_oid: Option<String>,
}
```

Critical changes from v1:

* `pending` is a first-class epoch, not just a few counters
* `age_minutes` must measure `now - pending.first_seen_at`, not `now - last_checkpoint_at`
* `anchor` is always tied to a real commit oid
* `observed_head_*` is metadata, not the anchor identity

The field name `last_checkpoint_commit` in v1 is misleading when the workspace was dirty at initialization. `anchor.checkpoint_commit_oid` must always point to a real Sprocket-created commit.

---

## 4. Replace the snapshot model

Use a **present-only** manifest.

Do not store explicit `"deleted"` entries in the current snapshot.

Deletions are just:

```text
paths present in old manifest but absent in new manifest
```

That simplifies everything.

Manifest entry:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub path: String,     // normalized repo-relative slash path
    pub mode: u32,        // 100644, 100755, 120000
    pub digest: String,   // blake3:<hex> for observation
}
```

For correctness:

* include file mode
* include symlinks
* ignore directories
* treat submodules as opaque gitlinks or exclude them for v2
* normalize paths once, centrally

Do **not** hardcode `src/tests` as defaults. For universal drop-in behavior, default to repo root plus strong excludes. Narrowing the surface is a policy optimization, not a correctness dependency.

### Complex logic: observation snapshot capture

```rust
use blake3::Hasher;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use std::{fs, os::unix::fs::PermissionsExt};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub path: String,
    pub mode: u32,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub fingerprint: String,
    pub manifest_id: String,
    pub entries: Vec<ManifestEntry>,
}

pub fn capture_observation_snapshot(
    repo_root: &Utf8Path,
    git: &dyn GitBackend,
    owned: &OwnedPolicy,
) -> anyhow::Result<Snapshot> {
    let mut entries = Vec::new();

    // Machine-readable, present-only path listing.
    let paths = git.list_present_paths(owned)?;

    for rel in paths {
        let abs = repo_root.join(&rel);
        let meta = fs::symlink_metadata(&abs)?;

        if meta.file_type().is_dir() {
            continue;
        }

        let (mode, bytes): (u32, Vec<u8>) = if meta.file_type().is_symlink() {
            let target = fs::read_link(&abs)?;
            (0o120000, target.as_os_str().as_encoded_bytes().to_vec())
        } else if meta.is_file() {
            let exec = (meta.permissions().mode() & 0o111) != 0;
            let mode = if exec { 0o100755 } else { 0o100644 };
            (mode, fs::read(&abs)?)
        } else {
            continue;
        };

        let digest = format!("blake3:{}", blake3::hash(&bytes).to_hex());
        entries.push(ManifestEntry {
            path: rel,
            mode,
            digest,
        });
    }

    entries.sort_by(|a, b| a.path.cmp(&b.path));

    let mut hasher = Hasher::new();
    for e in &entries {
        hasher.update(e.path.as_bytes());
        hasher.update(&[0]);
        hasher.update(format!("{:o}", e.mode).as_bytes());
        hasher.update(&[0]);
        hasher.update(e.digest.as_bytes());
        hasher.update(&[0xff]);
    }

    let fp = format!("blake3:{}", hasher.finalize().to_hex());

    Ok(Snapshot {
        fingerprint: fp.clone(),
        manifest_id: fp,
        entries,
    })
}
```

This is the observation fingerprint used by baseline/decision logic. It is deterministic, mode-aware, and present-only.

---

## 5. Add SessionStart adoption

Install a `SessionStart` hook in addition to `UserPromptSubmit`, `PreToolUse`, and `Stop`.

Purpose:

* observe inherited dirty state as soon as a session starts or resumes
* create/open a pending epoch even before the first user prompt
* optionally inject developer context telling Codex that uncheckpointed work already exists

Codex supports `SessionStart`, passes `session_id`, and allows additional developer context on stdout JSON for that event. ([OpenAI Developers][1])

### Initialization rule

On first-ever install:

* if no manager exists, initialize `anchor = current snapshot`
* do **not** auto-adopt the entire repo as dirty by default

That keeps cold start sane.

After that:

* if `current != anchor`, open or refresh `pending`
* inherited dirty must be visible even if the next turn makes no changes

This is the core fix for v1 starvation.

---

## 6. Turn state must remember anchor-at-start

Your turn state needs more than the baseline snapshot.

Use:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnState {
    pub version: u32,
    pub session_id: String,
    pub turn_id: String,
    pub started_at: i64,
    pub baseline_fingerprint: String,
    pub baseline_manifest_id: String,
    pub anchor_fingerprint_at_start: String,
    pub anchor_manifest_id_at_start: String,
}
```

Why `anchor_*_at_start` matters:

* `baseline == current` no longer implies noop
* if `baseline == current` but `baseline != anchor_at_start`, the turn observed inherited dirty work and must contribute to adoption/materialization

That is the exact place v1 is logically incomplete.

---

## 7. Rewrite Stop as an authoritative transaction

This is the critical path.

Non-negotiable rules:

1. acquire lock first
2. load manager and turn
3. capture current snapshot **inside** the lock
4. reconcile pending state
5. classify
6. materialize hidden checkpoint if required
7. recapture if needed
8. persist manager
9. delete turn
10. release lock

### Complex logic: pure classification

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PendingSource {
    TurnLocal,
    Inherited,
    Mixed,
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoopReason {
    MatchesAnchor,
    MissingTurn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Noop(NoopReason),
    RecordPending {
        source: PendingSource,
        changed_paths: u32,
    },
    Materialize {
        source: PendingSource,
        changed_paths: u32,
    },
}

pub struct ClassifyInput {
    pub now_unix: i64,
    pub anchor_fingerprint: String,
    pub turn_baseline_fingerprint: String,
    pub anchor_fingerprint_at_start: String,
    pub current_fingerprint: String,
    pub global_changed_paths: u32,
    pub pending_turn_count: u32,
    pub pending_first_seen_at: Option<i64>,
    pub turn_threshold: u32,
    pub file_threshold: u32,
    pub age_seconds: i64,
}

pub fn classify(input: &ClassifyInput) -> Decision {
    if input.current_fingerprint == input.anchor_fingerprint {
        return Decision::Noop(NoopReason::MatchesAnchor);
    }

    let changed_this_turn =
        input.current_fingerprint != input.turn_baseline_fingerprint;

    let dirty_existed_at_turn_start =
        input.turn_baseline_fingerprint != input.anchor_fingerprint_at_start;

    let source = match (dirty_existed_at_turn_start, changed_this_turn) {
        (false, true) => PendingSource::TurnLocal,
        (true, false) => PendingSource::Inherited,
        (true, true) => PendingSource::Mixed,
        (false, false) => PendingSource::External,
    };

    let turns = input.pending_turn_count.saturating_add(1);

    let dirty_age = input.pending_first_seen_at
        .map(|t| input.now_unix.saturating_sub(t))
        .unwrap_or(0);

    let should_materialize =
        turns >= input.turn_threshold
            || input.global_changed_paths >= input.file_threshold
            || dirty_age >= input.age_seconds;

    if should_materialize {
        Decision::Materialize {
            source,
            changed_paths: input.global_changed_paths,
        }
    } else {
        Decision::RecordPending {
            source,
            changed_paths: input.global_changed_paths,
        }
    }
}
```

Key property:

* inherited dirty (`baseline == current`, `baseline != anchor_at_start`) now contributes to checkpointing
* external drift also becomes visible instead of vanishing behind a noop

### Complex logic: stale-safe lock with RAII

```rust
use camino::{Utf8Path, Utf8PathBuf};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    time::{Duration, SystemTime},
};

pub struct RepoLock {
    path: Utf8PathBuf,
    _file: File,
}

impl RepoLock {
    pub fn try_acquire(
        path: &Utf8Path,
        stale_after: Duration,
    ) -> anyhow::Result<Option<Self>> {
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
        {
            Ok(mut file) => {
                writeln!(
                    file,
                    "pid={} acquired_at={}",
                    std::process::id(),
                    chrono::Utc::now().timestamp()
                )?;
                file.sync_all()?;
                return Ok(Some(Self {
                    path: path.to_owned(),
                    _file: file,
                }));
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err.into()),
        }

        let meta = fs::metadata(path)?;
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let age = SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default();

        if age < stale_after {
            return Ok(None);
        }

        // Best-effort stale lock reap.
        let _ = fs::remove_file(path);

        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
        {
            Ok(mut file) => {
                writeln!(
                    file,
                    "pid={} reacquired_at={}",
                    std::process::id(),
                    chrono::Utc::now().timestamp()
                )?;
                file.sync_all()?;
                Ok(Some(Self {
                    path: path.to_owned(),
                    _file: file,
                }))
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
            Err(err) => Err(err.into()),
        }
    }
}

impl Drop for RepoLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
```

That is enough for repo-local serialization. Do not block. Busy lock should no-op and rely on the next turn.

---

## 8. Materialize hidden checkpoints from an exact snapshot

This is the most important implementation choice.

There are two ways to build the hidden checkpoint tree:

### Fast path

Temporary index + `git add -A` on owned pathspecs.

### Strict path

Read file bytes once, write blobs with `git hash-object -w --stdin --path`, then populate a temp index with `git update-index`, then `write-tree`, `commit-tree`, `update-ref`.

For your stated goal, implement the **strict path** first.

Why:

* exact tree corresponds to the observed snapshot
* no dependency on the user’s main index
* honors Git clean filters via `hash-object --path`
* does not assume SHA-1-only object ids

Git’s `hash-object --path` applies path-based filters before writing the blob, `update-index` can insert exact mode/object/path entries, `write-tree` writes a tree from the current index, `commit-tree` creates a commit object from that tree, and `update-ref` safely moves the hidden ref. Git also supports SHA-256 object-format repositories, so Sprocket must not assume 40-character SHA-1 oids. ([Git][4])

### Complex logic: strict snapshot + hidden commit materialization

```rust
use camino::Utf8Path;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrictEntry {
    pub path: String,
    pub mode: u32,
    pub digest: String,   // observation digest
    pub git_oid: String,  // object-format-aware oid returned by git
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrictSnapshot {
    pub fingerprint: String,
    pub manifest_id: String,
    pub entries: Vec<StrictEntry>,
}

pub fn capture_strict_snapshot(
    repo_root: &Utf8Path,
    git: &dyn GitBackend,
    owned: &OwnedPolicy,
) -> anyhow::Result<StrictSnapshot> {
    let mut entries = Vec::new();
    let paths = git.list_present_paths(owned)?;

    for rel in paths {
        let abs = repo_root.join(&rel);
        let meta = std::fs::symlink_metadata(&abs)?;

        let (mode, bytes) = if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&abs)?;
            (0o120000, target.as_os_str().as_encoded_bytes().to_vec())
        } else if meta.is_file() {
            #[cfg(unix)]
            let exec = (std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o111) != 0;
            #[cfg(not(unix))]
            let exec = false;

            let mode = if exec { 0o100755 } else { 0o100644 };
            (mode, std::fs::read(&abs)?)
        } else {
            continue;
        };

        let digest = format!("blake3:{}", blake3::hash(&bytes).to_hex());
        let git_oid = git.hash_object_for_path(&rel, &bytes)?; // git hash-object -w --stdin --path <rel>

        entries.push(StrictEntry {
            path: rel,
            mode,
            digest,
            git_oid,
        });
    }

    entries.sort_by(|a, b| a.path.cmp(&b.path));

    let mut hasher = blake3::Hasher::new();
    for e in &entries {
        hasher.update(e.path.as_bytes());
        hasher.update(&[0]);
        hasher.update(format!("{:o}", e.mode).as_bytes());
        hasher.update(&[0]);
        hasher.update(e.digest.as_bytes());
        hasher.update(&[0xff]);
    }

    let fp = format!("blake3:{}", hasher.finalize().to_hex());

    Ok(StrictSnapshot {
        fingerprint: fp.clone(),
        manifest_id: fp,
        entries,
    })
}

pub fn materialize_hidden_checkpoint(
    git: &dyn GitBackend,
    head_oid: Option<&str>,
    hidden_ref: &str,
    previous_hidden_oid: Option<&str>,
    strict: &StrictSnapshot,
    owned: &OwnedPolicy,
    message: &str,
) -> anyhow::Result<String> {
    let temp_index = git.create_temp_index()?;

    // Seed temp index from visible HEAD so unowned paths remain exactly as-is.
    if let Some(head) = head_oid {
        git.read_tree_into_index(temp_index.path(), head)?;
    }

    // Remove any owned HEAD paths that are absent from the strict snapshot.
    let head_owned_paths: BTreeSet<String> = if let Some(head) = head_oid {
        git.list_tree_paths(head, owned)?
    } else {
        BTreeSet::new()
    };

    let strict_paths: BTreeSet<String> =
        strict.entries.iter().map(|e| e.path.clone()).collect();

    let deleted: Vec<String> = head_owned_paths
        .difference(&strict_paths)
        .cloned()
        .collect();

    if !deleted.is_empty() {
        git.force_remove_from_index(temp_index.path(), &deleted)?;
    }

    // Add/replace exact snapshot entries without touching the real index.
    git.index_info(temp_index.path(), &strict.entries)?;

    let tree_oid = git.write_tree_from_index(temp_index.path())?;

    // Hidden lineage: first checkpoint can parent HEAD; later checkpoints parent previous hidden commit.
    let parent = previous_hidden_oid.or(head_oid);

    let commit_oid = git.commit_tree(
        &tree_oid,
        parent.into_iter().collect(),
        message,
    )?;

    git.update_ref_cas(hidden_ref, &commit_oid, previous_hidden_oid)?;

    Ok(commit_oid)
}
```

That is the checkpoint core.

### Commit message format

Use stable human subject + machine footers.

Example:

```text
checkpoint(core): save current work [auto]

Sprocket-Fingerprint: blake3:...
Sprocket-Source: inherited
Sprocket-Observed-Head: a1b2c3d4
Sprocket-Worktree: 7b1d...
Sprocket-Generation: 42
```

The footers matter more than a clever subject line.

---

## 9. Full Stop transaction

This is the orchestration path that must be correct.

### Complex logic: authoritative checkpoint transaction

```rust
pub fn run_checkpoint_turn(
    ctx: &CheckpointContext,
    git: &dyn GitBackend,
    stores: &Stores,
    clock: &dyn Clock,
) -> anyhow::Result<CheckpointOutcome> {
    let lock = match RepoLock::try_acquire(
        &stores.lock_path,
        std::time::Duration::from_secs(ctx.policy.lock_timeout_seconds as u64),
    )? {
        Some(lock) => lock,
        None => return Ok(CheckpointOutcome::Noop("lock-busy")),
    };

    let now = clock.now_unix();
    let mut manager = stores.manager.load()?;
    let turn = match stores.turns.load(&ctx.session_id, &ctx.turn_id)? {
        Some(t) => t,
        None => return Ok(CheckpointOutcome::Noop("missing-turn")),
    };

    let head = git.head_state()?;
    let strict = capture_strict_snapshot(&ctx.repo_root, git, &ctx.policy.owned)?;
    stores.manifests.put(&strict.manifest_id, &strict)?;

    if strict.fingerprint == manager.anchor.fingerprint {
        manager.pending = None;
        manager.last_seen = Some(Observation {
            fingerprint: strict.fingerprint.clone(),
            manifest_id: strict.manifest_id.clone(),
            seen_at: now,
            observed_head_oid: head.oid.clone(),
        });
        stores.manager.save(&manager)?;
        stores.turns.delete(&ctx.session_id, &ctx.turn_id)?;
        drop(lock);
        return Ok(CheckpointOutcome::Noop("matches-anchor"));
    }

    let anchor_manifest = stores
        .manifests
        .get::<StrictSnapshot>(&manager.anchor.manifest_id)?;

    let delta = diff_changed_paths(&anchor_manifest.entries, &strict.entries);

    let pending_turn_count = manager
        .pending
        .as_ref()
        .map(|p| p.pending_turn_count)
        .unwrap_or(0);

    let pending_first_seen_at = manager
        .pending
        .as_ref()
        .map(|p| p.first_seen_at)
        .or(Some(now));

    let decision = classify(&ClassifyInput {
        now_unix: now,
        anchor_fingerprint: manager.anchor.fingerprint.clone(),
        turn_baseline_fingerprint: turn.baseline_fingerprint.clone(),
        anchor_fingerprint_at_start: turn.anchor_fingerprint_at_start.clone(),
        current_fingerprint: strict.fingerprint.clone(),
        global_changed_paths: delta as u32,
        pending_turn_count,
        pending_first_seen_at,
        turn_threshold: ctx.policy.checkpoint.turn_threshold,
        file_threshold: ctx.policy.checkpoint.file_threshold,
        age_seconds: (ctx.policy.checkpoint.age_minutes as i64) * 60,
    });

    match decision {
        Decision::Noop(_) => {
            stores.turns.delete(&ctx.session_id, &ctx.turn_id)?;
            Ok(CheckpointOutcome::Noop("classified-noop"))
        }

        Decision::RecordPending { source, .. } => {
            manager.pending = Some(update_pending(
                manager.pending.take(),
                now,
                &ctx.session_id,
                source,
                &strict,
            ));
            manager.last_seen = Some(Observation {
                fingerprint: strict.fingerprint.clone(),
                manifest_id: strict.manifest_id.clone(),
                seen_at: now,
                observed_head_oid: head.oid.clone(),
            });
            stores.manager.save(&manager)?;
            stores.turns.delete(&ctx.session_id, &ctx.turn_id)?;
            Ok(CheckpointOutcome::Pending)
        }

        Decision::Materialize { source, .. } => {
            let message = build_checkpoint_message(&ctx.policy, source, &strict, &head, manager.generation + 1);

            let commit_oid = materialize_hidden_checkpoint(
                git,
                head.oid.as_deref(),
                &manager.anchor.hidden_ref,
                Some(&manager.anchor.checkpoint_commit_oid),
                &strict,
                &ctx.policy.owned,
                &message,
            )?;

            manager.generation += 1;
            manager.anchor = AnchorState {
                hidden_ref: manager.anchor.hidden_ref.clone(),
                checkpoint_commit_oid: commit_oid,
                observed_head_oid: head.oid.clone(),
                observed_head_ref: head.symref.clone(),
                fingerprint: strict.fingerprint.clone(),
                manifest_id: strict.manifest_id.clone(),
                materialized_at: now,
            };
            manager.pending = None;
            manager.last_seen = Some(Observation {
                fingerprint: strict.fingerprint.clone(),
                manifest_id: strict.manifest_id.clone(),
                seen_at: now,
                observed_head_oid: head.oid.clone(),
            });

            stores.manager.save(&manager)?;
            stores.turns.delete(&ctx.session_id, &ctx.turn_id)?;

            Ok(CheckpointOutcome::Materialized {
                commit_oid: manager.anchor.checkpoint_commit_oid.clone(),
            })
        }
    }
}
```

That is the correct transaction boundary.

---

## 10. Visible promotion is a separate layer

Only add visible branch commits as a second layer.

Modes:

* `hidden_only` — recommended default for shared branch / multi-agent
* `hidden_then_promote` — recommended if you want cleaner user-visible history later
* `visible_direct` — only for dedicated agent branches where Sprocket owns the main index

### Why promotion must be separate

A hidden checkpoint can be exact without touching the main index. A visible commit cannot. If you want automatic visible commits, you must either:

* own the main index, or
* accept that promotion may skip when the user index contains foreign staging state

My recommendation:

* implement `hidden_only` first
* add `hidden_then_promote`
* leave `visible_direct` behind a strict policy flag

### Promotion rules

Promote only if all are true:

1. validators passed
2. no staged changes outside owned surface
3. current HEAD still matches the observed promotion precondition
4. repo not in merge/rebase/cherry-pick state
5. policy explicitly allows index-owned visible commits

If any precondition fails:

* keep the hidden checkpoint
* do not fail the hook
* record a diagnostic journal event

### Validator loop

Use hidden checkpointing for rescue. Use `Stop` continuation only for promotion-quality gating.

Codex `Stop` hooks can continue the model by returning a block reason, which makes them suitable for “tests failed, keep going” behavior. ([OpenAI Developers][1])

Example response:

```json
{
  "decision": "block",
  "reason": "Sprocket validator failed: cargo test -q. Fix the failing tests before stopping."
}
```

That gives you “safe over time” without making autosave contingent on green tests.

---

## 11. Enforcement layer: Codex guardrail + Git hook guard

Do both.

### Codex-side

Keep `PreToolUse` to block obvious mutating Git commands.

But treat it as advisory only. `PreToolUse` only sees Bash, can be worked around via scripts, and some parsed outputs fail open. ([OpenAI Developers][1])

### Git-side

Install a `prepare-commit-msg` hook that aborts commits unless Sprocket sets an allow token.

Why `prepare-commit-msg`:

* it aborts the commit on non-zero exit
* it is **not** bypassed by `--no-verify`

`pre-commit` and `commit-msg` are useful for diagnostics, but they can be bypassed with `--no-verify`. ([Git][5])

Practical rule:

* `prepare-commit-msg`: hard cooperative guard
* `pre-commit`: optional diagnostics
* `commit-msg`: optional normalization
* do not claim security against a malicious human using plumbing commands

Also: if `core.hooksPath` is configured, install into the effective hooks directory, not blindly into `.git/hooks`. Git allows hooks to live in the default hooks dir or a custom path via `core.hooksPath`. ([Git][5])

---

## 12. Journal every decision

You will need this for debugging.

Write NDJSON events under runtime state.

Example event:

```json
{"ts":1711900000,"event":"stop","session_id":"s1","turn_id":"t9","anchor_fp":"blake3:...","current_fp":"blake3:...","decision":"materialize","source":"inherited","hidden_ref":"refs/sprocket/checkpoints/v2/...","commit_oid":"abc123"}
```

Why:

* reconstruct race conditions
* prove adoption logic worked
* debug “why didn’t it commit?”
* debug “why did it commit now?”

Do not try to infer this from manager state alone.

---

## 13. Best-practice implementation advice

These are the engineering choices I would make without hesitation.

### Use the real Git binary behind a typed adapter

Do not scatter `Command::new("git")` across the codebase.

One `GitBackend` trait. One implementation. Machine-readable outputs only.

Reason:

* worktrees
* filters
* sparse checkout edge cases
* object-format compatibility
* fewer semantic mismatches

You can swap to `gix` later if you want more speed. The interface should make that possible.

### Keep pure logic pure

`classify`, `diff`, `reconcile_pending`, `build_message`, `sanitize_stream_key` should have zero I/O.

That gives you property tests.

### Use content-addressed manifest storage

`manifests/<fingerprint>.json.zst`

Manager and turn state store manifest ids, not full manifests.

### Atomic state writes

Every state write:

1. write temp file
2. fsync file
3. rename
4. fsync parent dir if you want crash-hardening

### Never parse porcelain for semantics unless it is documented and machine-oriented

Use:

* `-z`
* `--name-only`
* `--stdin`
* `--index-info`

Avoid free-form text.

### Distinguish soft-fail from invariant-fail

Soft-fail examples:

* lock busy
* missing turn
* promotion skipped
* validator failed

Invariant-fail examples:

* manager anchor manifest missing
* hidden ref CAS failure under lock
* corrupt JSON state
* impossible path normalization collision

Journal both differently.

---

## 14. Test matrix you need before trusting this

At minimum:

1. clean repo, no changes
2. single turn edits owned file
3. single turn edits only unowned file
4. inherited dirty at session start, no turn-local edits
5. inherited dirty + new turn-local edits
6. two overlapping sessions, same worktree, same branch
7. lock busy during stop
8. visible HEAD moved between baseline and stop
9. branch rebased while pending exists
10. detached HEAD
11. linked worktree
12. symlink add/modify/delete
13. executable bit change
14. ignored generated file created under owned root
15. user has staged unrelated files in main index
16. merge conflict in workspace with hidden-only mode
17. promotion preconditions fail but hidden checkpoint succeeds
18. stale turn file cleanup
19. first install on dirty repo
20. SHA-256 repo object format

Use:

* integration tests with temporary repos
* property tests for `diff` and `classify`
* fault injection around lock/state writes/ref updates

---

## The exact v2 path I would build

In order:

1. move runtime state under `git rev-parse --git-path sprocket`
2. add `SessionStart`
3. replace manager with `anchor + pending`
4. replace turn state with `anchor_at_start`
5. switch snapshot model to present-only manifest
6. implement pure `classify`
7. recompute snapshot inside lock
8. implement strict hidden materializer
9. journal every decision
10. install `prepare-commit-msg` guard
11. add `hidden_then_promote`
12. only then consider `visible_direct`

## Final judgment

The best version of Sprocket is **not** “smart auto-commit directly on the working branch.”

The best version is:

* hidden checkpoint engine
* exact owned-surface snapshots
* anchor-relative pending adoption
* worktree-aware runtime state
* strict Git-plumbing materialization
* optional visible promotion

That is the shape that can actually converge.

The next useful deliverable is turning this spec into a concrete Rust crate skeleton with the `GitBackend` trait, state structs, and the first integration tests.

[1]: https://developers.openai.com/codex/hooks/ "https://developers.openai.com/codex/hooks/"
[2]: https://git-scm.com/docs/git-worktree "https://git-scm.com/docs/git-worktree"
[3]: https://git-scm.com/book/en/v2/Git-Internals-Git-References "https://git-scm.com/book/en/v2/Git-Internals-Git-References"
[4]: https://git-scm.com/docs/git-hash-object "https://git-scm.com/docs/git-hash-object"
[5]: https://git-scm.com/docs/githooks "https://git-scm.com/docs/githooks"

---

Good. I’m going to harden the design in two places before turning it into a concrete implementation spec, because both matter for correctness.

First, **manager state must be stream-scoped**, not just repo-scoped. A “stream” is the current logical branch context for a given worktree. If the user switches from `main` to `feature/x`, the anchor and pending episode must not bleed across that boundary.

Second, the first anchor should be a **hidden bootstrap checkpoint commit**, not just metadata pointing at the current dirty workspace. That removes the v1 mismatch where the anchor fingerprint could describe a state that no commit actually represented.

Below is the v2 implementation blueprint I would build.

---

# 1. Non-negotiable invariants

These are the invariants the code should enforce. Everything else is secondary.

1. Every stream has exactly one current **anchor**.
2. Every anchor has:

   * a hidden ref,
   * a real checkpoint commit oid,
   * a manifest id,
   * a workspace fingerprint.
3. Every `Stop` decision is based on a snapshot captured **inside the repo lock**.
4. Hidden checkpoint materialization must never mutate the user’s main index.
5. Turn-local inactivity must **not** starve inherited dirty state.
6. Switching branches or entering detached HEAD must not pollute another stream’s anchor.
7. Visible promotion is optional and must be a separate layer from hidden checkpoint materialization.
8. A lock miss is a soft no-op, not a retry loop.
9. All important state writes are atomic.
10. The system never assumes UTF-8 repo paths or SHA-1 object ids.

The last point is easy to miss and extremely important. If you truly want “drop into any codebase,” you cannot assume UTF-8 filenames or 40-char oids.

---

# 2. Opinionated architecture

## Crate layout

```text
src/
  lib.rs
  main.rs
  cli.rs

  app/
    install_codex.rs
    session_start.rs
    baseline.rs
    checkpoint.rs
    pre_tool_use.rs

  domain/
    ids.rs
    repopath.rs
    policy.rs
    manifest.rs
    snapshot.rs
    delta.rs
    decision.rs
    manager.rs
    turn.rs
    session.rs
    journal.rs
    errors.rs

  engine/
    init_stream.rs
    observe.rs
    reconcile_pending.rs
    classify.rs
    materialize_hidden.rs
    promote_visible.rs
    repair.rs

  infra/
    git.rs
    git_cli.rs
    refs.rs
    temp_index.rs
    lock.rs
    store.rs
    atomic_write.rs
    manifest_store.rs
    journal_store.rs
    clock.rs

  codex/
    payload.rs
    responses.rs
    hooks_json.rs
```

## Dependency choices

I would use these and keep the set tight:

```toml
[dependencies]
anyhow = "1"
thiserror = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
bstr = { version = "1", features = ["serde"] }
blake3 = "1"
base64 = "0.22"
tempfile = "3"
time = { version = "0.3", features = ["formatting", "parsing"] }
uuid = { version = "1", features = ["v4", "serde"] }
zstd = "0.13"
```

I would **not** make this async. This is a single-process CLI tool driving Git and local filesystem operations. Sync code is easier to reason about, easier to test, and less failure-prone.

## Core style rules

* Git interaction only through `infra::git::*`
* state transition logic only in `domain::*` and `engine::*`
* no module outside `infra` shells out
* no JSON `Value` wandering through the core domain
* no stringly-typed branch, stream, path, or oid identifiers in domain logic

---

# 3. Path model: do not assume UTF-8

This is one place where a lot of Rust code gets subtly wrong.

Use:

* `PathBuf` for absolute filesystem paths
* `RepoPath` for repo-relative Git paths as raw bytes

A good internal type is:

```rust
use bstr::{BStr, BString};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RepoPath(pub BString);

impl RepoPath {
    pub fn as_bstr(&self) -> &BStr {
        self.0.as_ref()
    }

    pub fn display_lossy(&self) -> String {
        String::from_utf8_lossy(self.0.as_ref()).into_owned()
    }
}
```

Use raw bytes because:

* Git paths are byte sequences on Unix
* `-z` output from Git is byte-oriented
* “any codebase” includes weird paths

For JSON state, `bstr` with `serde` is fine, or use a custom base64 serializer if you want more explicit storage.

---

# 4. Stream identity and runtime layout

A stream is the current worktree + logical HEAD context.

Use this identity model:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeadState {
    pub oid: Option<String>,
    pub symref: Option<String>,   // e.g. refs/heads/main
    pub detached: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamIdentity {
    pub worktree_id: String,
    pub stream_id: String,
    pub hidden_ref: String,
    pub display_name: String,
}
```

### Why `stream_id` must be hashed

Do not try to use raw symrefs directly as filenames or arbitrary ref components. Use a stable hash.

```rust
pub fn compute_stream_identity(worktree_root: &std::path::Path, head: &HeadState) -> StreamIdentity {
    let worktree_id = blake3::hash(worktree_root.to_string_lossy().as_bytes())
        .to_hex()
        .to_string();

    let stream_source = match (&head.symref, &head.oid) {
        (Some(symref), _) => format!("symref:{symref}"),
        (None, Some(oid)) => format!("detached:{oid}"),
        (None, None) => "unborn".to_string(),
    };

    let stream_id = blake3::hash(stream_source.as_bytes()).to_hex().to_string();
    let hidden_ref = format!("refs/sprocket/checkpoints/v2/{worktree_id}/{stream_id}");

    let display_name = head.symref.clone()
        .or_else(|| head.oid.clone().map(|o| format!("detached:{o}")))
        .unwrap_or_else(|| "unborn".to_string());

    StreamIdentity {
        worktree_id,
        stream_id,
        hidden_ref,
        display_name,
    }
}
```

## Runtime storage layout

Do **not** keep machine-local runtime under `.sprocket/state` in the working tree.

Resolve runtime root via:

```bash
git rev-parse --git-path sprocket
```

Then store:

```text
<git-path sprocket>/
  local.toml
  streams/
    <stream-id>/
      manager.json
      manifests/
        <manifest-id>.json.zst
      turns/
        <session-id>/
          <turn-id>.json
      sessions/
        <session-id>.json
      journal/
        events.ndjson
  checkpoint.lock
```

This gives you:

* per-worktree runtime state
* no `.gitignore` pollution
* clean separation between shared policy and local runtime

---

# 5. Shared policy format

Use Git pathspecs, not custom glob semantics. Git already knows how to match the repo surface.

```toml
version = 2

[owned]
include = ["."]
exclude = [
  ":(exclude).git",
  ":(exclude).sprocket",
  ":(exclude)node_modules",
  ":(exclude)target",
  ":(exclude)dist",
  ":(exclude)build",
  ":(exclude).next",
  ":(exclude)coverage",
  ":(exclude).venv"
]

[checkpoint]
mode = "hidden_only"         # hidden_only | hidden_then_promote | visible_direct
turn_threshold = 2
file_threshold = 4
age_minutes = 20
default_area = "core"
message_template = "checkpoint({area}): save current work [auto]"
lock_timeout_seconds = 300

[promotion]
enabled = false
validators = []
continue_on_failure = true

[guard]
codex_pretool = true
git_prepare_commit_msg = true

[compat]
allow_sparse_checkout = false
```

### Why Git pathspecs

This avoids writing your own buggy matcher and lets Git handle include/exclude semantics.

---

# 6. Git adapter surface

This is the highest-value interface in the whole codebase.

```rust
use std::path::{Path, PathBuf};

pub trait GitBackend {
    fn repo_root(&self) -> anyhow::Result<PathBuf>;
    fn git_path(&self, name: &str) -> anyhow::Result<PathBuf>;
    fn head_state(&self) -> anyhow::Result<HeadState>;
    fn repo_state(&self) -> anyhow::Result<RepoState>;

    fn list_present_paths(&self, pathspecs: &[String]) -> anyhow::Result<Vec<RepoPath>>;
    fn list_head_owned_paths(&self, head_oid: &str, pathspecs: &[String]) -> anyhow::Result<Vec<RepoPath>>;

    fn hash_object_for_path(&self, path: &RepoPath, bytes: &[u8]) -> anyhow::Result<String>;

    fn create_temp_index(&self) -> anyhow::Result<TempIndex>;
    fn read_tree_into_index(&self, index_path: &Path, treeish: &str) -> anyhow::Result<()>;
    fn update_index_cacheinfo(
        &self,
        index_path: &Path,
        mode: u32,
        oid: &str,
        path: &RepoPath,
    ) -> anyhow::Result<()>;
    fn force_remove_from_index(
        &self,
        index_path: &Path,
        paths: &[RepoPath],
    ) -> anyhow::Result<()>;
    fn write_tree_from_index(&self, index_path: &Path) -> anyhow::Result<String>;

    fn commit_tree(
        &self,
        tree_oid: &str,
        parents: &[String],
        message: &str,
    ) -> anyhow::Result<String>;

    fn update_ref_cas(
        &self,
        refname: &str,
        new_oid: &str,
        old_oid: Option<&str>,
    ) -> anyhow::Result<()>;

    fn rev_parse_ref(&self, refname: &str) -> anyhow::Result<Option<String>>;
    fn show_file_at_commit(&self, commit_oid: &str, path: &RepoPath) -> anyhow::Result<Vec<u8>>;

    fn install_hook_file(&self, hook_name: &str, content: &[u8]) -> anyhow::Result<()>;
}
```

## Why shell out to Git first

For v2, I would use the real Git binary, not `git2`/libgit2. You care about:

* worktree semantics
* pathspec behavior
* clean filters
* object format compatibility
* exact alignment with the user’s Git

That is easier to get correct with the Git CLI.

---

# 7. Git CLI implementation details

## Machine-readable execution helper

```rust
use anyhow::{anyhow, Context};
use std::ffi::{OsStr, OsString};
use std::process::{Command, Output, Stdio};

pub struct GitCli {
    repo_root: std::path::PathBuf,
}

impl GitCli {
    fn run<I, S>(&self, args: I) -> anyhow::Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.repo_root)
            .stdin(Stdio::null())
            .output()
            .context("failed to spawn git")?;

        if !output.status.success() {
            return Err(anyhow!(
                "git failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(output)
    }

    fn run_with_env<I, S>(
        &self,
        args: I,
        envs: &[(&str, &OsStr)],
    ) -> anyhow::Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(&self.repo_root);

        for (k, v) in envs {
            cmd.env(k, v);
        }

        let output = cmd.output().context("failed to spawn git")?;

        if !output.status.success() {
            return Err(anyhow!(
                "git failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(output)
    }
}
```

## Present path listing

This must list **present** tracked and untracked files, respecting ignore rules.

```rust
use bstr::{BString, ByteSlice};
use std::collections::BTreeSet;
use std::os::unix::ffi::OsStrExt;

impl GitBackend for GitCli {
    fn list_present_paths(&self, pathspecs: &[String]) -> anyhow::Result<Vec<RepoPath>> {
        let mut args: Vec<OsString> = vec![
            "ls-files".into(),
            "-z".into(),
            "--cached".into(),
            "--others".into(),
            "--exclude-standard".into(),
            "--".into(),
        ];
        args.extend(pathspecs.iter().map(OsString::from));

        let out = self.run(args)?;
        let mut seen = BTreeSet::<BString>::new();

        for raw in out.stdout.split_str("\0").filter(|s| !s.is_empty()) {
            let path_bytes = raw.as_bytes().to_vec();
            let abs = self.repo_root.join(std::ffi::OsStr::from_bytes(&path_bytes));

            // This existence filter is required because tracked deleted files may still
            // appear via index-oriented listing.
            if std::fs::symlink_metadata(&abs).is_ok() {
                seen.insert(BString::from(path_bytes));
            }
        }

        Ok(seen.into_iter().map(RepoPath).collect())
    }

    fn list_head_owned_paths(&self, head_oid: &str, pathspecs: &[String]) -> anyhow::Result<Vec<RepoPath>> {
        let mut args: Vec<OsString> = vec![
            "ls-tree".into(),
            "-r".into(),
            "-z".into(),
            "--name-only".into(),
            head_oid.into(),
            "--".into(),
        ];
        args.extend(pathspecs.iter().map(OsString::from));

        let out = self.run(args)?;
        let mut paths = Vec::new();
        for raw in out.stdout.split_str("\0").filter(|s| !s.is_empty()) {
            paths.push(RepoPath(BString::from(raw.as_bytes().to_vec())));
        }
        Ok(paths)
    }

    // other methods omitted here
}
```

### Sparse checkout

This implementation should explicitly reject sparse checkout unless you add skip-worktree handling.

At `SessionStart`, detect sparse mode. If policy says `allow_sparse_checkout = false`, log and no-op with a diagnostic. That is better than pretending to support it and corrupting deletion logic.

---

# 8. Manifest and snapshot model

Use a strict snapshot with both workspace digest and Git blob oid.

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrictEntry {
    pub path: RepoPath,
    pub mode: u32,          // 100644, 100755, 120000
    pub digest: String,     // blake3 of observed working-tree bytes / symlink target bytes
    pub git_oid: String,    // object oid after hash-object --path
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrictSnapshot {
    pub fingerprint: String,
    pub manifest_id: String,
    pub entries: Vec<StrictEntry>,
}
```

### Why both `digest` and `git_oid`

* `digest` fingerprints the workspace observation
* `git_oid` is what the hidden commit will actually reference

That distinction matters with filters and line-ending normalization.

## Snapshot capture

```rust
use blake3::Hasher;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;

pub fn capture_strict_snapshot(
    repo_root: &std::path::Path,
    git: &dyn GitBackend,
    pathspecs: &[String],
) -> anyhow::Result<StrictSnapshot> {
    let mut entries = Vec::new();

    for path in git.list_present_paths(pathspecs)? {
        let abs = repo_root.join(std::ffi::OsStr::from_bytes(path.as_bstr().as_bytes()));
        let meta = std::fs::symlink_metadata(&abs)?;

        let (mode, bytes) = if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&abs)?;
            (0o120000, target.as_os_str().as_bytes().to_vec())
        } else if meta.is_file() {
            let exec = (meta.permissions().mode() & 0o111) != 0;
            let mode = if exec { 0o100755 } else { 0o100644 };
            (mode, std::fs::read(&abs)?)
        } else {
            continue;
        };

        let digest = format!("blake3:{}", blake3::hash(&bytes).to_hex());
        let git_oid = git.hash_object_for_path(&path, &bytes)?;

        entries.push(StrictEntry {
            path,
            mode,
            digest,
            git_oid,
        });
    }

    entries.sort_by(|a, b| a.path.cmp(&b.path));

    let mut fp = Hasher::new();
    for e in &entries {
        fp.update(e.path.as_bstr().as_bytes());
        fp.update(&[0]);
        fp.update(format!("{:o}", e.mode).as_bytes());
        fp.update(&[0]);
        fp.update(e.digest.as_bytes());
        fp.update(&[0xff]);
    }

    let fingerprint = format!("blake3:{}", fp.finalize().to_hex());

    Ok(StrictSnapshot {
        fingerprint: fingerprint.clone(),
        manifest_id: fingerprint,
        entries,
    })
}
```

---

# 9. Manager, turn, and session state

## Manager

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerState {
    pub version: u32,
    pub stream: StreamIdentity,
    pub generation: u64,
    pub anchor: AnchorState,
    pub pending: Option<PendingEpisode>,
    pub last_seen: Option<Observation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorState {
    pub checkpoint_commit_oid: String,
    pub manifest_id: String,
    pub fingerprint: String,
    pub observed_head_oid: Option<String>,
    pub observed_head_ref: Option<String>,
    pub materialized_at: i64,
}
```

## Pending episode

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PendingSource {
    TurnLocal,
    Inherited,
    Mixed,
    External,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingEpisode {
    pub epoch_id: String,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    pub first_seen_fingerprint: String,
    pub latest_fingerprint: String,
    pub latest_manifest_id: String,
    pub pending_turn_count: u32,
    pub source: PendingSource,
    pub touched_sessions: Vec<String>,
}
```

## Turn state

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnState {
    pub version: u32,
    pub session_id: String,
    pub turn_id: String,
    pub stream_id_at_start: String,
    pub started_at: i64,
    pub baseline_fingerprint: String,
    pub baseline_manifest_id: String,
    pub anchor_fingerprint_at_start: String,
    pub anchor_manifest_id_at_start: String,
}
```

## Session state

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub version: u32,
    pub session_id: String,
    pub stream_id: String,
    pub started_at: i64,
    pub last_seen_at: i64,
}
```

---

# 10. Atomic state store

This is important enough to standardize.

```rust
use serde::{Serialize, de::DeserializeOwned};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    let parent = path.parent().expect("state file must have parent");
    fs::create_dir_all(parent)?;

    let tmp = path.with_extension("tmp");
    let mut f = File::create(&tmp)?;
    let bytes = serde_json::to_vec_pretty(value)?;
    f.write_all(&bytes)?;
    f.sync_all()?;
    drop(f);

    fs::rename(&tmp, path)?;

    if let Ok(dir) = File::open(parent) {
        let _ = dir.sync_all();
    }

    Ok(())
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> anyhow::Result<T> {
    let bytes = std::fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}
```

For manifests, use `json.zst` rather than pretty JSON.

---

# 11. Hook install and safe merge

Your v1 ownership rule is correct. Preserve unrelated hooks and remove only groups whose nested commands contain the stable marker.

## Complex merge code

```rust
use serde_json::{json, Value};

const SPROCKET_HOOK_MARKER: &str = "--sprocket-managed";

pub fn merge_codex_hooks_json(
    existing: Option<Value>,
    generated_groups: &[(String, Value)],
) -> anyhow::Result<Value> {
    let mut root = existing.unwrap_or_else(|| json!({}));

    let obj = root.as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("hooks.json root must be an object"))?;

    for (event_name, generated_group) in generated_groups {
        let arr = obj.entry(event_name.clone())
            .or_insert_with(|| Value::Array(Vec::new()));

        let existing_groups = arr.as_array_mut()
            .ok_or_else(|| anyhow::anyhow!("hooks event entry must be an array"))?;

        existing_groups.retain(|group| !group_contains_sprocket_marker(group));
        existing_groups.push(generated_group.clone());
    }

    Ok(root)
}

fn group_contains_sprocket_marker(group: &Value) -> bool {
    let Some(hooks) = group.get("hooks").and_then(|v| v.as_array()) else {
        return false;
    };

    hooks.iter().any(|hook| {
        hook.get("command")
            .and_then(|v| v.as_str())
            .map(|s| s.contains(SPROCKET_HOOK_MARKER))
            .unwrap_or(false)
    })
}
```

### Generated events

Install these:

* `SessionStart`
* `UserPromptSubmit`
* `PreToolUse` matcher `Bash`
* `Stop`

That is the v2 baseline.

---

# 12. SessionStart flow

This is where inherited dirty state gets adopted.

## Behavior

1. resolve repo root
2. resolve current `HeadState`
3. compute `StreamIdentity`
4. acquire lock
5. load manager for stream
6. if missing:

   * capture strict snapshot
   * materialize bootstrap hidden checkpoint
   * persist manager
7. upsert session state
8. if current snapshot != anchor:

   * open or refresh pending
9. sweep stale turn files for dead sessions
10. release lock

## Why bootstrap hidden checkpoint

This gives you a real commit object for the current workspace state without polluting visible history.

## Bootstrap code

```rust
pub fn ensure_stream_initialized(
    repo_root: &std::path::Path,
    git: &dyn GitBackend,
    stores: &Stores,
    stream: &StreamIdentity,
    head: &HeadState,
    now: i64,
    pathspecs: &[String],
    policy: &Policy,
) -> anyhow::Result<ManagerState> {
    if let Some(existing) = stores.manager.load_stream(&stream.stream_id)? {
        return Ok(existing);
    }

    let snap = capture_strict_snapshot(repo_root, git, pathspecs)?;
    stores.manifests.put(&stream.stream_id, &snap.manifest_id, &snap)?;

    let message = format!(
        "checkpoint({}): bootstrap anchor [auto]\n\nSprocket-Bootstrap: true\nSprocket-Fingerprint: {}\n",
        policy.checkpoint.default_area,
        snap.fingerprint
    );

    let commit_oid = materialize_hidden_checkpoint(
        git,
        head.oid.as_deref(),
        &stream.hidden_ref,
        None,
        &snap,
        pathspecs,
        &message,
    )?;

    let manager = ManagerState {
        version: 2,
        stream: stream.clone(),
        generation: 1,
        anchor: AnchorState {
            checkpoint_commit_oid: commit_oid,
            manifest_id: snap.manifest_id.clone(),
            fingerprint: snap.fingerprint.clone(),
            observed_head_oid: head.oid.clone(),
            observed_head_ref: head.symref.clone(),
            materialized_at: now,
        },
        pending: None,
        last_seen: Some(Observation {
            fingerprint: snap.fingerprint.clone(),
            manifest_id: snap.manifest_id.clone(),
            seen_at: now,
            observed_head_oid: head.oid.clone(),
        }),
    };

    stores.manager.save_stream(&stream.stream_id, &manager)?;
    Ok(manager)
}
```

---

# 13. UserPromptSubmit baseline flow

This should stay lean.

## Behavior

1. load session payload
2. resolve stream
3. acquire lock
4. ensure stream initialized
5. capture strict snapshot
6. save turn state using current snapshot as baseline and current anchor as `anchor_at_start`
7. refresh session heartbeat
8. release lock

The turn baseline must be captured under the same stream identity the turn began with.

---

# 14. Pending reconciliation

This is where a lot of systems get sloppy.

You need one function that updates or opens a pending episode deterministically.

```rust
pub fn reconcile_pending(
    existing: Option<PendingEpisode>,
    session_id: &str,
    source: PendingSource,
    now: i64,
    snap: &StrictSnapshot,
) -> PendingEpisode {
    match existing {
        None => PendingEpisode {
            epoch_id: uuid::Uuid::new_v4().to_string(),
            first_seen_at: now,
            last_seen_at: now,
            first_seen_fingerprint: snap.fingerprint.clone(),
            latest_fingerprint: snap.fingerprint.clone(),
            latest_manifest_id: snap.manifest_id.clone(),
            pending_turn_count: 1,
            source,
            touched_sessions: vec![session_id.to_string()],
        },
        Some(mut p) => {
            p.last_seen_at = now;
            p.latest_fingerprint = snap.fingerprint.clone();
            p.latest_manifest_id = snap.manifest_id.clone();
            p.pending_turn_count = p.pending_turn_count.saturating_add(1);
            p.source = merge_pending_source(p.source, source);
            if !p.touched_sessions.iter().any(|s| s == session_id) {
                p.touched_sessions.push(session_id.to_string());
            }
            p
        }
    }
}

fn merge_pending_source(a: PendingSource, b: PendingSource) -> PendingSource {
    use PendingSource::*;
    match (a, b) {
        (Mixed, _) | (_, Mixed) => Mixed,
        (Inherited, TurnLocal) | (TurnLocal, Inherited) => Mixed,
        (External, TurnLocal) | (TurnLocal, External) => Mixed,
        (External, Inherited) | (Inherited, External) => Mixed,
        (x, y) if x == y => x,
        _ => Mixed,
    }
}
```

---

# 15. Delta computation

Present-only manifests mean deletions are inferred by absence.

```rust
use std::collections::BTreeMap;

pub fn changed_path_count(old: &[StrictEntry], new: &[StrictEntry]) -> usize {
    let mut old_map = BTreeMap::new();
    let mut new_map = BTreeMap::new();

    for e in old {
        old_map.insert(e.path.clone(), (&e.mode, &e.digest));
    }
    for e in new {
        new_map.insert(e.path.clone(), (&e.mode, &e.digest));
    }

    let mut count = 0usize;

    for (path, old_sig) in &old_map {
        match new_map.get(path) {
            None => count += 1,
            Some(new_sig) if *new_sig != *old_sig => count += 1,
            _ => {}
        }
    }

    for path in new_map.keys() {
        if !old_map.contains_key(path) {
            count += 1;
        }
    }

    count
}
```

Rename detection is not required. Added + removed is enough for checkpointing.

---

# 16. Classification

This is the pure heart of the system.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoopReason {
    MatchesAnchor,
    MissingTurn,
    StreamChanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Noop(NoopReason),
    RecordPending {
        source: PendingSource,
        changed_paths: u32,
    },
    Materialize {
        source: PendingSource,
        changed_paths: u32,
    },
}

pub struct ClassifyInput<'a> {
    pub stream_id_now: &'a str,
    pub stream_id_at_start: &'a str,
    pub now_unix: i64,
    pub anchor_fingerprint: &'a str,
    pub turn_baseline_fingerprint: &'a str,
    pub anchor_fingerprint_at_start: &'a str,
    pub current_fingerprint: &'a str,
    pub global_changed_paths: u32,
    pub pending_turn_count: u32,
    pub pending_first_seen_at: Option<i64>,
    pub turn_threshold: u32,
    pub file_threshold: u32,
    pub age_seconds: i64,
}

pub fn classify(input: &ClassifyInput<'_>) -> Decision {
    if input.stream_id_now != input.stream_id_at_start {
        return Decision::Noop(NoopReason::StreamChanged);
    }

    if input.current_fingerprint == input.anchor_fingerprint {
        return Decision::Noop(NoopReason::MatchesAnchor);
    }

    let changed_this_turn =
        input.current_fingerprint != input.turn_baseline_fingerprint;

    let dirty_existed_at_turn_start =
        input.turn_baseline_fingerprint != input.anchor_fingerprint_at_start;

    let source = match (dirty_existed_at_turn_start, changed_this_turn) {
        (false, true) => PendingSource::TurnLocal,
        (true, false) => PendingSource::Inherited,
        (true, true) => PendingSource::Mixed,
        (false, false) => PendingSource::External,
    };

    let turns = input.pending_turn_count.saturating_add(1);
    let dirty_age = input.pending_first_seen_at
        .map(|t| input.now_unix.saturating_sub(t))
        .unwrap_or(0);

    let should_materialize =
        turns >= input.turn_threshold
            || input.global_changed_paths >= input.file_threshold
            || dirty_age >= input.age_seconds;

    if should_materialize {
        Decision::Materialize {
            source,
            changed_paths: input.global_changed_paths,
        }
    } else {
        Decision::RecordPending {
            source,
            changed_paths: input.global_changed_paths,
        }
    }
}
```

This solves the major v1 starvation bug.

---

# 17. Hidden checkpoint materialization

This is the most sensitive code in the system.

The safe structure is:

1. create temp index
2. seed temp index from visible HEAD tree
3. remove owned paths present in HEAD but absent from strict snapshot
4. insert exact owned snapshot entries via `--cacheinfo`
5. write tree
6. create commit with parent:

   * previous hidden checkpoint if it exists
   * otherwise visible HEAD if it exists
7. CAS update the hidden ref

## Materialization code

```rust
pub fn materialize_hidden_checkpoint(
    git: &dyn GitBackend,
    head_oid: Option<&str>,
    hidden_ref: &str,
    previous_hidden_oid: Option<&str>,
    strict: &StrictSnapshot,
    pathspecs: &[String],
    message: &str,
) -> anyhow::Result<String> {
    let temp_index = git.create_temp_index()?;

    if let Some(head) = head_oid {
        git.read_tree_into_index(temp_index.path(), head)?;
    }

    let head_owned = if let Some(head) = head_oid {
        git.list_head_owned_paths(head, pathspecs)?
    } else {
        Vec::new()
    };

    let strict_paths: std::collections::BTreeSet<_> =
        strict.entries.iter().map(|e| e.path.clone()).collect();

    let deletes: Vec<RepoPath> = head_owned
        .into_iter()
        .filter(|p| !strict_paths.contains(p))
        .collect();

    if !deletes.is_empty() {
        git.force_remove_from_index(temp_index.path(), &deletes)?;
    }

    for entry in &strict.entries {
        git.update_index_cacheinfo(
            temp_index.path(),
            entry.mode,
            &entry.git_oid,
            &entry.path,
        )?;
    }

    let tree_oid = git.write_tree_from_index(temp_index.path())?;

    let mut parents = Vec::new();
    if let Some(prev) = previous_hidden_oid {
        parents.push(prev.to_string());
    } else if let Some(head) = head_oid {
        parents.push(head.to_string());
    }

    let commit_oid = git.commit_tree(&tree_oid, &parents, message)?;
    git.update_ref_cas(hidden_ref, &commit_oid, previous_hidden_oid)?;

    Ok(commit_oid)
}
```

### Why seed from HEAD

Unowned paths should remain exactly as visible HEAD defines them. That keeps hidden checkpoints bounded to the owned surface.

### Why use CAS on `update-ref`

Even with a local lock, CAS protects you against stale assumptions if some other process touched the hidden ref anyway.

---

# 18. Stop transaction

This is the single most important workflow.

## Behavior

1. parse `session_id`, `turn_id`
2. resolve repo root, head, stream
3. acquire lock
4. load manager for stream
5. load turn
6. if missing turn, no-op
7. if stream changed since turn start, delete turn and no-op
8. capture strict snapshot inside lock
9. persist manifest
10. compute global delta vs anchor
11. classify
12. record pending or materialize hidden checkpoint
13. save manager
14. delete turn
15. append journal event
16. release lock

## Transaction code

```rust
pub fn run_checkpoint_turn(
    ctx: &CheckpointContext,
    git: &dyn GitBackend,
    stores: &Stores,
    clock: &dyn Clock,
) -> anyhow::Result<CheckpointOutcome> {
    let _lock = match RepoLock::try_acquire(
        &stores.lock_path,
        std::time::Duration::from_secs(ctx.policy.checkpoint.lock_timeout_seconds as u64),
    )? {
        Some(lock) => lock,
        None => return Ok(CheckpointOutcome::Noop("lock-busy")),
    };

    let now = clock.now_unix();
    let head = git.head_state()?;
    let stream = compute_stream_identity(&ctx.repo_root, &head);

    let mut manager = match stores.manager.load_stream(&stream.stream_id)? {
        Some(m) => m,
        None => ensure_stream_initialized(
            &ctx.repo_root,
            git,
            stores,
            &stream,
            &head,
            now,
            &ctx.pathspecs,
            &ctx.policy,
        )?,
    };

    let turn = match stores.turns.load(&stream.stream_id, &ctx.session_id, &ctx.turn_id)? {
        Some(t) => t,
        None => return Ok(CheckpointOutcome::Noop("missing-turn")),
    };

    let snap = capture_strict_snapshot(&ctx.repo_root, git, &ctx.pathspecs)?;
    stores.manifests.put(&stream.stream_id, &snap.manifest_id, &snap)?;

    let anchor = stores.manifests.get::<StrictSnapshot>(&stream.stream_id, &manager.anchor.manifest_id)?;
    let changed_paths = changed_path_count(&anchor.entries, &snap.entries) as u32;

    let decision = classify(&ClassifyInput {
        stream_id_now: &stream.stream_id,
        stream_id_at_start: &turn.stream_id_at_start,
        now_unix: now,
        anchor_fingerprint: &manager.anchor.fingerprint,
        turn_baseline_fingerprint: &turn.baseline_fingerprint,
        anchor_fingerprint_at_start: &turn.anchor_fingerprint_at_start,
        current_fingerprint: &snap.fingerprint,
        global_changed_paths: changed_paths,
        pending_turn_count: manager.pending.as_ref().map(|p| p.pending_turn_count).unwrap_or(0),
        pending_first_seen_at: manager.pending.as_ref().map(|p| p.first_seen_at),
        turn_threshold: ctx.policy.checkpoint.turn_threshold,
        file_threshold: ctx.policy.checkpoint.file_threshold,
        age_seconds: (ctx.policy.checkpoint.age_minutes as i64) * 60,
    });

    let outcome = match decision {
        Decision::Noop(reason) => {
            manager.last_seen = Some(Observation {
                fingerprint: snap.fingerprint.clone(),
                manifest_id: snap.manifest_id.clone(),
                seen_at: now,
                observed_head_oid: head.oid.clone(),
            });
            stores.manager.save_stream(&stream.stream_id, &manager)?;
            stores.turns.delete(&stream.stream_id, &ctx.session_id, &ctx.turn_id)?;
            CheckpointOutcome::Noop(match reason {
                NoopReason::MatchesAnchor => "matches-anchor",
                NoopReason::MissingTurn => "missing-turn",
                NoopReason::StreamChanged => "stream-changed",
            })
        }

        Decision::RecordPending { source, .. } => {
            manager.pending = Some(reconcile_pending(
                manager.pending.take(),
                &ctx.session_id,
                source,
                now,
                &snap,
            ));
            manager.last_seen = Some(Observation {
                fingerprint: snap.fingerprint.clone(),
                manifest_id: snap.manifest_id.clone(),
                seen_at: now,
                observed_head_oid: head.oid.clone(),
            });
            stores.manager.save_stream(&stream.stream_id, &manager)?;
            stores.turns.delete(&stream.stream_id, &ctx.session_id, &ctx.turn_id)?;
            CheckpointOutcome::Pending
        }

        Decision::Materialize { source, .. } => {
            let msg = build_checkpoint_message(
                &ctx.policy,
                source,
                &snap,
                manager.generation + 1,
                &head,
                &stream,
            );

            let commit_oid = materialize_hidden_checkpoint(
                git,
                head.oid.as_deref(),
                &stream.hidden_ref,
                Some(&manager.anchor.checkpoint_commit_oid),
                &snap,
                &ctx.pathspecs,
                &msg,
            )?;

            manager.generation += 1;
            manager.anchor = AnchorState {
                checkpoint_commit_oid: commit_oid.clone(),
                manifest_id: snap.manifest_id.clone(),
                fingerprint: snap.fingerprint.clone(),
                observed_head_oid: head.oid.clone(),
                observed_head_ref: head.symref.clone(),
                materialized_at: now,
            };
            manager.pending = None;
            manager.last_seen = Some(Observation {
                fingerprint: snap.fingerprint.clone(),
                manifest_id: snap.manifest_id.clone(),
                seen_at: now,
                observed_head_oid: head.oid.clone(),
            });

            stores.manager.save_stream(&stream.stream_id, &manager)?;
            stores.turns.delete(&stream.stream_id, &ctx.session_id, &ctx.turn_id)?;

            CheckpointOutcome::Materialized { commit_oid }
        }
    };

    stores.journal.append(&stream.stream_id, JournalEvent::from_outcome(&outcome, &ctx.session_id, &ctx.turn_id, now))?;
    Ok(outcome)
}
```

This is the real core.

---

# 19. Commit message builder

Keep the subject boring and put the useful stuff in footers.

```rust
pub fn build_checkpoint_message(
    policy: &Policy,
    source: PendingSource,
    snap: &StrictSnapshot,
    generation: u64,
    head: &HeadState,
    stream: &StreamIdentity,
) -> String {
    let subject = policy.checkpoint.message_template
        .replace("{area}", &policy.checkpoint.default_area);

    format!(
        "{subject}\n\n\
         Sprocket-Generation: {generation}\n\
         Sprocket-Source: {:?}\n\
         Sprocket-Fingerprint: {}\n\
         Sprocket-Observed-Head: {}\n\
         Sprocket-Stream: {}\n\
         Sprocket-Worktree: {}\n",
        source,
        snap.fingerprint,
        head.oid.clone().unwrap_or_else(|| "none".to_string()),
        stream.display_name,
        stream.worktree_id,
    )
}
```

Do not try to infer smart natural-language commit subjects in the checkpoint core.

---

# 20. Promotion to visible commits

This should be a separate module and **not** part of initial GA.

## Promotion modes

* `hidden_only` — ship first
* `hidden_then_promote` — second milestone
* `visible_direct` — only for owned agent branches

## Promotion preconditions

Promotion may proceed only if all are true:

1. current stream still matches the manager stream
2. repo is not mid-merge/rebase/cherry-pick
3. no staged changes exist outside owned surface
4. validators pass
5. current HEAD still matches the promotion precondition
6. policy explicitly allows promotion

If any precondition fails:

* hidden checkpoint remains valid
* visible promotion skips
* journal event records why

## Promotion philosophy

Hidden checkpointing is for safety.
Visible promotion is for history hygiene.

Never couple the first to the second.

---

# 21. Git mutation guard

Keep both guard layers.

## Codex-side

Keep `PreToolUse` to deny obvious mutating Git commands.

## Git-side

Install a `prepare-commit-msg` hook that aborts commits unless Sprocket explicitly allows them.

A minimal hook file:

```bash
#!/usr/bin/env bash
set -euo pipefail

if [[ "${SPROCKET_ALLOW_COMMIT:-}" == "1" ]]; then
  exit 0
fi

echo "Commits are owned by Sprocket. Make code changes only." >&2
exit 1
```

During controlled visible promotion, set `SPROCKET_ALLOW_COMMIT=1`.

### Why `prepare-commit-msg`

This is a cooperative guard that is not bypassed by `--no-verify`, unlike `pre-commit` and `commit-msg`.

It is still not a security boundary against malicious plumbing commands, but it is much stronger than only relying on Codex `PreToolUse`.

---

# 22. Journaling

Append NDJSON, never rewrite.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum JournalEvent {
    SessionStart {
        ts: i64,
        session_id: String,
        stream_id: String,
    },
    Baseline {
        ts: i64,
        session_id: String,
        turn_id: String,
        baseline_fingerprint: String,
    },
    StopDecision {
        ts: i64,
        session_id: String,
        turn_id: String,
        outcome: String,
        commit_oid: Option<String>,
    },
    PromotionSkipped {
        ts: i64,
        reason: String,
    },
}
```

This file will save you a lot of time once concurrency bugs show up.

---

# 23. Test harness

The correct test strategy is:

* unit tests for pure logic
* integration tests with real Git repos in temp dirs
* no mocking around the Git core

## Integration harness helper

```rust
pub struct TestRepo {
    pub dir: tempfile::TempDir,
    pub root: std::path::PathBuf,
}

impl TestRepo {
    pub fn new() -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().to_path_buf();

        run_git(&root, ["init"])?;
        run_git(&root, ["config", "user.name", "Test User"])?;
        run_git(&root, ["config", "user.email", "test@example.com"])?;

        Ok(Self { dir, root })
    }

    pub fn write(&self, rel: &str, contents: &str) -> anyhow::Result<()> {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)?;
        Ok(())
    }

    pub fn commit_all(&self, message: &str) -> anyhow::Result<()> {
        run_git(&self.root, ["add", "."])?;
        run_git(&self.root, ["commit", "-m", message])?;
        Ok(())
    }
}

fn run_git<const N: usize>(cwd: &std::path::Path, args: [&str; N]) -> anyhow::Result<()> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()?;

    if !out.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}
```

## High-value test: inherited dirty state does not starve

```rust
#[test]
fn inherited_dirty_state_eventually_materializes() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;
    repo.write("src/lib.rs", "pub fn a() {}\n")?;
    repo.commit_all("init")?;

    // Initialize Sprocket stream.
    let ctx = test_ctx(&repo.root)?;
    run_session_start(&ctx)?;

    // Dirty the workspace outside of any turn.
    repo.write("src/lib.rs", "pub fn a() { println!(\"x\"); }\n")?;

    // New session begins; inherited dirty should be noticed.
    run_session_start(&ctx)?;

    // Turn 1: user asks something, agent changes nothing.
    run_baseline(&ctx, "s1", "t1")?;
    let out1 = run_checkpoint(&ctx, "s1", "t1")?;
    assert_eq!(out1.kind(), "pending");

    // Turn 2: again, no extra edits.
    run_baseline(&ctx, "s1", "t2")?;
    let out2 = run_checkpoint(&ctx, "s1", "t2")?;
    assert_eq!(out2.kind(), "materialized");

    Ok(())
}
```

That test directly proves the v1 starvation bug is fixed.

## High-value test: hidden checkpoint tree matches observed snapshot

```rust
#[test]
fn hidden_checkpoint_tree_matches_snapshot() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;
    repo.write("src/main.rs", "fn main() {}\n")?;
    repo.commit_all("init")?;

    let ctx = test_ctx(&repo.root)?;
    run_session_start(&ctx)?;

    repo.write("src/main.rs", "fn main() { println!(\"hi\"); }\n")?;
    run_baseline(&ctx, "s1", "t1")?;
    let out = run_checkpoint_force_materialize(&ctx, "s1", "t1")?;
    let commit_oid = out.commit_oid().unwrap();

    let blob = show_file_at_commit(&repo.root, &commit_oid, "src/main.rs")?;
    assert_eq!(blob, b"fn main() { println!(\"hi\"); }\n");

    Ok(())
}
```

## Other mandatory tests

1. same-turn no changes, anchor matches, noop
2. new untracked file under owned surface
3. delete tracked file
4. executable bit change
5. symlink add/modify/delete
6. overlapping sessions on same stream
7. stream change between baseline and stop
8. lock busy
9. stale lock reap
10. dirty age threshold
11. file-count threshold
12. turn-count threshold
13. hidden ref CAS failure
14. detached HEAD
15. initial bootstrap on dirty repo
16. merge/rebase in progress with hidden-only mode
17. promotion skip because user has foreign staged changes

---

# 24. Performance roadmap

Correctness first, then speed.

## Ship in v2 GA

* full strict snapshot every baseline/stop
* BLAKE3 content hashing
* Git-driven path listing
* temp-index hidden commit materialization

This will already be fast enough for many repos.

## Add in v2.1

A cheap prefilter:

* path list from Git
* per-file signature: `(mode, size, mtime_ns)`
* fast fingerprint from signatures
* if identical to last seen fast fingerprint, skip content hashing

But do **not** let the fast path update anchors. Only let it short-circuit full snapshot recomputation when nothing changed.

That keeps correctness intact.

---

# 25. Implementation order

This is the exact sequence I would follow.

## Phase 1: hard foundation

1. `RepoPath`, `HeadState`, `StreamIdentity`
2. `GitBackend` trait
3. `GitCli` implementation for:

   * repo root
   * git path
   * head state
   * ls-files
   * hash-object
   * temp index
   * read-tree
   * update-index --cacheinfo
   * write-tree
   * commit-tree
   * update-ref
4. atomic store helpers
5. manifest store
6. lock

## Phase 2: state and initialization

7. manager/turn/session structs
8. stream runtime layout
9. `ensure_stream_initialized`
10. `SessionStart`

## Phase 3: checkpoint core

11. snapshot capture
12. delta computation
13. pending reconciliation
14. classification
15. `UserPromptSubmit`
16. `Stop` transaction
17. journaling

## Phase 4: install/guard

18. Codex hook JSON merge
19. PreToolUse guard
20. prepare-commit-msg hook install

## Phase 5: test matrix

21. unit tests for pure functions
22. integration tests for hidden checkpoint exactness
23. concurrency tests using lock contention
24. stream-switch tests

## Phase 6: later features

25. promotion engine
26. validator loop
27. cheap-scan optimization
28. sparse checkout support

---

# 26. Final opinionated recommendations

These are the strongest architectural opinions I have on this codebase.

### 1. Hidden checkpoints are the product

Treat visible promotion as a separate optional feature. That one decision will keep the core sane.

### 2. Stream-scope everything

Branch changes are not incidental. They are a state boundary.

### 3. Make bootstrap create a real hidden commit

Never let your anchor be “metadata only.”

### 4. Use Git pathspecs as policy language

Do not invent your own path matcher.

### 5. Use byte-oriented repo paths

UTF-8 assumptions are not compatible with “any codebase.”

### 6. Reject sparse checkout until explicitly supported

Do not fake correctness there.

### 7. Keep the entire core deterministic and journaled

You should be able to explain every commit or non-commit after the fact from state + journal.

### 8. Keep the CLI thin

The CLI should just parse args and dispatch into `app::*`.

---

The correct first coding pass is:

* `infra/git.rs`
* `infra/git_cli.rs`
* `domain/decision.rs`
* `engine/materialize_hidden.rs`
* `app/session_start.rs`
* `app/baseline.rs`
* `app/checkpoint.rs`

That sequence gives you the real correctness core before you spend time on polish.
