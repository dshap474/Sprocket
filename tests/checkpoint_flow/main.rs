#[path = "../support/mod.rs"]
mod support;

use serde_json::Value;
use sprocket::domain::ids::compute_stream_identity;
use sprocket::infra::git::GitBackend;
use sprocket::infra::git_cli::GitCli;

use sprocket::domain::intent::IntentPhase;

use support::assertions::{
    hidden_ref_oid, manager_for_stream, read_intents, read_journal, session_tracker_for_stream,
    stream_root, turn_path,
};
use support::cmd::run;
use support::payloads;
use support::repo::TestRepo;

fn set_policy(repo: &TestRepo, body: &str) {
    std::fs::create_dir_all(repo.root.join(".sprocket")).unwrap();
    std::fs::write(repo.root.join(".sprocket/policy.toml"), body).unwrap();
}

fn stream_manager(repo: &TestRepo) -> sprocket::domain::manager::ManagerState {
    let git = GitCli::discover(&repo.root).unwrap();
    let stream = compute_stream_identity(&repo.root, &git.head_state().unwrap());
    manager_for_stream(&stream_root(repo, &stream.stream_id))
}

fn current_stream(repo: &TestRepo) -> sprocket::domain::session::StreamIdentity {
    let git = GitCli::discover(&repo.root).unwrap();
    compute_stream_identity(&repo.root, &git.head_state().unwrap())
}

fn start_materialization(repo: &TestRepo, session: &str, turn: &str) {
    set_policy(
        repo,
        r#"
version = 2
[owned]
include = ["."]
exclude = [":(exclude).git",":(exclude).sprocket",":(exclude)node_modules",":(exclude)target",":(exclude)dist",":(exclude)build",":(exclude).next",":(exclude)coverage",":(exclude).venv"]
[checkpoint]
mode = "hidden_only"
turn_threshold = 9
file_threshold = 1
age_minutes = 20
default_area = "core"
message_template = "checkpoint({area}): save current work [auto]"
lock_timeout_seconds = 300
"#,
    );
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, session)),
        &[],
    )
    .assert_success();
    repo.write("src/lib.rs", "pub fn a() { println!(\"x\"); }\n");
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "baseline"],
        Some(&payloads::baseline(&repo.root, session, turn)),
        &[],
    )
    .assert_success();
}

fn plan_commit(repo: &TestRepo, session: &str, extra_env: &[(&str, String)]) -> Value {
    let output = run(
        &repo.root,
        &repo.hermetic,
        &["session", "plan-commit", "--session-id", session],
        None,
        extra_env,
    );
    output.assert_success();
    serde_json::from_str(&output.stdout_string()).unwrap()
}

#[test]
fn inherited_dirty_eventually_materializes() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[],
    )
    .assert_success();
    repo.write("src/lib.rs", "pub fn a() { println!(\"x\"); }\n");

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s2")),
        &[],
    )
    .assert_success();
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "baseline"],
        Some(&payloads::baseline(&repo.root, "s2", "t1")),
        &[],
    )
    .assert_success();
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "checkpoint"],
        Some(&payloads::checkpoint(&repo.root, "s2", "t1")),
        &[],
    )
    .assert_success();
    assert!(stream_manager(&repo).pending.is_some());

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "baseline"],
        Some(&payloads::baseline(&repo.root, "s2", "t2")),
        &[],
    )
    .assert_success();
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "checkpoint"],
        Some(&payloads::checkpoint(&repo.root, "s2", "t2")),
        &[],
    )
    .assert_success();

    let manager = stream_manager(&repo);
    assert!(manager.pending.is_none());
    assert_eq!(manager.generation, 2);
}

#[test]
fn turn_local_changes_materialize_by_file_threshold() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    set_policy(
        &repo,
        r#"
version = 2
[owned]
include = ["."]
exclude = [":(exclude).git",":(exclude).sprocket",":(exclude)node_modules",":(exclude)target",":(exclude)dist",":(exclude)build",":(exclude).next",":(exclude)coverage",":(exclude).venv"]
[checkpoint]
mode = "hidden_only"
turn_threshold = 9
file_threshold = 1
age_minutes = 20
default_area = "core"
message_template = "checkpoint({area}): save current work [auto]"
lock_timeout_seconds = 300
"#,
    );

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[],
    )
    .assert_success();
    repo.write("src/lib.rs", "pub fn a() { println!(\"x\"); }\n");
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "baseline"],
        Some(&payloads::baseline(&repo.root, "s1", "t1")),
        &[],
    )
    .assert_success();
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "checkpoint"],
        Some(&payloads::checkpoint(&repo.root, "s1", "t1")),
        &[],
    )
    .assert_success();

    assert_eq!(stream_manager(&repo).generation, 2);
}

