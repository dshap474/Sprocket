use anyhow::Result;
use uuid::Uuid;

use crate::domain::intent::{CheckpointIntent, IntentPhase};
use crate::domain::journal::JournalEvent;
use crate::domain::manager::{ManagerState, PendingEpisode, PendingSource};
use crate::domain::policy::Policy;
use crate::domain::session::{HeadState, SessionState, StreamIdentity};
use crate::engine::materialize_hidden::{
    CheckpointMessageContext, build_checkpoint_message, prepare_hidden_checkpoint,
};
use crate::engine::observe::capture_strict_snapshot;
use crate::engine::repair::{
    HiddenRefManagerInput, build_manager_from_hidden_ref, policy_epoch_changed,
    reconcile_stream_state,
};
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
    let policy_epoch = policy.policy_epoch();
    if let Some(existing) = reconcile_stream_state(git, stores, stream, now, policy)? {
        if !policy_epoch_changed(&existing, &policy_epoch) {
            return Ok(existing);
        }
        stores.journal.append(&JournalEvent::Recovery {
            ts: now,
            stream_id: stream.stream_id.clone(),
            reason: "policy-epoch-changed".to_string(),
            commit_oid: Some(existing.anchor.checkpoint_commit_oid.clone()),
        })?;
        return bootstrap_anchor(
            git,
            stores,
            stream,
            head,
            now,
            policy,
            existing.generation + 1,
        );
    }

    bootstrap_anchor(git, stores, stream, head, now, policy, 1)
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
            epoch_id: Uuid::new_v4().to_string(),
            first_seen_at: now,
            last_seen_at: now,
            first_seen_materialized_fingerprint: snapshot.materialized_fingerprint.clone(),
            latest_materialized_fingerprint: snapshot.materialized_fingerprint.clone(),
            latest_manifest_id: snapshot.manifest_id.clone(),
            pending_turn_count: 0,
            source: PendingSource::Inherited,
            touched_sessions: vec![session_id.to_string()],
        },
        Some(mut pending) => {
            pending.last_seen_at = now;
            pending.latest_materialized_fingerprint = snapshot.materialized_fingerprint.clone();
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
    let stream = crate::domain::ids::compute_stream_identity(git.repo_root(), &head);
    Ok((head, stream))
}

fn bootstrap_anchor(
    git: &dyn GitBackend,
    stores: &Stores,
    stream: &StreamIdentity,
    head: &HeadState,
    now: i64,
    policy: &Policy,
    generation: u64,
) -> Result<ManagerState> {
    let snapshot = capture_strict_snapshot(git.repo_root(), git, policy)?;
    let policy_epoch = policy.policy_epoch();
    let head_owned_paths = head
        .oid
        .as_deref()
        .map(|oid| {
            git.list_head_owned_paths(oid, &policy.git_include_pathspecs())
                .map(|paths| {
                    paths
                        .into_iter()
                        .filter(|path| policy.matches_owned_path(path))
                        .collect::<Vec<_>>()
                })
        })
        .transpose()?
        .unwrap_or_default();
    let message = build_checkpoint_message(CheckpointMessageContext {
        subject: &format!(
            "checkpoint({}): bootstrap anchor [auto]",
            policy.checkpoint.default_area
        ),
        generation,
        source: PendingSource::Inherited,
        snapshot: &snapshot,
        head,
        stream,
        policy_epoch: &policy_epoch,
    });
    let prepared = prepare_hidden_checkpoint(
        git,
        head.oid.as_deref(),
        None,
        &head_owned_paths,
        &snapshot,
        &message,
    )?;
    let intent = CheckpointIntent {
        version: 1,
        ts: now,
        intent_id: Uuid::new_v4().to_string(),
        stream_id: stream.stream_id.clone(),
        hidden_ref: stream.hidden_ref.clone(),
        checkpoint_commit_oid: prepared.commit_oid.clone(),
        previous_hidden_oid: None,
        manifest_id: snapshot.manifest_id.clone(),
        materialized_fingerprint: snapshot.materialized_fingerprint.clone(),
        observed_fingerprint: snapshot.observed_fingerprint.clone(),
        policy_epoch: policy_epoch.0.clone(),
        stream_class: stream.class.clone(),
        phase: IntentPhase::Prepared,
    };
    stores.intents.append(&intent)?;
    stores.journal.append(&JournalEvent::IntentTransition {
        ts: now,
        stream_id: stream.stream_id.clone(),
        intent_id: intent.intent_id.clone(),
        checkpoint_commit_oid: intent.checkpoint_commit_oid.clone(),
        phase: IntentPhase::Prepared,
    })?;
    git.update_ref_cas(&stream.hidden_ref, &prepared.commit_oid, None)?;
    let ref_updated = CheckpointIntent {
        phase: IntentPhase::RefUpdated,
        ..intent.clone()
    };
    stores.intents.append(&ref_updated)?;
    stores.journal.append(&JournalEvent::IntentTransition {
        ts: now,
        stream_id: stream.stream_id.clone(),
        intent_id: ref_updated.intent_id.clone(),
        checkpoint_commit_oid: ref_updated.checkpoint_commit_oid.clone(),
        phase: IntentPhase::RefUpdated,
    })?;

    stores.manifests.put(&snapshot.manifest_id, &snapshot)?;
    let manager = build_manager_from_hidden_ref(HiddenRefManagerInput {
        stream,
        commit_oid: &prepared.commit_oid,
        generation,
        policy_epoch: &policy_epoch.0,
        stream_class: &stream.class,
        observed_head_oid: &head.oid,
        observed_head_ref: &head.symref,
        observed_fingerprint: snapshot.observed_fingerprint.clone(),
        now,
        snapshot: &snapshot,
    });
    stores.manager.save(&manager)?;
    stores.intents.append(&CheckpointIntent {
        phase: IntentPhase::Finalized,
        ..intent
    })?;
    stores.journal.append(&JournalEvent::IntentTransition {
        ts: now,
        stream_id: stream.stream_id.clone(),
        intent_id: ref_updated.intent_id,
        checkpoint_commit_oid: prepared.commit_oid.clone(),
        phase: IntentPhase::Finalized,
    })?;
    Ok(manager)
}
