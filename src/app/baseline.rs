use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

use crate::cli::resolve_repo_from_target;
use crate::codex::payload::{cwd, session_id, turn_id};
use crate::domain::journal::JournalEvent;
use crate::domain::policy::Policy;
use crate::domain::turn::TurnState;
use crate::engine::init_stream::{ensure_stream_initialized, refresh_session, resolve_stream};
use crate::engine::observe::capture_strict_snapshot;
use crate::infra::clock::{Clock, SystemClock};
use crate::infra::git_cli::GitCli;
use crate::infra::lock::RepoLock;
use crate::infra::store::{RuntimeLayout, Stores, load_toml};

pub fn run(payload: &Value) -> Result<()> {
    let clock = SystemClock;
    let cwd = cwd(payload).unwrap_or(std::env::current_dir()?);
    let repo = resolve_repo_from_target(Some(&cwd))?;
    let git = GitCli::discover(&repo)?;
    let policy: Policy = load_toml(&git.repo_root().join(".sprocket/policy.toml"))
        .unwrap_or_else(|_| Policy::default());
    let (head, stream) = resolve_stream(&git)?;
    let runtime = RuntimeLayout::from_git(&git)?;
    let stores = Stores::for_stream(runtime, &stream.stream_id);
    let _lock = match RepoLock::try_acquire(
        &stores.lock_path,
        Duration::from_secs(policy.checkpoint.lock_timeout_seconds),
    )? {
        Some(lock) => lock,
        None => return Ok(()),
    };

    let now = clock.now_unix();
    let manager = ensure_stream_initialized(&git, &stores, &stream, &head, now, &policy)?;
    let snapshot = capture_strict_snapshot(git.repo_root(), &git, &policy)?;
    stores.manifests.put(&snapshot.manifest_id, &snapshot)?;

    let turn = TurnState {
        version: 2,
        session_id: session_id(payload),
        turn_id: turn_id(payload),
        stream_id_at_start: stream.stream_id.clone(),
        started_at: now,
        baseline_fingerprint: snapshot.fingerprint.clone(),
        baseline_manifest_id: snapshot.manifest_id.clone(),
        anchor_fingerprint_at_start: manager.anchor.fingerprint.clone(),
        anchor_manifest_id_at_start: manager.anchor.manifest_id.clone(),
    };
    stores.turns.save(&turn)?;
    refresh_session(&stores, &turn.session_id, &stream.stream_id, now)?;
    stores.journal.append(&JournalEvent::Baseline {
        ts: now,
        session_id: turn.session_id,
        turn_id: turn.turn_id,
        stream_id: stream.stream_id,
        baseline_fingerprint: snapshot.fingerprint,
    })?;
    Ok(())
}
