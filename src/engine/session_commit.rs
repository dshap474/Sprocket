use std::collections::BTreeMap;

use anyhow::Result;

use crate::domain::delta::{changed_paths, entries_by_path};
use crate::domain::ids::snapshot_fingerprint;
use crate::domain::manifest::{StrictEntry, StrictSnapshot};
use crate::domain::repopath::RepoPath;
use crate::domain::session::{HeadState, StreamIdentity};
use crate::domain::session_tracker::{
    BlockedPath, CommitBlockReason, CommitPlan, PathClaimState, SessionThresholdInput,
    SessionTracker, SessionTrackerStatus, TrackedPathState, entry_oid, thresholds_met,
};
use crate::engine::repair::snapshot_from_commit;
use crate::infra::git::GitBackend;
use crate::infra::store::Stores;

pub const ACTIVE_SESSION_TTL_SECONDS: i64 = 2 * 60 * 60;

pub struct BuildCommitPlanInput<'a> {
    pub git: &'a dyn GitBackend,
    pub stores: &'a Stores,
    pub stream: &'a StreamIdentity,
    pub head: &'a HeadState,
    pub current_snapshot: &'a StrictSnapshot,
    pub tracker: Option<&'a SessionTracker>,
    pub now: i64,
    pub turn_threshold: u32,
    pub file_threshold: u32,
    pub age_seconds: i64,
}

pub fn stable_session_id(value: &str) -> bool {
    value != "session-current"
}

pub fn tracker_head_snapshot(
    git: &dyn GitBackend,
    stores: &Stores,
    head_oid: Option<&str>,
    policy: &crate::domain::policy::Policy,
) -> Result<StrictSnapshot> {
    let snapshot = if let Some(oid) = head_oid {
        snapshot_from_commit(git, oid, policy)?
    } else {
        empty_snapshot()
    };
    stores.manifests.put(&snapshot.manifest_id, &snapshot)?;
    Ok(snapshot)
}

pub fn create_or_refresh_tracker(
    stores: &Stores,
    session_id: Option<&str>,
    stream: &StreamIdentity,
    head: &HeadState,
    head_snapshot: &StrictSnapshot,
    current_snapshot: &StrictSnapshot,
    _now: i64,
) -> Result<Option<SessionTracker>> {
    let Some(session_id) = session_id.filter(|id| stable_session_id(id)) else {
        return Ok(None);
    };
    let mut tracker = stores
        .session_trackers
        .load(session_id)?
        .unwrap_or_else(|| SessionTracker {
            version: crate::domain::session_tracker::SESSION_TRACKER_VERSION,
            session_id: session_id.to_string(),
            stream_id: stream.stream_id.clone(),
            epoch: 1,
            status: SessionTrackerStatus::Active,
            start_head_oid: head.oid.clone(),
            start_head_manifest_id: head_snapshot.manifest_id.clone(),
            epoch_start_worktree_manifest_id: current_snapshot.manifest_id.clone(),
            last_seen_manifest_id: current_snapshot.manifest_id.clone(),
            first_dirty_at: None,
            last_dirty_at: None,
            turn_count_since_reset: 0,
            touched_paths: BTreeMap::new(),
        });
    tracker.stream_id = stream.stream_id.clone();
    tracker.last_seen_manifest_id = current_snapshot.manifest_id.clone();
    if tracker.touched_paths.is_empty() {
        tracker.status = SessionTrackerStatus::Active;
    }
    stores.session_trackers.save(&tracker)?;
    Ok(Some(tracker))
}

pub fn refresh_active_sessions(
    stores: &Stores,
    manager: &mut crate::domain::manager::ManagerState,
    current_session_id: Option<&str>,
    now: i64,
) -> Result<()> {
    let mut active = active_session_ids(stores, now)?;
    if let Some(session_id) = current_session_id.filter(|id| stable_session_id(id))
        && !active.iter().any(|current| current == session_id)
    {
        active.push(session_id.to_string());
        active.sort();
        active.dedup();
    }
    manager.active_sessions = active;
    Ok(())
}

