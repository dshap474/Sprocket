use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::manifest::StrictSnapshot;
use crate::domain::session::{Observation, StreamClass, StreamIdentity};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerState {
    pub version: u32,
    pub stream: StreamIdentity,
    pub generation: u64,
    pub anchor: AnchorState,
    pub pending: Option<PendingEpisode>,
    pub last_seen: Option<Observation>,
    pub active_sessions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorState {
    pub checkpoint_commit_oid: String,
    pub manifest_id: String,
    pub materialized_fingerprint: String,
    pub observed_fingerprint: Option<String>,
    pub policy_epoch: String,
    pub stream_class: StreamClass,
    pub observed_head_oid: Option<String>,
    pub observed_head_ref: Option<String>,
    pub materialized_at: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PendingSource {
    TurnLocal,
    Inherited,
    Mixed,
    External,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingEpisode {
    pub epoch_id: String,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    pub first_seen_materialized_fingerprint: String,
    pub latest_materialized_fingerprint: String,
    pub latest_manifest_id: String,
    pub pending_turn_count: u32,
    pub source: PendingSource,
    pub touched_sessions: Vec<String>,
}

pub fn reconcile_pending(
    existing: Option<PendingEpisode>,
    session_id: &str,
    source: PendingSource,
    now: i64,
    snapshot: &StrictSnapshot,
) -> PendingEpisode {
    match existing {
        None => PendingEpisode {
            epoch_id: Uuid::new_v4().to_string(),
            first_seen_at: now,
            last_seen_at: now,
            first_seen_materialized_fingerprint: snapshot.materialized_fingerprint.clone(),
            latest_materialized_fingerprint: snapshot.materialized_fingerprint.clone(),
            latest_manifest_id: snapshot.manifest_id.clone(),
            pending_turn_count: 1,
            source,
            touched_sessions: vec![session_id.to_string()],
        },
        Some(mut episode) => {
            episode.last_seen_at = now;
            episode.latest_materialized_fingerprint = snapshot.materialized_fingerprint.clone();
            episode.latest_manifest_id = snapshot.manifest_id.clone();
            episode.pending_turn_count = episode.pending_turn_count.saturating_add(1);
            episode.source = merge_pending_source(episode.source, source);
            if !episode
                .touched_sessions
                .iter()
                .any(|current| current == session_id)
            {
                episode.touched_sessions.push(session_id.to_string());
            }
            episode
        }
    }
}

pub fn merge_pending_source(left: PendingSource, right: PendingSource) -> PendingSource {
    use PendingSource::*;

    match (left, right) {
        (Mixed, _) | (_, Mixed) => Mixed,
        (current, incoming) if current == incoming => current,
        _ => Mixed,
    }
}
