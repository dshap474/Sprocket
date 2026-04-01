use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamClass {
    Branch { symref: String },
    DetachedHead,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeadState {
    pub oid: Option<String>,
    pub symref: Option<String>,
    pub detached: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamIdentity {
    pub worktree_id: String,
    pub stream_id: String,
    pub hidden_ref: String,
    pub display_name: String,
    pub class: StreamClass,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub version: u32,
    pub session_id: String,
    pub stream_id: String,
    pub started_at: i64,
    pub last_seen_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub materialized_fingerprint: String,
    pub observed_fingerprint: Option<String>,
    pub manifest_id: String,
    pub seen_at: i64,
    pub observed_head_oid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RepoState {
    pub merge_in_progress: bool,
    pub rebase_in_progress: bool,
    pub cherry_pick_in_progress: bool,
    pub sequencer_in_progress: bool,
    pub sparse_checkout: bool,
}

impl RepoState {
    pub fn unsupported_reason(&self) -> Option<&'static str> {
        if self.merge_in_progress {
            Some("merge-in-progress")
        } else if self.rebase_in_progress {
            Some("rebase-in-progress")
        } else if self.cherry_pick_in_progress {
            Some("cherry-pick-in-progress")
        } else if self.sequencer_in_progress {
            Some("sequencer-in-progress")
        } else {
            None
        }
    }
}
