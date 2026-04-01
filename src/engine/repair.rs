use anyhow::Result;

use crate::domain::ids::snapshot_fingerprint;
use crate::domain::intent::{CheckpointIntent, IntentPhase};
use crate::domain::journal::JournalEvent;
use crate::domain::manager::{AnchorState, ManagerState};
use crate::domain::manifest::{StrictEntry, StrictSnapshot};
use crate::domain::policy::{Policy, PolicyEpoch};
use crate::domain::session::{Observation, StreamIdentity};
use crate::engine::materialize_hidden::parse_checkpoint_metadata;
use crate::infra::git::{GitBackend, TreeEntry};
use crate::infra::store::Stores;

pub fn reconcile_stream_state(
    git: &dyn GitBackend,
    stores: &Stores,
    stream: &StreamIdentity,
    now: i64,
    policy: &Policy,
) -> Result<Option<ManagerState>> {
    let hidden_ref_oid = git.rev_parse_ref(&stream.hidden_ref)?;
    let intents = stores.intents.load_all()?;

    reconcile_latest_intent(stores, stream, now, hidden_ref_oid.as_deref(), &intents)?;

    let Some(commit_oid) = hidden_ref_oid else {
        stores.manager.delete()?;
        return Ok(None);
    };

    let snapshot = snapshot_from_commit(git, &commit_oid, policy)?;
    let metadata = parse_checkpoint_metadata(&git.commit_message(&commit_oid)?)?;
    let rebuilt = build_manager_from_hidden_ref(HiddenRefManagerInput {
        stream,
        commit_oid: &commit_oid,
        generation: metadata.generation,
        policy_epoch: &metadata.policy_epoch,
        stream_class: &metadata.stream_class,
        observed_head_oid: &metadata.observed_head_oid,
        observed_head_ref: &metadata.observed_head_ref,
        observed_fingerprint: metadata.observed_fingerprint.clone(),
        now,
        snapshot: &snapshot,
    });

    let current = stores.manager.load()?;
    let manifest_missing = !stores.manifests.path(&snapshot.manifest_id).exists();
    let stale = current
        .as_ref()
        .map(|cached| {
            cached.anchor.checkpoint_commit_oid != rebuilt.anchor.checkpoint_commit_oid
                || cached.anchor.materialized_fingerprint != rebuilt.anchor.materialized_fingerprint
                || cached.anchor.policy_epoch != rebuilt.anchor.policy_epoch
        })
        .unwrap_or(true);
    let manager = if stale {
        rebuilt
    } else {
        current
            .clone()
            .expect("current manager should exist when not stale")
    };
    if manifest_missing {
        stores.manifests.put(&snapshot.manifest_id, &snapshot)?;
    }
    if stale || manifest_missing {
        stores.manager.save(&manager)?;
        stores.journal.append(&JournalEvent::Recovery {
            ts: now,
            stream_id: stream.stream_id.clone(),
            reason: "rebuilt-caches-from-hidden-ref".to_string(),
            commit_oid: Some(commit_oid),
        })?;
    }

    finalize_latest_intent_if_needed(stores, stream, now, &manager, &intents)?;
    Ok(Some(manager))
}

pub fn snapshot_from_commit(
    git: &dyn GitBackend,
    commit_oid: &str,
    policy: &Policy,
) -> Result<StrictSnapshot> {
    let mut entries = git
        .list_tree_entries(commit_oid, &policy.git_include_pathspecs())?
        .into_iter()
        .filter(|entry| policy.matches_owned_path(&entry.path))
        .filter(|entry| entry.kind == "blob")
        .map(|entry| strict_entry_from_tree(git, commit_oid, entry))
        .collect::<Result<Vec<_>>>()?;
    entries.sort_by(|left, right| left.path.cmp(&right.path));

    let materialized_fingerprint = snapshot_fingerprint(
        &entries
            .iter()
            .map(|entry| (entry.path.as_bytes(), entry.mode, entry.git_oid.as_str()))
            .collect::<Vec<_>>(),
    );
    let observed_fingerprint = snapshot_fingerprint(
        &entries
            .iter()
            .map(|entry| {
                (
                    entry.path.as_bytes(),
                    entry.mode,
                    entry.observed_digest.as_str(),
                )
            })
            .collect::<Vec<_>>(),
    );
    Ok(StrictSnapshot {
        materialized_fingerprint: materialized_fingerprint.clone(),
        observed_fingerprint: Some(observed_fingerprint),
        manifest_id: materialized_fingerprint,
        entries,
    })
}

