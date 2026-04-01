use anyhow::Result;
use serde_json::Value;
use uuid::Uuid;

use crate::app::support::{
    RepoSupportContext, ensure_supported_repo, journal_lock_busy, load_policy,
};
use crate::cli::resolve_repo_from_target;
use crate::codex::payload::{cwd, session_id, turn_id};
use crate::domain::decision::{Decision, classify};
use crate::domain::intent::{CheckpointIntent, IntentPhase};
use crate::domain::journal::JournalEvent;
use crate::domain::manager::{AnchorState, reconcile_pending};
use crate::domain::session::Observation;
use crate::engine::init_stream::{ensure_stream_initialized, resolve_stream};
use crate::engine::materialize_hidden::{
    CheckpointMessageContext, build_checkpoint_message, prepare_hidden_checkpoint,
};
use crate::engine::observe::capture_strict_snapshot;
use crate::engine::repair::snapshot_from_commit;
use crate::infra::clock::{Clock, SystemClock};
use crate::infra::failpoint::maybe_fail;
use crate::infra::git::GitBackend;
use crate::infra::git_cli::GitCli;
use crate::infra::lock::RepoLock;
use crate::infra::store::{RuntimeLayout, Stores};

pub fn run(payload: &Value) -> Result<()> {
    let clock = SystemClock;
    let now = clock.now_unix();
    let cwd = cwd(payload).unwrap_or(std::env::current_dir()?);
    let repo = resolve_repo_from_target(Some(&cwd))?;
    let git = GitCli::discover(&repo)?;
    let (head, stream) = resolve_stream(&git)?;
    let runtime = RuntimeLayout::from_git(&git)?;
    let stores = Stores::for_stream(runtime, &stream.stream_id);
    let policy = load_policy(&git, &stores, &stream, "checkpoint", now)?;
    let repo_state = git.repo_state()?;
    if !ensure_supported_repo(RepoSupportContext {
        git: &git,
        stores: &stores,
        stream: &stream,
        hook: "checkpoint",
        now,
        head: &head,
        repo_state: &repo_state,
        policy: &policy,
    })? {
        return Ok(());
    }

    let _lock = match RepoLock::try_acquire(&stores.lock_path)? {
        Some(lock) => lock,
        None => {
            journal_lock_busy(&stores, &stream, "checkpoint", now)?;
            return Ok(());
        }
    };

    let mut manager = ensure_stream_initialized(&git, &stores, &stream, &head, now, &policy)?;
    let Some(turn) = stores.turns.load(&session_id(payload), &turn_id(payload))? else {
        return Ok(());
    };

    if turn.stream_id_at_start != stream.stream_id {
        stores.turns.delete(&turn.session_id, &turn.turn_id)?;
        stores.journal.append(&JournalEvent::StopDecision {
            ts: now,
            session_id: turn.session_id,
            turn_id: turn.turn_id,
            stream_id: stream.stream_id,
            outcome: "stream-changed".to_string(),
            source: None,
            commit_oid: None,
        })?;
        return Ok(());
    }
    if turn.policy_epoch_at_start != manager.anchor.policy_epoch {
        stores.turns.delete(&turn.session_id, &turn.turn_id)?;
        stores.journal.append(&JournalEvent::StopDecision {
            ts: now,
            session_id: turn.session_id,
            turn_id: turn.turn_id,
            stream_id: stream.stream_id,
            outcome: "policy-epoch-changed".to_string(),
            source: None,
            commit_oid: None,
        })?;
        return Ok(());
    }

    let snapshot = capture_strict_snapshot(git.repo_root(), &git, &policy)?;
    stores.manifests.put(&snapshot.manifest_id, &snapshot)?;
    let anchor_snapshot = match stores
        .manifests
        .get::<crate::domain::manifest::StrictSnapshot>(&manager.anchor.manifest_id)
    {
        Ok(snapshot) => snapshot,
        Err(_) => {
            let recovered =
                snapshot_from_commit(&git, &manager.anchor.checkpoint_commit_oid, &policy)?;
            stores.manifests.put(&recovered.manifest_id, &recovered)?;
            manager.anchor.manifest_id = recovered.manifest_id.clone();
            manager.anchor.materialized_fingerprint = recovered.materialized_fingerprint.clone();
            manager.anchor.observed_fingerprint = recovered.observed_fingerprint.clone();
            stores.manager.save(&manager)?;
            stores.journal.append(&JournalEvent::Recovery {
                ts: now,
                stream_id: stream.stream_id.clone(),
                reason: "recovered-anchor-manifest-from-hidden-ref".to_string(),
                commit_oid: Some(manager.anchor.checkpoint_commit_oid.clone()),
            })?;
            recovered
        }
    };
    let changed_paths =
        crate::domain::delta::changed_path_count(&anchor_snapshot.entries, &snapshot.entries)
            as u32;

    let decision = classify(&crate::domain::decision::ClassifyInput {
        stream_id_now: &stream.stream_id,
        stream_id_at_start: &turn.stream_id_at_start,
        now_unix: now,
        anchor_fingerprint: &manager.anchor.materialized_fingerprint,
        turn_baseline_fingerprint: &turn.baseline_materialized_fingerprint,
        anchor_fingerprint_at_start: &turn.anchor_materialized_fingerprint_at_start,
        current_fingerprint: &snapshot.materialized_fingerprint,
        global_changed_paths: changed_paths,
        pending_turn_count: manager
            .pending
            .as_ref()
            .map(|pending| pending.pending_turn_count)
            .unwrap_or(0),
        pending_first_seen_at: manager
            .pending
            .as_ref()
            .map(|pending| pending.first_seen_at),
        turn_threshold: policy.checkpoint.turn_threshold,
        file_threshold: policy.checkpoint.file_threshold,
        age_seconds: (policy.checkpoint.age_minutes as i64) * 60,
    });

    let (outcome, source, commit_oid) = match decision {
        Decision::Noop(reason) => {
            manager.pending = if reason == crate::domain::decision::NoopReason::MatchesAnchor {
                None
            } else {
                manager.pending
            };
            manager.last_seen = Some(last_seen(&snapshot, now, &head));
            stores.manager.save(&manager)?;
            stores.turns.delete(&turn.session_id, &turn.turn_id)?;
            let outcome = match reason {
                crate::domain::decision::NoopReason::MatchesAnchor => "matches-anchor",
                crate::domain::decision::NoopReason::MissingTurn => "missing-turn",
                crate::domain::decision::NoopReason::StreamChanged => "stream-changed",
            };
            (outcome.to_string(), None, None)
        }
        Decision::RecordPending { source, .. } => {
            manager.pending = Some(reconcile_pending(
                manager.pending.take(),
                &turn.session_id,
                source,
                now,
                &snapshot,
            ));
            manager.last_seen = Some(last_seen(&snapshot, now, &head));
            stores.manager.save(&manager)?;
            stores.turns.delete(&turn.session_id, &turn.turn_id)?;
            ("pending".to_string(), Some(source), None)
        }
        Decision::Materialize { source, .. } => {
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
                subject: &policy.checkpoint_subject(),
                generation: manager.generation + 1,
                source,
                snapshot: &snapshot,
                head: &head,
                stream: &stream,
                policy_epoch: &policy.policy_epoch(),
            });
            let prepared = prepare_hidden_checkpoint(
                &git,
                head.oid.as_deref(),
                Some(&manager.anchor.checkpoint_commit_oid),
                &head_owned_paths,
                &snapshot,
                &message,
            )?;
            maybe_fail("after_commit_object")?;
            let intent = CheckpointIntent {
                version: 1,
                ts: now,
                intent_id: Uuid::new_v4().to_string(),
                stream_id: stream.stream_id.clone(),
                hidden_ref: stream.hidden_ref.clone(),
                checkpoint_commit_oid: prepared.commit_oid.clone(),
                previous_hidden_oid: Some(manager.anchor.checkpoint_commit_oid.clone()),
                manifest_id: snapshot.manifest_id.clone(),
                materialized_fingerprint: snapshot.materialized_fingerprint.clone(),
                observed_fingerprint: snapshot.observed_fingerprint.clone(),
                policy_epoch: manager.anchor.policy_epoch.clone(),
                stream_class: stream.class.clone(),
                phase: IntentPhase::Prepared,
            };
            stores.intents.append(&intent)?;
            stores.journal.append(&JournalEvent::IntentTransition {
                ts: now,
                stream_id: stream.stream_id.clone(),
                intent_id: intent.intent_id.clone(),
                checkpoint_commit_oid: prepared.commit_oid.clone(),
                phase: IntentPhase::Prepared,
            })?;
            maybe_fail("after_prepared")?;
            git.update_ref_cas(
                &stream.hidden_ref,
                &prepared.commit_oid,
                Some(&manager.anchor.checkpoint_commit_oid),
            )?;
            maybe_fail("after_hidden_ref_cas")?;
            stores.intents.append(&CheckpointIntent {
                phase: IntentPhase::RefUpdated,
                ..intent.clone()
            })?;
            stores.journal.append(&JournalEvent::IntentTransition {
                ts: now,
                stream_id: stream.stream_id.clone(),
                intent_id: intent.intent_id.clone(),
                checkpoint_commit_oid: prepared.commit_oid.clone(),
                phase: IntentPhase::RefUpdated,
            })?;
            maybe_fail("after_ref_updated")?;

            manager.generation += 1;
            manager.anchor = AnchorState {
                checkpoint_commit_oid: prepared.commit_oid.clone(),
                manifest_id: snapshot.manifest_id.clone(),
                materialized_fingerprint: snapshot.materialized_fingerprint.clone(),
                observed_fingerprint: snapshot.observed_fingerprint.clone(),
                policy_epoch: manager.anchor.policy_epoch.clone(),
                stream_class: stream.class.clone(),
                observed_head_oid: head.oid.clone(),
                observed_head_ref: head.symref.clone(),
                materialized_at: now,
            };
            manager.pending = None;
            manager.last_seen = Some(last_seen(&snapshot, now, &head));
            stores.manifests.put(&snapshot.manifest_id, &snapshot)?;
            stores.manager.save(&manager)?;
            maybe_fail("after_cache_save")?;
            stores.intents.append(&CheckpointIntent {
                phase: IntentPhase::Finalized,
                ..intent.clone()
            })?;
            stores.journal.append(&JournalEvent::IntentTransition {
                ts: now,
                stream_id: stream.stream_id.clone(),
                intent_id: intent.intent_id,
                checkpoint_commit_oid: prepared.commit_oid.clone(),
                phase: IntentPhase::Finalized,
            })?;
            maybe_fail("after_finalized")?;
            stores.turns.delete(&turn.session_id, &turn.turn_id)?;
            maybe_fail("after_turn_delete")?;
            (
                "materialized".to_string(),
                Some(source),
                Some(prepared.commit_oid),
            )
        }
    };

    stores.journal.append(&JournalEvent::StopDecision {
        ts: now,
        session_id: turn.session_id,
        turn_id: turn.turn_id,
        stream_id: stream.stream_id,
        outcome,
        source,
        commit_oid,
    })?;
    Ok(())
}

fn last_seen(
    snapshot: &crate::domain::manifest::StrictSnapshot,
    now: i64,
    head: &crate::domain::session::HeadState,
) -> Observation {
    Observation {
        materialized_fingerprint: snapshot.materialized_fingerprint.clone(),
        observed_fingerprint: snapshot.observed_fingerprint.clone(),
        manifest_id: snapshot.manifest_id.clone(),
        seen_at: now,
        observed_head_oid: head.oid.clone(),
    }
}
