use std::process::Command;

use anyhow::Result;

use crate::domain::policy::{CheckpointMode, Policy};
use crate::domain::session::{HeadState, RepoState, StreamIdentity};
use crate::infra::git::GitBackend;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromotionOutcome {
    Skipped(String),
    Promoted(String),
    Blocked(String),
}

pub fn maybe_promote_visible(
    git: &dyn GitBackend,
    policy: &Policy,
    repo_state: &RepoState,
    head: &HeadState,
    hidden_commit_oid: &str,
    _stream: &StreamIdentity,
) -> Result<PromotionOutcome> {
    if policy.checkpoint.mode == CheckpointMode::HiddenOnly || !policy.promotion.enabled {
        return Ok(PromotionOutcome::Skipped("hidden-only".to_string()));
    }
    if let Some(reason) = repo_state.promotion_blocker() {
        return Ok(PromotionOutcome::Skipped(reason.to_string()));
    }

    let all_staged = git.staged_paths()?;
    let owned_staged = git
        .staged_paths_matching(&policy.git_include_pathspecs())?
        .into_iter()
        .filter(|path| policy.is_owned_path(path))
        .collect::<Vec<_>>();
    if all_staged.len() != owned_staged.len() {
        return Ok(PromotionOutcome::Skipped(
            "foreign-staged-changes".to_string(),
        ));
    }

    for validator in &policy.promotion.validators {
        let status = Command::new("sh")
            .arg("-lc")
            .arg(validator)
            .current_dir(git.repo_root())
            .status()?;
        if !status.success() {
            if policy.promotion.continue_on_failure {
                return Ok(PromotionOutcome::Skipped(format!(
                    "validator-failed:{validator}"
                )));
            }
            return Ok(PromotionOutcome::Blocked(format!(
                "Sprocket validator failed: {validator}"
            )));
        }
    }

    let tree_oid = git.commit_tree_oid(hidden_commit_oid)?;
    let message = git.commit_message(hidden_commit_oid)?;
    let parents = head.oid.clone().into_iter().collect::<Vec<_>>();
    let visible_commit = git.commit_tree(&tree_oid, &parents, &message)?;
    git.advance_head_to_commit(head, &visible_commit, head.oid.as_deref())?;
    git.sync_main_index_to_tree(&visible_commit)?;
    Ok(PromotionOutcome::Promoted(visible_commit))
}