#[test]
fn age_threshold_uses_test_clock_override() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    set_policy(
        &repo,
        r#"
version = 2
[owned]
include = ["."]
exclude = [":(exclude).git",":(exclude).sprocket",":(exclude)node_modules",":(exclude)target",":(exclude)dist",":(exclude)build",":(exclude).next",":(exclude)coverage",":(exclude).venv"]
[checkpoint]
mode = "hidden_only"
turn_threshold = 9
file_threshold = 9
age_minutes = 1
default_area = "core"
message_template = "checkpoint({area}): save current work [auto]"
lock_timeout_seconds = 300
"#,
    );

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[("SPROCKET_TEST_NOW", "100".to_string())],
    )
    .assert_success();
    repo.write("src/lib.rs", "pub fn a() { println!(\"x\"); }\n");
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[("SPROCKET_TEST_NOW", "200".to_string())],
    )
    .assert_success();
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "baseline"],
        Some(&payloads::baseline(&repo.root, "s1", "t1")),
        &[("SPROCKET_TEST_NOW", "240".to_string())],
    )
    .assert_success();
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "checkpoint"],
        Some(&payloads::checkpoint(&repo.root, "s1", "t1")),
        &[("SPROCKET_TEST_NOW", "250".to_string())],
    )
    .assert_success();
    assert!(stream_manager(&repo).pending.is_some());
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "baseline"],
        Some(&payloads::baseline(&repo.root, "s1", "t2")),
        &[("SPROCKET_TEST_NOW", "270".to_string())],
    )
    .assert_success();
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "checkpoint"],
        Some(&payloads::checkpoint(&repo.root, "s1", "t2")),
        &[("SPROCKET_TEST_NOW", "270".to_string())],
    )
    .assert_success();
    assert_eq!(stream_manager(&repo).generation, 2);
}

#[test]
fn stream_switch_discards_global_turn_and_detached_head_is_rejected() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[],
    )
    .assert_success();
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "baseline"],
        Some(&payloads::baseline(&repo.root, "s1", "t1")),
        &[],
    )
    .assert_success();
    assert!(turn_path(&repo, "s1", "t1").exists());
    repo.git(&["checkout", "-b", "feature"]);
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "checkpoint"],
        Some(&payloads::checkpoint(&repo.root, "s1", "t1")),
        &[],
    )
    .assert_success();
    assert!(!turn_path(&repo, "s1", "t1").exists());
    repo.git(&["checkout", "--detach"]);
    let detached = run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s2")),
        &[],
    );
    detached.assert_success();
    let git = GitCli::discover(&repo.root).unwrap();
    let stream = compute_stream_identity(&repo.root, &git.head_state().unwrap());
    let journal = support::assertions::read_journal(&stream_root(&repo, &stream.stream_id));
    assert!(stream.display_name.starts_with("detached:"));
    assert!(journal.iter().any(|event| matches!(
        event,
        sprocket::domain::journal::JournalEvent::HookNoop { reason, .. }
            if reason == "detached-head-unsupported"
    )));
}

#[test]
fn long_tracked_paths_are_enumerated_without_tar_parsing() {
    let repo = TestRepo::new();
    let long_name = format!("src/{}/lib.rs", "a".repeat(120));
    repo.write(&long_name, "pub fn a() {}\n");
    repo.commit_all("init");
    set_policy(
        &repo,
        r#"
version = 2
[owned]
include = ["."]
exclude = [":(exclude).git",":(exclude).sprocket",":(exclude)node_modules",":(exclude)target",":(exclude)dist",":(exclude)build",":(exclude).next",":(exclude)coverage",":(exclude).venv"]
[checkpoint]
mode = "hidden_only"
turn_threshold = 9
file_threshold = 1
age_minutes = 20
default_area = "core"
message_template = "checkpoint({area}): save current work [auto]"
lock_timeout_seconds = 300
"#,
    );

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[],
    )
    .assert_success();

    std::fs::remove_file(repo.root.join(&long_name)).unwrap();
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "baseline"],
        Some(&payloads::baseline(&repo.root, "s1", "t1")),
        &[],
    )
    .assert_success();
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "checkpoint"],
        Some(&payloads::checkpoint(&repo.root, "s1", "t1")),
        &[],
    )
    .assert_success();

    assert_eq!(stream_manager(&repo).generation, 2);
}

