use serde::{Deserialize, Serialize};

use crate::domain::manager::PendingSource;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum JournalEvent {
    SessionStart {
        ts: i64,
        session_id: String,
        stream_id: String,
    },
    Baseline {
        ts: i64,
        session_id: String,
        turn_id: String,
        stream_id: String,
        baseline_fingerprint: String,
    },
    StopDecision {
        ts: i64,
        session_id: String,
        turn_id: String,
        stream_id: String,
        outcome: String,
        source: Option<PendingSource>,
        commit_oid: Option<String>,
    },
    PromotionSkipped {
        ts: i64,
        stream_id: String,
        reason: String,
    },
}
