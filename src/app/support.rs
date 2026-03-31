use anyhow::{Context, Result, anyhow};

use crate::domain::journal::JournalEvent;
use crate::domain::policy::Policy;
use crate::domain::session::{RepoState, StreamIdentity};
use crate::infra::git::GitBackend;
use crate::infra::store::{Stores, load_toml};

pub fn load_policy(
    git: &dyn GitBackend,
    stores: &Stores,
    stream: &StreamIdentity,
    hook: &str,
    now: i64,
) -> Result<Policy> {
    let path = git.repo_root().join(".sprocket/policy.toml");
    if !path.exists() {
        return Ok(Policy::default());
    }

    match load_toml(&path) {
        Ok(policy) => Ok(policy),
        Err(error) => {
            let reason = format!("invalid-policy: {error}");
            let _ = stores.journal.append(&JournalEvent::HookError {
                ts: now,
                stream_id: stream.stream_id.clone(),
                hook: hook.to_string(),
                reason: reason.clone(),
            });
            Err(anyhow!(reason)).with_context(|| format!("failed to load {}", path.display()))
        }
    }
}

pub fn ensure_supported_repo(
    stores: &Stores,
    stream: &StreamIdentity,
    hook: &str,
    now: i64,
    repo_state: &RepoState,
    policy: &Policy,
) -> Result<bool> {
    if repo_state.sparse_checkout && !policy.compat.allow_sparse_checkout {
        stores.journal.append(&JournalEvent::HookNoop {
            ts: now,
            stream_id: stream.stream_id.clone(),
            hook: hook.to_string(),
            reason: "sparse-checkout-unsupported".to_string(),
        })?;
        return Ok(false);
    }

    Ok(true)
}
