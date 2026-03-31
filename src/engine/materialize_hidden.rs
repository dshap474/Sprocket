use std::collections::BTreeSet;

use anyhow::Result;

use crate::domain::manager::PendingSource;
use crate::domain::manifest::StrictSnapshot;
use crate::domain::policy::Policy;
use crate::domain::session::{HeadState, StreamIdentity};
use crate::infra::git::GitBackend;

pub fn materialize_hidden_checkpoint(
    git: &dyn GitBackend,
    head_oid: Option<&str>,
    hidden_ref: &str,
    previous_hidden_oid: Option<&str>,
    snapshot: &StrictSnapshot,
    policy: &Policy,
    message: &str,
) -> Result<String> {
    let temp_index = git.create_temp_index()?;
    if let Some(head) = head_oid {
        git.read_tree_into_index(temp_index.path(), head)?;
    }

    let head_owned = if let Some(head) = head_oid {
        git.list_head_owned_paths(head, &policy.git_owned_pathspecs())?
    } else {
        Vec::new()
    };
    let strict_paths: BTreeSet<_> = snapshot
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    let deleted: Vec<_> = head_owned
        .into_iter()
        .filter(|path| !strict_paths.contains(path))
        .collect();
    if !deleted.is_empty() {
        git.force_remove_from_index(temp_index.path(), &deleted)?;
    }

    for entry in &snapshot.entries {
        git.update_index_cacheinfo(temp_index.path(), entry.mode, &entry.git_oid, &entry.path)?;
    }

    let tree_oid = git.write_tree_from_index(temp_index.path())?;
    let mut parents = Vec::new();
    if let Some(previous) = previous_hidden_oid {
        parents.push(previous.to_string());
    } else if let Some(head) = head_oid {
        parents.push(head.to_string());
    }

    let commit_oid = git.commit_tree(&tree_oid, &parents, message)?;
    git.update_ref_cas(hidden_ref, &commit_oid, previous_hidden_oid)?;
    Ok(commit_oid)
}

pub fn build_checkpoint_message(
    policy: &Policy,
    source: PendingSource,
    snapshot: &StrictSnapshot,
    generation: u64,
    head: &HeadState,
    stream: &StreamIdentity,
) -> String {
    format!(
        "{}\n\nSprocket-Generation: {}\nSprocket-Source: {:?}\nSprocket-Fingerprint: {}\nSprocket-Observed-Head: {}\nSprocket-Stream: {}\nSprocket-Worktree: {}\n",
        policy.checkpoint_subject(),
        generation,
        source,
        snapshot.fingerprint,
        head.oid.clone().unwrap_or_else(|| "none".to_string()),
        stream.display_name,
        stream.worktree_id,
    )
}
