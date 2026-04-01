use anyhow::Result;
use serde_json::Value;

use crate::app::support::{
    RepoSupportContext, ensure_supported_repo, journal_lock_busy, load_policy,
};
use crate::cli::resolve_repo_from_target;
use crate::codex::payload::{cwd, session_id, turn_id};
use crate::domain::journal::JournalEvent;
use crate::domain::turn::TurnState;
use crate::engine::init_stream::{ensure_stream_initialized, refresh_session, resolve_stream};
use crate::engine::observe::capture_strict_snapshot;
use crate::infra::clock::{Clock, SystemClock};
use crate::infra::git::GitBackend;
use crate::infra::git_cli::GitCli;
use crate::infra::lock::RepoLock;
use crate::infra::store::{RuntimeLayout, Stores};

pub fn run(payload: &Value) -> Result<()> {
    let clock = SystemClock;
    let now = clock.now_unix();
    let cwd = cwd(payload).unwrap_or(std::env::current_dir()?);
    let repo = resolve_repo_from_target(Some(&cwd))?;
    let git = GitCli::discover(&repo)?;
    let (head, stream) = resolve_stream(&git)?;
    let runtime = RuntimeLayout::from_git(&git)?;
    let stores = Stores::for_stream(runtime, &stream.stream_id);
    let policy = load_policy(&git, &stores, &stream, "baseline", now)?;
    let repo_state = git.repo_state()?;
    if !ensure_supported_repo(RepoSupportContext {
        git: &git,
        stores: &stores,
        stream: &stream,
        hook: "baseline",
        now,
        head: &head,
        repo_state: &repo_state,
        policy: &policy,
    })? {
        return Ok(());
    }
    let _lock = match RepoLock::try_acquire(&stores.lock_path)? {
        Some(lock) => lock,
        None => {
            journal_lock_busy(&stores, &stream, "baseline", now)?;
            return Ok(());
        }
    };

    let manager = ensure_stream_initialized(&git, &stores, &stream, &head, now, &policy)?;
    let snapshot = capture_strict_snapshot(git.repo_root(), &git, &policy)?;
    stores.manifests.put(&snapshot.manifest_id, &snapshot)?;

    let turn = TurnState {
        version: 3,
        session_id: session_id(payload),
        turn_id: turn_id(payload),
        stream_id_at_start: stream.stream_id.clone(),
        stream_class_at_start: stream.class.clone(),
        policy_epoch_at_start: policy.policy_epoch().0,
        started_at: now,
        baseline_materialized_fingerprint: snapshot.materialized_fingerprint.clone(),
        baseline_manifest_id: snapshot.manifest_id.clone(),
        anchor_materialized_fingerprint_at_start: manager.anchor.materialized_fingerprint.clone(),
        anchor_manifest_id_at_start: manager.anchor.manifest_id.clone(),
    };
    stores.turns.save(&turn)?;
    refresh_session(&stores, &turn.session_id, &stream.stream_id, now)?;
    stores.journal.append(&JournalEvent::Baseline {
        ts: now,
        session_id: turn.session_id,
        turn_id: turn.turn_id,
        stream_id: stream.stream_id,
        baseline_materialized_fingerprint: snapshot.materialized_fingerprint,
    })?;
    Ok(())
}