pub struct HiddenRefManagerInput<'a> {
    pub stream: &'a StreamIdentity,
    pub commit_oid: &'a str,
    pub generation: u64,
    pub policy_epoch: &'a str,
    pub stream_class: &'a crate::domain::session::StreamClass,
    pub observed_head_oid: &'a Option<String>,
    pub observed_head_ref: &'a Option<String>,
    pub observed_fingerprint: Option<String>,
    pub now: i64,
    pub snapshot: &'a StrictSnapshot,
}

pub fn build_manager_from_hidden_ref(input: HiddenRefManagerInput<'_>) -> ManagerState {
    ManagerState {
        version: 3,
        stream: input.stream.clone(),
        generation: input.generation,
        anchor: AnchorState {
            checkpoint_commit_oid: input.commit_oid.to_string(),
            manifest_id: input.snapshot.manifest_id.clone(),
            materialized_fingerprint: input.snapshot.materialized_fingerprint.clone(),
            observed_fingerprint: input.observed_fingerprint,
            policy_epoch: input.policy_epoch.to_string(),
            stream_class: input.stream_class.clone(),
            observed_head_oid: input.observed_head_oid.clone(),
            observed_head_ref: input.observed_head_ref.clone(),
            materialized_at: input.now,
        },
        pending: None,
        last_seen: Some(Observation {
            materialized_fingerprint: input.snapshot.materialized_fingerprint.clone(),
            observed_fingerprint: input.snapshot.observed_fingerprint.clone(),
            manifest_id: input.snapshot.manifest_id.clone(),
            seen_at: input.now,
            observed_head_oid: input.observed_head_oid.clone(),
        }),
    }
}

fn reconcile_latest_intent(
    stores: &Stores,
    stream: &StreamIdentity,
    now: i64,
    hidden_ref_oid: Option<&str>,
    intents: &[CheckpointIntent],
) -> Result<()> {
    let Some(latest) = intents.last() else {
        return Ok(());
    };
    if latest.phase != IntentPhase::Prepared {
        return Ok(());
    }
    if hidden_ref_oid == Some(latest.checkpoint_commit_oid.as_str()) {
        stores.intents.append(&CheckpointIntent {
            phase: IntentPhase::RefUpdated,
            ts: now,
            ..latest.clone()
        })?;
        stores.journal.append(&JournalEvent::IntentTransition {
            ts: now,
            stream_id: stream.stream_id.clone(),
            intent_id: latest.intent_id.clone(),
            checkpoint_commit_oid: latest.checkpoint_commit_oid.clone(),
            phase: IntentPhase::RefUpdated,
        })?;
        return Ok(());
    }
    stores.intents.append(&CheckpointIntent {
        phase: IntentPhase::Aborted,
        ts: now,
        ..latest.clone()
    })?;
    stores.journal.append(&JournalEvent::IntentTransition {
        ts: now,
        stream_id: stream.stream_id.clone(),
        intent_id: latest.intent_id.clone(),
        checkpoint_commit_oid: latest.checkpoint_commit_oid.clone(),
        phase: IntentPhase::Aborted,
    })?;
    Ok(())
}

fn finalize_latest_intent_if_needed(
    stores: &Stores,
    stream: &StreamIdentity,
    now: i64,
    manager: &ManagerState,
    intents: &[CheckpointIntent],
) -> Result<()> {
    let Some(latest) = intents.last() else {
        return Ok(());
    };
    if latest.checkpoint_commit_oid != manager.anchor.checkpoint_commit_oid {
        return Ok(());
    }
    if matches!(latest.phase, IntentPhase::Finalized | IntentPhase::Aborted) {
        return Ok(());
    }
    if !matches!(
        latest.phase,
        IntentPhase::Prepared | IntentPhase::RefUpdated
    ) {
        return Ok(());
    }

    let finalized = CheckpointIntent {
        phase: IntentPhase::Finalized,
        ts: now,
        ..latest.clone()
    };
    stores.intents.append(&finalized)?;
    stores.journal.append(&JournalEvent::IntentTransition {
        ts: now,
        stream_id: stream.stream_id.clone(),
        intent_id: finalized.intent_id.clone(),
        checkpoint_commit_oid: finalized.checkpoint_commit_oid.clone(),
        phase: IntentPhase::Finalized,
    })?;
    Ok(())
}

fn strict_entry_from_tree(
    git: &dyn GitBackend,
    commit_oid: &str,
    entry: TreeEntry,
) -> Result<StrictEntry> {
    let bytes = git.show_file_at_commit(commit_oid, &entry.path)?;
    let observed_digest = format!("blake3:{}", blake3::hash(&bytes).to_hex());
    Ok(StrictEntry {
        path: entry.path,
        mode: entry.mode,
        observed_digest,
        git_oid: entry.oid,
    })
}

pub fn policy_epoch_changed(manager: &ManagerState, policy_epoch: &PolicyEpoch) -> bool {
    manager.anchor.policy_epoch != policy_epoch.0
}
