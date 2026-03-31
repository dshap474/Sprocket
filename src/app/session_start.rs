use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

use crate::cli::resolve_repo_from_target;
use crate::codex::payload::{cwd, session_id};
use crate::domain::policy::Policy;
use crate::engine::init_stream::{
    adopt_pending_snapshot, ensure_stream_initialized, journal_session_start, refresh_session,
    resolve_stream,
};
use crate::engine::observe::capture_strict_snapshot;
use crate::infra::clock::{Clock, SystemClock};
use crate::infra::git::GitBackend;
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
    let mut manager = ensure_stream_initialized(&git, &stores, &stream, &head, now, &policy)?;
    let repo_state = git.repo_state()?;
    if repo_state.sparse_checkout && !policy.compat.allow_sparse_checkout {
        return Ok(());
    }

    let current = capture_strict_snapshot(git.repo_root(), &git, &policy)?;
    stores.manifests.put(&current.manifest_id, &current)?;
    if current.fingerprint != manager.anchor.fingerprint {
        manager.pending = Some(adopt_pending_snapshot(
            manager.pending.take(),
            &session_id(payload),
            now,
            &current,
        ));
    } else {
        manager.pending = None;
    }
    manager.last_seen = Some(crate::domain::session::Observation {
        fingerprint: current.fingerprint.clone(),
        manifest_id: current.manifest_id.clone(),
        seen_at: now,
        observed_head_oid: head.oid.clone(),
    });
    stores.manager.save(&manager)?;
    refresh_session(&stores, &session_id(payload), &stream.stream_id, now)?;
    journal_session_start(&stores, &session_id(payload), &stream.stream_id, now)?;
    Ok(())
}
