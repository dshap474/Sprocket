use serde::{Deserialize, Serialize};

use crate::domain::session::StreamClass;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentPhase {
    Prepared,
    RefUpdated,
    Finalized,
    Aborted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointIntent {
    pub version: u32,
    pub ts: i64,
    pub intent_id: String,
    pub stream_id: String,
    pub hidden_ref: String,
    pub checkpoint_commit_oid: String,
    pub previous_hidden_oid: Option<String>,
    pub manifest_id: String,
    pub materialized_fingerprint: String,
    pub observed_fingerprint: Option<String>,
    pub policy_epoch: String,
    pub stream_class: StreamClass,
    pub phase: IntentPhase,
}
