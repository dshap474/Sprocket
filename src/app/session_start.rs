use anyhow::Result;
use serde_json::Value;

use crate::app::support::{
    RepoSupportContext, ensure_supported_repo, journal_lock_busy, load_policy,
};
use crate::cli::resolve_repo_from_target;
use crate::codex::payload::{cwd, explicit_session_id, session_id};
use crate::engine::init_stream::{
    adopt_pending_snapshot, ensure_stream_initialized, journal_session_start, refresh_session,
    resolve_stream,
};
use crate::engine::observe::capture_strict_snapshot;
use crate::engine::session_commit::{
    create_or_refresh_tracker, refresh_active_sessions, tracker_head_snapshot,
};
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
    let policy = load_policy(&git, &stores, &stream, "session-start", now)?;
    let repo_state = git.repo_state()?;
    if !ensure_supported_repo(RepoSupportContext {
        git: &git,
        stores: &stores,
        stream: &stream,
        hook: "session-start",
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
            journal_lock_busy(&stores, &stream, "session-start", now)?;
            return Ok(());
        }
    };

    let session_id = session_id(payload);
    let mut manager = ensure_stream_initialized(&git, &stores, &stream, &head, now, &policy)?;
    let current = capture_strict_snapshot(git.repo_root(), &git, &policy)?;
    stores.manifests.put(&current.manifest_id, &current)?;
    let head_snapshot = tracker_head_snapshot(&git, &stores, head.oid.as_deref(), &policy)?;
    create_or_refresh_tracker(
        &stores,
        explicit_session_id(payload),
        &stream,
        &head,
        &head_snapshot,
        &current,
        now,
    )?;
    if current.materialized_fingerprint != manager.anchor.materialized_fingerprint {
        manager.pending = Some(adopt_pending_snapshot(
            manager.pending.take(),
            &session_id,
            now,
            &current,
        ));
    } else {
        manager.pending = None;
    }
    manager.last_seen = Some(crate::domain::session::Observation {
        materialized_fingerprint: current.materialized_fingerprint.clone(),
        observed_fingerprint: current.observed_fingerprint.clone(),
        manifest_id: current.manifest_id.clone(),
        seen_at: now,
        observed_head_oid: head.oid.clone(),
    });
    refresh_session(&stores, &session_id, &stream.stream_id, now)?;
    refresh_active_sessions(&stores, &mut manager, explicit_session_id(payload), now)?;
    stores.manager.save(&manager)?;
    journal_session_start(&stores, &session_id, &stream.stream_id, now)?;
    Ok(())
}
