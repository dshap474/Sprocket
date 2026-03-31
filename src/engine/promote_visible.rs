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
    expected_head_oid: Option<&str>,
    hidden_commit_oid: &str,
    _stream: &StreamIdentity,
) -> Result<PromotionOutcome> {
    if policy.checkpoint.mode == CheckpointMode::HiddenOnly || !policy.promotion.enabled {
        return Ok(PromotionOutcome::Skipped("hidden-only".to_string()));
    }
    if head.oid.is_none() {
        return Ok(PromotionOutcome::Skipped("unborn-head".to_string()));
    }
    if let Some(reason) = repo_state.promotion_blocker() {
        return Ok(PromotionOutcome::Skipped(reason.to_string()));
    }
    let current_head = git.head_state()?;
    if current_head.oid != head.oid
        || current_head.symref != head.symref
        || expected_head_oid != current_head.oid.as_deref()
    {
        return Ok(PromotionOutcome::Skipped("head-moved".to_string()));
    }

    let all_staged = git.staged_paths()?;
    let owned_staged = git.staged_paths_matching(&policy.git_owned_pathspecs())?;
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
    let visible_commit = match git.commit_tree(&tree_oid, &parents, &message) {
        Ok(commit_oid) => commit_oid,
        Err(error) => {
            return Ok(PromotionOutcome::Skipped(format!(
                "promotion-commit-failed:{error}"
            )));
        }
    };

    if let Err(error) = git.sync_main_index_to_tree(&visible_commit) {
        return Ok(PromotionOutcome::Skipped(format!(
            "promotion-index-sync-failed:{error}"
        )));
    }

    if let Err(error) = git.advance_head_to_commit(head, &visible_commit, head.oid.as_deref()) {
        if let Some(previous_head) = head.oid.as_deref() {
            let _ = git.sync_main_index_to_tree(previous_head);
        }
        return Ok(PromotionOutcome::Skipped(format!(
            "promotion-head-advance-failed:{error}"
        )));
    }

    Ok(PromotionOutcome::Promoted(visible_commit))
}
