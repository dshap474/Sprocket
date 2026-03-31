use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

use crate::app::support::{ensure_supported_repo, load_policy};
use crate::cli::resolve_repo_from_target;
use crate::codex::payload::{cwd, session_id, turn_id};
use crate::codex::responses::emit_stop_block;
use crate::domain::decision::{Decision, NoopReason};
use crate::domain::errors::SprocketError;
use crate::domain::journal::JournalEvent;
use crate::domain::manager::reconcile_pending;
use crate::domain::policy::Policy;
use crate::domain::session::Observation;
use crate::engine::classify::{ClassifyInput, classify};
use crate::engine::init_stream::{ensure_stream_initialized, resolve_stream};
use crate::engine::materialize_hidden::{build_checkpoint_message, materialize_hidden_checkpoint};
use crate::engine::observe::capture_strict_snapshot;
use crate::engine::promote_visible::{PromotionOutcome, maybe_promote_visible};
use crate::infra::clock::{Clock, SystemClock};
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
    if !ensure_supported_repo(&stores, &stream, "checkpoint", now, &repo_state, &policy)? {
        return Ok(());
    }

    let _lock = match RepoLock::try_acquire(
        &stores.lock_path,
        Duration::from_secs(policy.checkpoint.lock_timeout_seconds),
    )? {
        Some(lock) => lock,
        None => return Ok(()),
    };

    let mut manager = ensure_stream_initialized(&git, &stores, &stream, &head, now, &policy)?;
    let Some(mut turn) = stores.turns.load(&session_id(payload), &turn_id(payload))? else {
        return Ok(());
    };
    if turn.stream_id_at_start != stream.stream_id {
        stores.turns.delete(&turn.session_id, &turn.turn_id)?;
        return Ok(());
    }

    let snapshot = capture_strict_snapshot(git.repo_root(), &git, &policy)?;
    stores.manifests.put(&snapshot.manifest_id, &snapshot)?;
    let anchor_snapshot = stores
        .manifests
        .get::<crate::domain::manifest::StrictSnapshot>(&manager.anchor.manifest_id)
        .map_err(|_| SprocketError::MissingAnchorManifest(manager.anchor.manifest_id.clone()))?;
    let changed_paths =
        crate::domain::delta::changed_path_count(&anchor_snapshot.entries, &snapshot.entries)
            as u32;

    if let Some(hidden_commit_oid) = turn.pending_promotion_commit_oid.clone() {
        if hidden_commit_oid == manager.anchor.checkpoint_commit_oid
            && snapshot.fingerprint == manager.anchor.fingerprint
        {
            return finish_promotion_attempt(PromotionAttemptContext {
                git: &git,
                stores: &stores,
                policy: &policy,
                repo_state: &repo_state,
                head: &head,
                stream: &stream,
                turn: &turn,
                now,
                hidden_commit_oid: &hidden_commit_oid,
            });
        }

        turn.pending_promotion_commit_oid = None;
        stores.turns.save(&turn)?;
    }

    let decision = classify(&ClassifyInput {
        stream_id_now: &stream.stream_id,
        stream_id_at_start: &turn.stream_id_at_start,
        now_unix: now,
        anchor_fingerprint: &manager.anchor.fingerprint,
        turn_baseline_fingerprint: &turn.baseline_fingerprint,
        anchor_fingerprint_at_start: &turn.anchor_fingerprint_at_start,
        current_fingerprint: &snapshot.fingerprint,
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
            manager.pending = if reason == NoopReason::MatchesAnchor {
                None
            } else {
                manager.pending
            };
            manager.last_seen = Some(last_seen(&snapshot, now, &head));
            stores.manager.save(&manager)?;
            stores.turns.delete(&turn.session_id, &turn.turn_id)?;
            let outcome = match reason {
                NoopReason::MatchesAnchor => "matches-anchor",
                NoopReason::MissingTurn => "missing-turn",
                NoopReason::StreamChanged => "stream-changed",
            };
            (outcome.to_string(), None, None)
        }
        Decision::RecordPending {
            source,
            changed_paths: _,
        } => {
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
        Decision::Materialize {
            source,
            changed_paths: _,
        } => {
            let message = build_checkpoint_message(
                &policy,
                source,
                &snapshot,
                manager.generation + 1,
                &head,
                &stream,
            );
            let commit_oid = materialize_hidden_checkpoint(
                &git,
                head.oid.as_deref(),
                &stream.hidden_ref,
                Some(&manager.anchor.checkpoint_commit_oid),
                &snapshot,
                &policy,
                &message,
            )?;
            manager.generation += 1;
            manager.anchor = crate::domain::manager::AnchorState {
                checkpoint_commit_oid: commit_oid.clone(),
                manifest_id: snapshot.manifest_id.clone(),
                fingerprint: snapshot.fingerprint.clone(),
                observed_head_oid: head.oid.clone(),
                observed_head_ref: head.symref.clone(),
                materialized_at: now,
            };
            manager.pending = None;
            manager.last_seen = Some(last_seen(&snapshot, now, &head));
            stores.manager.save(&manager)?;
            turn.pending_promotion_commit_oid = Some(commit_oid.clone());
            stores.turns.save(&turn)?;

            match maybe_promote_visible(&git, &policy, &repo_state, &head, &commit_oid, &stream)? {
                PromotionOutcome::Skipped(reason) => {
                    stores.turns.delete(&turn.session_id, &turn.turn_id)?;
                    stores.journal.append(&JournalEvent::PromotionSkipped {
                        ts: now,
                        stream_id: stream.stream_id.clone(),
                        reason,
                    })?;
                    ("materialized".to_string(), Some(source), Some(commit_oid))
                }
                PromotionOutcome::Blocked(reason) => {
                    emit_stop_block(&reason)?;
                    (
                        "materialized-blocked".to_string(),
                        Some(source),
                        Some(commit_oid),
                    )
                }
                PromotionOutcome::Promoted(_) => {
                    stores.turns.delete(&turn.session_id, &turn.turn_id)?;
                    ("materialized".to_string(), Some(source), Some(commit_oid))
                }
            }
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

struct PromotionAttemptContext<'a> {
    git: &'a dyn GitBackend,
    stores: &'a Stores,
    policy: &'a Policy,
    repo_state: &'a crate::domain::session::RepoState,
    head: &'a crate::domain::session::HeadState,
    stream: &'a crate::domain::session::StreamIdentity,
    turn: &'a crate::domain::turn::TurnState,
    now: i64,
    hidden_commit_oid: &'a str,
}

fn finish_promotion_attempt(ctx: PromotionAttemptContext<'_>) -> Result<()> {
    let outcome = match maybe_promote_visible(
        ctx.git,
        ctx.policy,
        ctx.repo_state,
        ctx.head,
        ctx.hidden_commit_oid,
        ctx.stream,
    )? {
        PromotionOutcome::Skipped(reason) => {
            ctx.stores
                .turns
                .delete(&ctx.turn.session_id, &ctx.turn.turn_id)?;
            ctx.stores.journal.append(&JournalEvent::PromotionSkipped {
                ts: ctx.now,
                stream_id: ctx.stream.stream_id.clone(),
                reason,
            })?;
            "materialized".to_string()
        }
        PromotionOutcome::Blocked(reason) => {
            emit_stop_block(&reason)?;
            "materialized-blocked".to_string()
        }
        PromotionOutcome::Promoted(_) => {
            ctx.stores
                .turns
                .delete(&ctx.turn.session_id, &ctx.turn.turn_id)?;
            "materialized".to_string()
        }
    };

    ctx.stores.journal.append(&JournalEvent::StopDecision {
        ts: ctx.now,
        session_id: ctx.turn.session_id.clone(),
        turn_id: ctx.turn.turn_id.clone(),
        stream_id: ctx.stream.stream_id.clone(),
        outcome,
        source: None,
        commit_oid: Some(ctx.hidden_commit_oid.to_string()),
    })?;
    Ok(())
}

fn last_seen(
    snapshot: &crate::domain::manifest::StrictSnapshot,
    now: i64,
    head: &crate::domain::session::HeadState,
) -> Observation {
    Observation {
        fingerprint: snapshot.fingerprint.clone(),
        manifest_id: snapshot.manifest_id.clone(),
        seen_at: now,
        observed_head_oid: head.oid.clone(),
    }
}