#[test]
fn linked_worktrees_get_separate_stream_runtime() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "main-session")),
        &[],
    )
    .assert_success();

    let worktree_path = repo.root.parent().unwrap().join("linked");
    repo.worktree_add(&worktree_path, "feature-worktree");
    let worktree_repo = TestRepo::for_existing(worktree_path, repo.hermetic.clone());
    run(
        &worktree_repo.root,
        &worktree_repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(
            &worktree_repo.root,
            "feature-session",
        )),
        &[],
    )
    .assert_success();

    let main_stream = {
        let git = GitCli::discover(&repo.root).unwrap();
        compute_stream_identity(&repo.root, &git.head_state().unwrap())
    };
    let linked_stream = {
        let git = GitCli::discover(&worktree_repo.root).unwrap();
        compute_stream_identity(&worktree_repo.root, &git.head_state().unwrap())
    };

    assert_ne!(main_stream.worktree_id, linked_stream.worktree_id);
    assert!(
        stream_root(&repo, &main_stream.stream_id)
            .join("manager.json")
            .exists()
    );
    assert!(
        worktree_repo
            .git_path("sprocket")
            .join("streams")
            .join(&linked_stream.stream_id)
            .join("manager.json")
            .exists()
    );
}

#[test]
fn policy_epoch_change_creates_new_anchor_epoch_without_stream_change() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.write("README.md", "readme\n");
    repo.commit_all("init");

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[],
    )
    .assert_success();
    let before = stream_manager(&repo);

    set_policy(
        &repo,
        r#"
version = 2
[owned]
include = ["README.md"]
exclude = [":(exclude).git",":(exclude).sprocket"]
[checkpoint]
mode = "hidden_only"
turn_threshold = 2
file_threshold = 4
age_minutes = 20
default_area = "core"
message_template = "checkpoint({area}): save current work [auto]"
lock_timeout_seconds = 300
"#,
    );

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s2")),
        &[],
    )
    .assert_success();

    let after = stream_manager(&repo);
    assert_eq!(before.stream.stream_id, after.stream.stream_id);
    assert_ne!(before.anchor.policy_epoch, after.anchor.policy_epoch);
    assert_eq!(after.generation, before.generation + 1);
}

#[test]
fn fail_after_commit_object_leaves_hidden_ref_and_caches_unchanged() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    start_materialization(&repo, "s1", "t1");

    let stream = current_stream(&repo);
    let before = stream_manager(&repo);
    let output = run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "checkpoint"],
        Some(&payloads::checkpoint(&repo.root, "s1", "t1")),
        &[("SPROCKET_FAIL_AT", "after_commit_object".to_string())],
    );
    assert!(!output.output.status.success());

    let after = stream_manager(&repo);
    assert_eq!(before.generation, after.generation);
    assert_eq!(
        hidden_ref_oid(&repo, &stream.hidden_ref),
        Some(before.anchor.checkpoint_commit_oid)
    );
    let intents = read_intents(&stream_root(&repo, &stream.stream_id));
    assert_eq!(
        intents
            .iter()
            .filter(|intent| intent.phase == IntentPhase::Finalized)
            .count(),
        1
    );
}

#[test]
fn fail_after_prepared_is_recovered_as_aborted() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    start_materialization(&repo, "s1", "t1");

    let stream = current_stream(&repo);
    let before = stream_manager(&repo);
    let output = run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "checkpoint"],
        Some(&payloads::checkpoint(&repo.root, "s1", "t1")),
        &[("SPROCKET_FAIL_AT", "after_prepared".to_string())],
    );
    assert!(!output.output.status.success());

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s2")),
        &[],
    )
    .assert_success();

    let after = stream_manager(&repo);
    assert_eq!(after.generation, before.generation);
    let intents = read_intents(&stream_root(&repo, &stream.stream_id));
    assert_eq!(intents.last().unwrap().phase, IntentPhase::Aborted);
}

