use serde::{Deserialize, Serialize};

use crate::domain::session::StreamClass;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnState {
    pub version: u32,
    pub session_id: String,
    pub turn_id: String,
    pub stream_id_at_start: String,
    pub stream_class_at_start: StreamClass,
    pub policy_epoch_at_start: String,
    pub started_at: i64,
    pub baseline_materialized_fingerprint: String,
    pub baseline_manifest_id: String,
    pub anchor_materialized_fingerprint_at_start: String,
    pub anchor_manifest_id_at_start: String,
}
