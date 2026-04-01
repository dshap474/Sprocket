use anyhow::{Context, Result, anyhow};

use crate::domain::journal::JournalEvent;
use crate::domain::policy::Policy;
use crate::domain::session::{HeadState, RepoState, StreamIdentity};
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

pub fn ensure_supported_repo(ctx: RepoSupportContext<'_>) -> Result<bool> {
    if !ctx.policy.hidden_only_mode() {
        journal_noop(
            ctx.stores,
            ctx.stream,
            ctx.hook,
            ctx.now,
            "checkpoint-mode-unsupported",
        )?;
        return Ok(false);
    }
    if ctx.head.detached {
        journal_noop(
            ctx.stores,
            ctx.stream,
            ctx.hook,
            ctx.now,
            "detached-head-unsupported",
        )?;
        return Ok(false);
    }
    if let Some(reason) = ctx.repo_state.unsupported_reason() {
        journal_noop(ctx.stores, ctx.stream, ctx.hook, ctx.now, reason)?;
        return Ok(false);
    }
    if ctx.repo_state.sparse_checkout {
        journal_noop(
            ctx.stores,
            ctx.stream,
            ctx.hook,
            ctx.now,
            "sparse-checkout-unsupported",
        )?;
        return Ok(false);
    }
    if let Some(oid) = ctx.head.oid.as_deref() {
        let entries = ctx
            .git
            .list_tree_entries(oid, &ctx.policy.git_include_pathspecs())?
            .into_iter()
            .filter(|entry| ctx.policy.matches_owned_path(&entry.path))
            .collect::<Vec<_>>();
        if entries.iter().any(|entry| entry.kind == "commit") {
            journal_noop(
                ctx.stores,
                ctx.stream,
                ctx.hook,
                ctx.now,
                "gitlinks-unsupported",
            )?;
            return Ok(false);
        }
    }
    if contains_gitattributes(ctx.git.repo_root())? {
        journal_noop(
            ctx.stores,
            ctx.stream,
            ctx.hook,
            ctx.now,
            "gitattributes-unsupported",
        )?;
        return Ok(false);
    }

    Ok(true)
}

pub struct RepoSupportContext<'a> {
    pub git: &'a dyn GitBackend,
    pub stores: &'a Stores,
    pub stream: &'a StreamIdentity,
    pub hook: &'a str,
    pub now: i64,
    pub head: &'a HeadState,
    pub repo_state: &'a RepoState,
    pub policy: &'a Policy,
}

fn journal_noop(
    stores: &Stores,
    stream: &StreamIdentity,
    hook: &str,
    now: i64,
    reason: &str,
) -> Result<()> {
    stores.journal.append(&JournalEvent::HookNoop {
        ts: now,
        stream_id: stream.stream_id.clone(),
        hook: hook.to_string(),
        reason: reason.to_string(),
    })?;
    Ok(())
}

pub fn journal_lock_busy(
    stores: &Stores,
    stream: &StreamIdentity,
    hook: &str,
    now: i64,
) -> Result<()> {
    journal_noop(stores, stream, hook, now, "lock-busy")
}

fn contains_gitattributes(root: &std::path::Path) -> Result<bool> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        for entry in std::fs::read_dir(&path)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let name = entry.file_name();
            if file_type.is_dir() {
                if name == ".git" || name == ".sprocket" {
                    continue;
                }
                stack.push(entry.path());
                continue;
            }
            if file_type.is_file() && name == ".gitattributes" {
                return Ok(true);
            }
        }
    }

    Ok(false)
}
