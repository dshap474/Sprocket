use serde_json::json;

use crate::codex::hooks_json::{group_contains_marker, merge_hooks_json};
use crate::domain::decision::{ClassifyInput, Decision, classify};
use crate::domain::delta::changed_paths;
use crate::domain::ids::compute_stream_identity;
use crate::domain::manager::{PendingSource, merge_pending_source, reconcile_pending};
use crate::domain::manifest::StrictSnapshot;
use crate::domain::session_tracker::{CommitBlockReason, SessionThresholdInput, thresholds_met};
use crate::engine::classify::NoopReason;

#[test]
fn compute_stream_identity_changes_between_branch_and_detached() {
    let repo = std::path::Path::new("/tmp/worktree");
    let attached = crate::domain::session::HeadState {
        oid: Some("abc123".to_string()),
        symref: Some("refs/heads/main".to_string()),
        detached: false,
    };
    let detached = crate::domain::session::HeadState {
        oid: Some("abc123".to_string()),
        symref: None,
        detached: true,
    };

    let attached_stream = compute_stream_identity(repo, &attached);
    let detached_stream = compute_stream_identity(repo, &detached);

    assert_ne!(attached_stream.stream_id, detached_stream.stream_id);
    assert_eq!(attached_stream.display_name, "refs/heads/main");
    assert!(detached_stream.display_name.starts_with("detached:"));
}

#[test]
fn classify_inherited_dirty_materializes_after_threshold() {
    let decision = classify(&ClassifyInput {
        stream_id_now: "stream",
        stream_id_at_start: "stream",
        now_unix: 300,
        anchor_fingerprint: "anchor",
        turn_baseline_fingerprint: "dirty",
        anchor_fingerprint_at_start: "anchor",
        current_fingerprint: "dirty",
        global_changed_paths: 1,
        pending_turn_count: 1,
        pending_first_seen_at: Some(100),
        turn_threshold: 2,
        file_threshold: 9,
        age_seconds: 999,
    });
    assert!(matches!(
        decision,
        Decision::Materialize {
            source: PendingSource::Inherited,
            ..
        }
    ));
}

#[test]
fn classify_stream_change_noops() {
    let decision = classify(&ClassifyInput {
        stream_id_now: "stream-b",
        stream_id_at_start: "stream-a",
        now_unix: 300,
        anchor_fingerprint: "anchor",
        turn_baseline_fingerprint: "dirty",
        anchor_fingerprint_at_start: "anchor",
        current_fingerprint: "dirty",
        global_changed_paths: 1,
        pending_turn_count: 0,
        pending_first_seen_at: None,
        turn_threshold: 2,
        file_threshold: 9,
        age_seconds: 999,
    });
    assert!(matches!(
        decision,
        Decision::Noop(NoopReason::StreamChanged)
    ));
}

#[test]
fn reconcile_pending_merges_sources_and_sessions() {
    let snapshot = StrictSnapshot {
        materialized_fingerprint: "blake3:1".into(),
        observed_fingerprint: Some("blake3:o1".into()),
        manifest_id: "blake3:1".into(),
        entries: Vec::new(),
    };
    let pending = reconcile_pending(None, "s1", PendingSource::TurnLocal, 10, &snapshot);
    let merged = reconcile_pending(Some(pending), "s2", PendingSource::Inherited, 20, &snapshot);
    assert_eq!(merged.pending_turn_count, 2);
    assert_eq!(merged.source, PendingSource::Mixed);
    assert_eq!(
        merge_pending_source(PendingSource::External, PendingSource::External),
        PendingSource::External
    );
    assert_eq!(
        merged.touched_sessions,
        vec!["s1".to_string(), "s2".to_string()]
    );
}

#[test]
fn hooks_merge_replaces_only_sprocket_groups() {
    let existing = json!({
        "hooks": {
            "Stop": [
                {"hooks":[{"command":"other"}]},
                {"hooks":[{"command":"bin --sprocket-managed hook codex checkpoint"}]}
            ]
        }
    });
    let merged = merge_hooks_json(
        Some(existing),
        &[(
            "Stop".to_string(),
            json!({"hooks":[{"command":"new --sprocket-managed"}]}),
        )],
        "--sprocket-managed",
    )
    .unwrap();
    let stop = merged["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop.len(), 2);
    assert!(group_contains_marker(&stop[1], "--sprocket-managed"));
    assert_eq!(stop[0]["hooks"][0]["command"], "other");
}

#[test]
fn prettool_guard_blocks_commit_but_not_status() {
    assert!(crate::app::pre_tool_use::should_block_git_command(Some(
        "git commit -m hi"
    )));
    assert!(crate::app::pre_tool_use::should_block_git_command(Some(
        "git reset --hard HEAD"
    )));
    assert!(!crate::app::pre_tool_use::should_block_git_command(Some(
        "git status --short"
    )));
    assert!(crate::app::pre_tool_use::should_block_git_command(Some(
        "git checkout feature"
    )));
}

#[test]
fn changed_paths_returns_real_path_set() {
    let old = vec![crate::domain::manifest::StrictEntry {
        path: "src/lib.rs".into(),
        mode: 0o100644,
        observed_digest: "blake3:a".into(),
        git_oid: "oid-a".into(),
    }];
    let new = vec![
        crate::domain::manifest::StrictEntry {
            path: "src/lib.rs".into(),
            mode: 0o100644,
            observed_digest: "blake3:b".into(),
            git_oid: "oid-b".into(),
        },
        crate::domain::manifest::StrictEntry {
            path: "README.md".into(),
            mode: 0o100644,
            observed_digest: "blake3:c".into(),
            git_oid: "oid-c".into(),
        },
    ];
    let changed = changed_paths(&old, &new);
    assert!(changed.contains(&crate::domain::repopath::RepoPath::from("src/lib.rs")));
    assert!(changed.contains(&crate::domain::repopath::RepoPath::from("README.md")));
}

#[test]
fn session_thresholds_require_turns_files_or_age() {
    assert!(!thresholds_met(&SessionThresholdInput {
        turn_count_since_reset: 1,
        exclusive_path_count: 1,
        first_dirty_at: Some(100),
        now_unix: 120,
        turn_threshold: 2,
        file_threshold: 4,
        age_seconds: 60,
    }));
    assert!(thresholds_met(&SessionThresholdInput {
        turn_count_since_reset: 2,
        exclusive_path_count: 1,
        first_dirty_at: Some(100),
        now_unix: 120,
        turn_threshold: 2,
        file_threshold: 4,
        age_seconds: 60,
    }));
    assert!(matches!(
        CommitBlockReason::HeadMoved,
        CommitBlockReason::HeadMoved
    ));
}
