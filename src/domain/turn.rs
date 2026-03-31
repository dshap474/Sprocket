use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnState {
    pub version: u32,
    pub session_id: String,
    pub turn_id: String,
    pub stream_id_at_start: String,
    pub started_at: i64,
    pub baseline_fingerprint: String,
    pub baseline_manifest_id: String,
    pub anchor_fingerprint_at_start: String,
    pub anchor_manifest_id_at_start: String,
}
