#[path = "../support/mod.rs"]
mod support;

use sprocket::domain::ids::compute_stream_identity;
use sprocket::infra::git::GitBackend;
use sprocket::infra::git_cli::GitCli;

use support::assertions::{manager_for_stream, stream_root};
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
fn stream_switch_and_detached_head_are_isolated() {
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
    repo.git(&["checkout", "-b", "feature"]);
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "checkpoint"],
        Some(&payloads::checkpoint(&repo.root, "s1", "t1")),
        &[],
    )
    .assert_success();
    repo.git(&["checkout", "--detach"]);
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s2")),
        &[],
    )
    .assert_success();
    let git = GitCli::discover(&repo.root).unwrap();
    let stream = compute_stream_identity(&repo.root, &git.head_state().unwrap());
    assert!(stream.display_name.starts_with("detached:"));
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