#[test]
fn fail_after_hidden_ref_cas_recovers_and_is_idempotent() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    start_materialization(&repo, "s1", "t1");

    let stream = current_stream(&repo);
    let output = run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "checkpoint"],
        Some(&payloads::checkpoint(&repo.root, "s1", "t1")),
        &[("SPROCKET_FAIL_AT", "after_hidden_ref_cas".to_string())],
    );
    assert!(!output.output.status.success());
    std::fs::remove_file(stream_root(&repo, &stream.stream_id).join("manager.json")).unwrap();

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s2")),
        &[],
    )
    .assert_success();

    let recovered = stream_manager(&repo);
    let tip = hidden_ref_oid(&repo, &stream.hidden_ref).unwrap();
    assert_eq!(recovered.anchor.checkpoint_commit_oid, tip);
    assert_eq!(recovered.generation, 2);

    let first_pass = read_intents(&stream_root(&repo, &stream.stream_id));
    let recovered_phases: Vec<_> = first_pass
        .iter()
        .filter(|intent| intent.checkpoint_commit_oid == tip)
        .map(|intent| intent.phase)
        .collect();
    assert!(recovered_phases.contains(&IntentPhase::Prepared));
    assert!(recovered_phases.contains(&IntentPhase::RefUpdated));
    assert!(recovered_phases.contains(&IntentPhase::Finalized));

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s3")),
        &[],
    )
    .assert_success();
    let second_pass = read_intents(&stream_root(&repo, &stream.stream_id));
    assert_eq!(first_pass.len(), second_pass.len());
}

#[test]
fn fail_after_cache_save_recovers_by_only_appending_finalize() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    start_materialization(&repo, "s1", "t1");

    let stream = current_stream(&repo);
    let output = run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "checkpoint"],
        Some(&payloads::checkpoint(&repo.root, "s1", "t1")),
        &[("SPROCKET_FAIL_AT", "after_cache_save".to_string())],
    );
    assert!(!output.output.status.success());

    let before = read_intents(&stream_root(&repo, &stream.stream_id));
    let tip = hidden_ref_oid(&repo, &stream.hidden_ref).unwrap();
    let before_finalize_count = before
        .iter()
        .filter(|intent| {
            intent.checkpoint_commit_oid == tip && intent.phase == IntentPhase::Finalized
        })
        .count();
    assert_eq!(before_finalize_count, 0);

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s2")),
        &[],
    )
    .assert_success();

    let after = read_intents(&stream_root(&repo, &stream.stream_id));
    let after_finalize_count = after
        .iter()
        .filter(|intent| {
            intent.checkpoint_commit_oid == tip && intent.phase == IntentPhase::Finalized
        })
        .count();
    assert_eq!(after_finalize_count, 1);
    assert_eq!(stream_manager(&repo).generation, 2);
}

#[test]
fn fail_after_turn_delete_keeps_anchor_valid() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    start_materialization(&repo, "s1", "t1");

    let stream = current_stream(&repo);
    let output = run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "checkpoint"],
        Some(&payloads::checkpoint(&repo.root, "s1", "t1")),
        &[("SPROCKET_FAIL_AT", "after_turn_delete".to_string())],
    );
    assert!(!output.output.status.success());
    assert!(!turn_path(&repo, "s1", "t1").exists());

    let tip = hidden_ref_oid(&repo, &stream.hidden_ref).unwrap();
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s2")),
        &[],
    )
    .assert_success();
    let manager = stream_manager(&repo);
    assert_eq!(manager.anchor.checkpoint_commit_oid, tip);
    assert_eq!(manager.generation, 2);
}

#[test]
fn cherry_pick_state_is_rejected_early() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    std::fs::write(repo.git_path("CHERRY_PICK_HEAD"), b"deadbeef\n").unwrap();

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[],
    )
    .assert_success();

    let stream = current_stream(&repo);
    let journal = read_journal(&stream_root(&repo, &stream.stream_id));
    assert!(journal.iter().any(|event| matches!(
        event,
        sprocket::domain::journal::JournalEvent::HookNoop { reason, .. }
            if reason == "cherry-pick-in-progress"
    )));
}

