use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::domain::manifest::StrictEntry;
use crate::domain::repopath::RepoPath;

pub const SESSION_TRACKER_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionTrackerStatus {
    Active,
    Blocked,
    Committing,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PathClaimState {
    Exclusive,
    InheritedDirty,
    Contended,
    ExternalInterference,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedPathState {
    pub first_touched_at: i64,
    pub last_touched_at: i64,
    pub first_turn_id: String,
    pub last_turn_id: String,
    pub claim_state: PathClaimState,
    pub start_head_oid: Option<String>,
    pub start_worktree_oid: Option<String>,
    pub current_oid: Option<String>,
    pub claimed_by_session: String,
    pub other_sessions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTracker {
    pub version: u32,
    pub session_id: String,
    pub stream_id: String,
    pub epoch: u32,
    pub status: SessionTrackerStatus,
    pub start_head_oid: Option<String>,
    pub start_head_manifest_id: String,
    pub epoch_start_worktree_manifest_id: String,
    pub last_seen_manifest_id: String,
    pub first_dirty_at: Option<i64>,
    pub last_dirty_at: Option<i64>,
    pub turn_count_since_reset: u32,
    #[serde(with = "tracked_paths_serde")]
    pub touched_paths: BTreeMap<RepoPath, TrackedPathState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum CommitBlockReason {
    UnstableSessionId,
    MissingTracker,
    HeadMoved,
    StagedChangesPresent,
    UnsupportedRepoState,
    ThresholdsNotMet,
    PathInheritedDirty,
    PathContended,
    ExternalInterference,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockedPath {
    pub path: RepoPath,
    pub reasons: Vec<CommitBlockReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitPlan {
    pub version: u32,
    pub session_id: String,
    pub stream_id: String,
    pub epoch: u32,
    pub thresholds_met: bool,
    pub safe_to_commit: bool,
    pub eligible_paths: Vec<RepoPath>,
    pub blocked_paths: Vec<BlockedPath>,
    pub blocking_reasons: Vec<CommitBlockReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionThresholdInput {
    pub turn_count_since_reset: u32,
    pub exclusive_path_count: u32,
    pub first_dirty_at: Option<i64>,
    pub now_unix: i64,
    pub turn_threshold: u32,
    pub file_threshold: u32,
    pub age_seconds: i64,
}

pub fn thresholds_met(input: &SessionThresholdInput) -> bool {
    let dirty_age = input
        .first_dirty_at
        .map(|first_seen| input.now_unix.saturating_sub(first_seen))
        .unwrap_or(0);
    input.turn_count_since_reset >= input.turn_threshold
        || input.exclusive_path_count >= input.file_threshold
        || dirty_age >= input.age_seconds
}

pub fn entry_oid(entry: Option<&StrictEntry>) -> Option<String> {
    entry.map(|current| current.git_oid.clone())
}

mod tracked_paths_serde {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use crate::domain::repopath::RepoPath;

    use super::TrackedPathState;

    #[derive(Serialize, Deserialize)]
    struct TrackedPathRecord {
        path: RepoPath,
        state: TrackedPathState,
    }

    pub fn serialize<S>(
        map: &BTreeMap<RepoPath, TrackedPathState>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let records = map
            .iter()
            .map(|(path, state)| TrackedPathRecord {
                path: path.clone(),
                state: state.clone(),
            })
            .collect::<Vec<_>>();
        records.serialize(serializer)
    }

    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<BTreeMap<RepoPath, TrackedPathState>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let records = Vec::<TrackedPathRecord>::deserialize(deserializer)?;
        Ok(records
            .into_iter()
            .map(|record| (record.path, record.state))
            .collect())
    }
}
