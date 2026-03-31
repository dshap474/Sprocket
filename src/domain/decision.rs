use serde::{Deserialize, Serialize};

use crate::domain::manager::PendingSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoopReason {
    MatchesAnchor,
    MissingTurn,
    StreamChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    Noop(NoopReason),
    RecordPending {
        source: PendingSource,
        changed_paths: u32,
    },
    Materialize {
        source: PendingSource,
        changed_paths: u32,
    },
}

pub struct ClassifyInput<'a> {
    pub stream_id_now: &'a str,
    pub stream_id_at_start: &'a str,
    pub now_unix: i64,
    pub anchor_fingerprint: &'a str,
    pub turn_baseline_fingerprint: &'a str,
    pub anchor_fingerprint_at_start: &'a str,
    pub current_fingerprint: &'a str,
    pub global_changed_paths: u32,
    pub pending_turn_count: u32,
    pub pending_first_seen_at: Option<i64>,
    pub turn_threshold: u32,
    pub file_threshold: u32,
    pub age_seconds: i64,
}

pub fn classify(input: &ClassifyInput<'_>) -> Decision {
    if input.stream_id_now != input.stream_id_at_start {
        return Decision::Noop(NoopReason::StreamChanged);
    }
    if input.current_fingerprint == input.anchor_fingerprint {
        return Decision::Noop(NoopReason::MatchesAnchor);
    }

    let changed_this_turn = input.current_fingerprint != input.turn_baseline_fingerprint;
    let dirty_existed_at_turn_start =
        input.turn_baseline_fingerprint != input.anchor_fingerprint_at_start;

    let source = match (dirty_existed_at_turn_start, changed_this_turn) {
        (false, true) => PendingSource::TurnLocal,
        (true, false) => PendingSource::Inherited,
        (true, true) => PendingSource::Mixed,
        (false, false) => PendingSource::External,
    };

    let turns = input.pending_turn_count.saturating_add(1);
    let dirty_age = input
        .pending_first_seen_at
        .map(|first_seen_at| input.now_unix.saturating_sub(first_seen_at))
        .unwrap_or(0);

    let should_materialize = turns >= input.turn_threshold
        || input.global_changed_paths >= input.file_threshold
        || dirty_age >= input.age_seconds;

    if should_materialize {
        Decision::Materialize {
            source,
            changed_paths: input.global_changed_paths,
        }
    } else {
        Decision::RecordPending {
            source,
            changed_paths: input.global_changed_paths,
        }
    }
}