pub fn update_tracker_from_turn(
    stores: &Stores,
    tracker: &mut SessionTracker,
    turn: &crate::domain::turn::TurnState,
    current_snapshot: &StrictSnapshot,
    now: i64,
) -> Result<()> {
    let baseline_snapshot = stores
        .manifests
        .get::<StrictSnapshot>(&turn.baseline_manifest_id)?;
    let start_head_snapshot = stores
        .manifests
        .get::<StrictSnapshot>(&tracker.start_head_manifest_id)?;
    let epoch_start_snapshot = stores
        .manifests
        .get::<StrictSnapshot>(&tracker.epoch_start_worktree_manifest_id)?;

    let baseline_map = entries_by_path(&baseline_snapshot.entries);
    let current_map = entries_by_path(&current_snapshot.entries);
    let start_head_map = entries_by_path(&start_head_snapshot.entries);
    let epoch_start_map = entries_by_path(&epoch_start_snapshot.entries);
    let other_trackers = active_trackers(stores, now, Some(&tracker.session_id))?;
    let changed = changed_paths(&baseline_snapshot.entries, &current_snapshot.entries);

    if !changed.is_empty() {
        tracker.turn_count_since_reset = tracker.turn_count_since_reset.saturating_add(1);
        tracker.first_dirty_at.get_or_insert(now);
        tracker.last_dirty_at = Some(now);
    }
    tracker.last_seen_manifest_id = current_snapshot.manifest_id.clone();

    for path in changed {
        let current_entry = current_map.get(&path);
        let baseline_entry = baseline_map.get(&path);
        let start_head_entry = start_head_map.get(&path);
        let epoch_start_entry = epoch_start_map.get(&path);

        if same_entry(current_entry, epoch_start_entry) {
            tracker.touched_paths.remove(&path);
            continue;
        }

        let other_sessions = sessions_touching_path(&other_trackers, &path);
        let claim_state = if !same_entry(epoch_start_entry, start_head_entry) {
            PathClaimState::InheritedDirty
        } else if !other_sessions.is_empty() {
            PathClaimState::Contended
        } else if tracker
            .touched_paths
            .get(&path)
            .is_some_and(|existing| existing.current_oid != entry_oid(baseline_entry))
        {
            PathClaimState::ExternalInterference
        } else {
            PathClaimState::Exclusive
        };

        let tracked = tracker.touched_paths.remove(&path);
        tracker.touched_paths.insert(
            path,
            TrackedPathState {
                first_touched_at: tracked
                    .as_ref()
                    .map(|current| current.first_touched_at)
                    .unwrap_or(now),
                last_touched_at: now,
                first_turn_id: tracked
                    .as_ref()
                    .map(|current| current.first_turn_id.clone())
                    .unwrap_or_else(|| turn.turn_id.clone()),
                last_turn_id: turn.turn_id.clone(),
                claim_state,
                start_head_oid: entry_oid(start_head_entry),
                start_worktree_oid: entry_oid(epoch_start_entry),
                current_oid: entry_oid(current_entry),
                claimed_by_session: tracker.session_id.clone(),
                other_sessions,
            },
        );
    }

    tracker.status = if tracker
        .touched_paths
        .values()
        .all(|state| state.claim_state == PathClaimState::Exclusive)
    {
        SessionTrackerStatus::Active
    } else {
        SessionTrackerStatus::Blocked
    };
    stores.session_trackers.save(tracker)?;
    Ok(())
}

