use anyhow::Result;

use crate::domain::ids::compute_stream_identity;
use crate::domain::journal::JournalEvent;
use crate::domain::manager::{AnchorState, ManagerState, PendingEpisode, PendingSource};
use crate::domain::policy::Policy;
use crate::domain::session::{HeadState, Observation, SessionState, StreamIdentity};
use crate::engine::materialize_hidden::materialize_hidden_checkpoint;
use crate::engine::observe::capture_strict_snapshot;
use crate::infra::git::GitBackend;
use crate::infra::store::Stores;

pub fn ensure_stream_initialized(
    git: &dyn GitBackend,
    stores: &Stores,
    stream: &StreamIdentity,
    head: &HeadState,
    now: i64,
    policy: &Policy,
) -> Result<ManagerState> {
    if let Some(existing) = stores.manager.load()? {
        return Ok(existing);
    }

    let snapshot = capture_strict_snapshot(git.repo_root(), git, policy)?;
    stores.manifests.put(&snapshot.manifest_id, &snapshot)?;
    let message = format!(
        "checkpoint({}): bootstrap anchor [auto]\n\nSprocket-Bootstrap: true\nSprocket-Fingerprint: {}\n",
        policy.checkpoint.default_area, snapshot.fingerprint,
    );
    let commit_oid = materialize_hidden_checkpoint(
        git,
        head.oid.as_deref(),
        &stream.hidden_ref,
        None,
        &snapshot,
        policy,
        &message,
    )?;
    let manager = ManagerState {
        version: 2,
        stream: stream.clone(),
        generation: 1,
        anchor: AnchorState {
            checkpoint_commit_oid: commit_oid,
            manifest_id: snapshot.manifest_id.clone(),
            fingerprint: snapshot.fingerprint.clone(),
            observed_head_oid: head.oid.clone(),
            observed_head_ref: head.symref.clone(),
            materialized_at: now,
        },
        pending: None,
        last_seen: Some(Observation {
            fingerprint: snapshot.fingerprint.clone(),
            manifest_id: snapshot.manifest_id.clone(),
            seen_at: now,
            observed_head_oid: head.oid.clone(),
        }),
    };
    stores.manager.save(&manager)?;
    Ok(manager)
}

pub fn refresh_session(
    stores: &Stores,
    session_id: &str,
    stream_id: &str,
    now: i64,
) -> Result<SessionState> {
    let session = stores
        .sessions
        .load(session_id)?
        .map(|mut current| {
            current.last_seen_at = now;
            current
        })
        .unwrap_or(SessionState {
            version: 2,
            session_id: session_id.to_string(),
            stream_id: stream_id.to_string(),
            started_at: now,
            last_seen_at: now,
        });
    stores.sessions.save(&session)?;
    Ok(session)
}

pub fn adopt_pending_snapshot(
    existing: Option<PendingEpisode>,
    session_id: &str,
    now: i64,
    snapshot: &crate::domain::manifest::StrictSnapshot,
) -> PendingEpisode {
    match existing {
        None => PendingEpisode {
            epoch_id: uuid::Uuid::new_v4().to_string(),
            first_seen_at: now,
            last_seen_at: now,
            first_seen_fingerprint: snapshot.fingerprint.clone(),
            latest_fingerprint: snapshot.fingerprint.clone(),
            latest_manifest_id: snapshot.manifest_id.clone(),
            pending_turn_count: 0,
            source: PendingSource::Inherited,
            touched_sessions: vec![session_id.to_string()],
        },
        Some(mut pending) => {
            pending.last_seen_at = now;
            pending.latest_fingerprint = snapshot.fingerprint.clone();
            pending.latest_manifest_id = snapshot.manifest_id.clone();
            if !pending
                .touched_sessions
                .iter()
                .any(|current| current == session_id)
            {
                pending.touched_sessions.push(session_id.to_string());
            }
            pending
        }
    }
}

pub fn journal_session_start(
    stores: &Stores,
    session_id: &str,
    stream_id: &str,
    now: i64,
) -> Result<()> {
    stores.journal.append(&JournalEvent::SessionStart {
        ts: now,
        session_id: session_id.to_string(),
        stream_id: stream_id.to_string(),
    })
}

pub fn resolve_stream(git: &dyn GitBackend) -> Result<(HeadState, StreamIdentity)> {
    let head = git.head_state()?;
    let stream = compute_stream_identity(git.repo_root(), &head);
    Ok((head, stream))
}