#[test]
fn session_start_creates_tracker_and_checkpoint_marks_exclusive_paths() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[],
    )
    .assert_success();

    let stream = current_stream(&repo);
    let tracker = session_tracker_for_stream(&stream_root(&repo, &stream.stream_id), "s1");
    assert_eq!(tracker.epoch, 1);
    assert!(tracker.touched_paths.is_empty());

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "baseline"],
        Some(&payloads::baseline(&repo.root, "s1", "t1")),
        &[],
    )
    .assert_success();
    repo.write("src/lib.rs", "pub fn a() { println!(\"x\"); }\n");
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "checkpoint"],
        Some(&payloads::checkpoint(&repo.root, "s1", "t1")),
        &[],
    )
    .assert_success();

    let tracker = session_tracker_for_stream(&stream_root(&repo, &stream.stream_id), "s1");
    let path = sprocket::domain::repopath::RepoPath::from("src/lib.rs");
    assert_eq!(tracker.turn_count_since_reset, 1);
    assert_eq!(
        tracker.touched_paths.get(&path).unwrap().claim_state,
        sprocket::domain::session_tracker::PathClaimState::Exclusive
    );

    let plan = plan_commit(&repo, "s1", &[]);
    assert_eq!(plan["safe_to_commit"], serde_json::json!(false));
    assert_eq!(plan["thresholds_met"], serde_json::json!(false));
    assert_eq!(plan["eligible_paths"], serde_json::json!(["src/lib.rs"]));
}

#[test]
fn contended_paths_block_session_plan() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");

    for session in ["s1", "s2"] {
        run(
            &repo.root,
            &repo.hermetic,
            &["hook", "codex", "session-start"],
            Some(&payloads::session_start(&repo.root, session)),
            &[("SPROCKET_TEST_NOW", "100".to_string())],
        )
        .assert_success();
    }

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "baseline"],
        Some(&payloads::baseline(&repo.root, "s1", "t1")),
        &[("SPROCKET_TEST_NOW", "110".to_string())],
    )
    .assert_success();
    repo.write("src/lib.rs", "pub fn a() { println!(\"one\"); }\n");
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "checkpoint"],
        Some(&payloads::checkpoint(&repo.root, "s1", "t1")),
        &[("SPROCKET_TEST_NOW", "120".to_string())],
    )
    .assert_success();

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "baseline"],
        Some(&payloads::baseline(&repo.root, "s2", "t2")),
        &[("SPROCKET_TEST_NOW", "130".to_string())],
    )
    .assert_success();
    repo.write("src/lib.rs", "pub fn a() { println!(\"two\"); }\n");
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "checkpoint"],
        Some(&payloads::checkpoint(&repo.root, "s2", "t2")),
        &[("SPROCKET_TEST_NOW", "140".to_string())],
    )
    .assert_success();

    let plan = plan_commit(&repo, "s2", &[("SPROCKET_TEST_NOW", "150".to_string())]);
    assert_eq!(plan["safe_to_commit"], serde_json::json!(false));
    assert!(
        plan["blocking_reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason == "path_contended")
    );
    assert_eq!(plan["blocked_paths"][0]["path"], "src/lib.rs");
}

#[test]
fn restored_paths_drop_out_of_the_final_candidate_set() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[],
    )
    .assert_success();
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "baseline"],
        Some(&payloads::baseline(&repo.root, "s1", "t1")),
        &[],
    )
    .assert_success();
    repo.write("src/lib.rs", "pub fn a() { println!(\"x\"); }\n");
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "checkpoint"],
        Some(&payloads::checkpoint(&repo.root, "s1", "t1")),
        &[],
    )
    .assert_success();

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "baseline"],
        Some(&payloads::baseline(&repo.root, "s1", "t2")),
        &[],
    )
    .assert_success();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "checkpoint"],
        Some(&payloads::checkpoint(&repo.root, "s1", "t2")),
        &[],
    )
    .assert_success();

    let stream = current_stream(&repo);
    let tracker = session_tracker_for_stream(&stream_root(&repo, &stream.stream_id), "s1");
    assert!(tracker.touched_paths.is_empty());
    let plan = plan_commit(&repo, "s1", &[]);
    assert_eq!(plan["eligible_paths"], serde_json::json!([]));
}
