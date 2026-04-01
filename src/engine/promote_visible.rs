use anyhow::Result;

use crate::domain::policy::Policy;
use crate::domain::session::{HeadState, RepoState, StreamIdentity};
use crate::infra::git::GitBackend;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromotionOutcome {
    Skipped(String),
    Promoted(String),
    Blocked(String),
}

pub fn maybe_promote_visible(
    _git: &dyn GitBackend,
    _policy: &Policy,
    _repo_state: &RepoState,
    _head: &HeadState,
    _expected_head_oid: Option<&str>,
    _hidden_commit_oid: &str,
    _stream: &StreamIdentity,
) -> Result<PromotionOutcome> {
    Ok(PromotionOutcome::Skipped(
        "promotion-disabled-by-safety-envelope".to_string(),
    ))
}