pub fn build_commit_plan(input: BuildCommitPlanInput<'_>) -> Result<CommitPlan> {
    let Some(tracker) = input.tracker else {
        return Ok(CommitPlan {
            version: 1,
            session_id: String::new(),
            stream_id: input.stream.stream_id.clone(),
            epoch: 0,
            thresholds_met: false,
            safe_to_commit: false,
            eligible_paths: Vec::new(),
            blocked_paths: Vec::new(),
            blocking_reasons: vec![CommitBlockReason::MissingTracker],
        });
    };

    let epoch_start_snapshot = input
        .stores
        .manifests
        .get::<StrictSnapshot>(&tracker.epoch_start_worktree_manifest_id)?;
    let epoch_start_map = entries_by_path(&epoch_start_snapshot.entries);
    let current_map = entries_by_path(&input.current_snapshot.entries);
    let active_trackers = active_trackers(input.stores, input.now, Some(&tracker.session_id))?;

    let exclusive_count = tracker
        .touched_paths
        .values()
        .filter(|state| state.claim_state == PathClaimState::Exclusive)
        .count() as u32;
    let threshold_input = SessionThresholdInput {
        turn_count_since_reset: tracker.turn_count_since_reset,
        exclusive_path_count: exclusive_count,
        first_dirty_at: tracker.first_dirty_at,
        now_unix: input.now,
        turn_threshold: input.turn_threshold,
        file_threshold: input.file_threshold,
        age_seconds: input.age_seconds,
    };
    let thresholds_met = thresholds_met(&threshold_input);

    let mut blocking_reasons = Vec::new();
    if !stable_session_id(&tracker.session_id) {
        blocking_reasons.push(CommitBlockReason::UnstableSessionId);
    }
    if input.head.oid != tracker.start_head_oid {
        blocking_reasons.push(CommitBlockReason::HeadMoved);
    }
    if !input.git.staged_paths()?.is_empty() {
        blocking_reasons.push(CommitBlockReason::StagedChangesPresent);
    }
    if !thresholds_met {
        blocking_reasons.push(CommitBlockReason::ThresholdsNotMet);
    }

    let mut eligible_paths = Vec::new();
    let mut blocked_paths = Vec::new();
    for (path, state) in &tracker.touched_paths {
        if same_entry(current_map.get(path), epoch_start_map.get(path)) {
            continue;
        }

        let mut reasons = Vec::new();
        match state.claim_state {
            PathClaimState::Exclusive => {}
            PathClaimState::InheritedDirty => reasons.push(CommitBlockReason::PathInheritedDirty),
            PathClaimState::Contended => reasons.push(CommitBlockReason::PathContended),
            PathClaimState::ExternalInterference => {
                reasons.push(CommitBlockReason::ExternalInterference)
            }
        }
        if sessions_touching_path(&active_trackers, path).len() > state.other_sessions.len() {
            reasons.push(CommitBlockReason::PathContended);
        }
        if entry_oid(current_map.get(path)) != state.current_oid {
            reasons.push(CommitBlockReason::ExternalInterference);
        }
        reasons.sort();
        reasons.dedup();

        if reasons.is_empty() {
            eligible_paths.push(path.clone());
        } else {
            blocked_paths.push(BlockedPath {
                path: path.clone(),
                reasons: reasons.clone(),
            });
            blocking_reasons.extend(reasons);
        }
    }

    blocking_reasons.sort();
    blocking_reasons.dedup();
    let safe_to_commit =
        thresholds_met && blocking_reasons.is_empty() && !eligible_paths.is_empty();

    Ok(CommitPlan {
        version: 1,
        session_id: tracker.session_id.clone(),
        stream_id: tracker.stream_id.clone(),
        epoch: tracker.epoch,
        thresholds_met,
        safe_to_commit,
        eligible_paths,
        blocked_paths,
        blocking_reasons,
    })
}

pub fn active_session_ids(stores: &Stores, now: i64) -> Result<Vec<String>> {
    let mut active = Vec::new();
    for tracker in stores.session_trackers.list_all()? {
        if let Some(session) = stores.sessions.load(&tracker.session_id)?
            && session.last_seen_at >= now.saturating_sub(ACTIVE_SESSION_TTL_SECONDS)
        {
            active.push(tracker.session_id);
        }
    }
    active.sort();
    active.dedup();
    Ok(active)
}

fn active_trackers(
    stores: &Stores,
    now: i64,
    exclude_session_id: Option<&str>,
) -> Result<Vec<SessionTracker>> {
    let mut trackers = Vec::new();
    for tracker in stores.session_trackers.list_all()? {
        if exclude_session_id.is_some_and(|excluded| excluded == tracker.session_id) {
            continue;
        }
        if let Some(session) = stores.sessions.load(&tracker.session_id)?
            && session.last_seen_at >= now.saturating_sub(ACTIVE_SESSION_TTL_SECONDS)
        {
            trackers.push(tracker);
        }
    }
    Ok(trackers)
}

fn sessions_touching_path(trackers: &[SessionTracker], path: &RepoPath) -> Vec<String> {
    let mut session_ids = trackers
        .iter()
        .filter(|tracker| tracker.touched_paths.contains_key(path))
        .map(|tracker| tracker.session_id.clone())
        .collect::<Vec<_>>();
    session_ids.sort();
    session_ids.dedup();
    session_ids
}

fn empty_snapshot() -> StrictSnapshot {
    let fingerprint = snapshot_fingerprint(&[]);
    StrictSnapshot {
        materialized_fingerprint: fingerprint.clone(),
        observed_fingerprint: Some(fingerprint.clone()),
        manifest_id: fingerprint,
        entries: Vec::new(),
    }
}

fn same_entry(left: Option<&StrictEntry>, right: Option<&StrictEntry>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => left.mode == right.mode && left.git_oid == right.git_oid,
        _ => false,
    }
}
