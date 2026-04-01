use serde::{Deserialize, Serialize};

use crate::domain::intent::IntentPhase;
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
        baseline_materialized_fingerprint: String,
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
    IntentTransition {
        ts: i64,
        stream_id: String,
        intent_id: String,
        checkpoint_commit_oid: String,
        phase: IntentPhase,
    },
    HookNoop {
        ts: i64,
        stream_id: String,
        hook: String,
        reason: String,
    },
    HookError {
        ts: i64,
        stream_id: String,
        hook: String,
        reason: String,
    },
    Recovery {
        ts: i64,
        stream_id: String,
        reason: String,
        commit_oid: Option<String>,
    },
}
