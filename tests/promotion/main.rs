#[path = "../support/mod.rs"]
mod support;

use sprocket::domain::ids::compute_stream_identity;
use sprocket::infra::git::GitBackend;
use sprocket::infra::git_cli::GitCli;

use support::assertions::{head_file, manager_for_stream, stream_root};
use support::cmd::run;
use support::payloads;
use support::repo::TestRepo;

fn set_policy(repo: &TestRepo, body: &str) {
    std::fs::create_dir_all(repo.root.join(".sprocket")).unwrap();
    std::fs::write(repo.root.join(".sprocket/policy.toml"), body).unwrap();
}

fn stream_state(
    repo: &TestRepo,
) -> (
    sprocket::domain::session::StreamIdentity,
    sprocket::domain::manager::ManagerState,
) {
    let git = GitCli::discover(&repo.root).unwrap();
    let stream = compute_stream_identity(&repo.root, &git.head_state().unwrap());
    let manager = manager_for_stream(&stream_root(repo, &stream.stream_id));
    (stream, manager)
}

fn promotion_policy(validators: &[&str], continue_on_failure: bool) -> String {
    format!(
        r#"
version = 2
[owned]
include = ["."]
exclude = [":(exclude).git",":(exclude).sprocket",":(exclude)node_modules",":(exclude)target",":(exclude)dist",":(exclude)build",":(exclude).next",":(exclude)coverage",":(exclude).venv"]
[checkpoint]
mode = "hidden_then_promote"
turn_threshold = 2
file_threshold = 1
age_minutes = 20
default_area = "core"
message_template = "checkpoint({{area}}): save current work [auto]"
lock_timeout_seconds = 300
[promotion]
enabled = true
validators = [{}]
continue_on_failure = {}
"#,
        validators
            .iter()
            .map(|validator| format!("{validator:?}"))
            .collect::<Vec<_>>()
            .join(", "),
        continue_on_failure
    )
}

#[test]
fn hidden_only_mode_does_not_promote() {
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

    assert_eq!(head_file(&repo, "HEAD:src/lib.rs"), "pub fn a() {}");
}

#[test]
fn hidden_then_promote_success_advances_visible_history() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    set_policy(&repo, &promotion_policy(&[], true));

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

    assert_eq!(
        head_file(&repo, "HEAD:src/lib.rs"),
        "pub fn a() { println!(\"x\"); }"
    );
}

#[test]
fn promotion_skip_keeps_hidden_checkpoint_but_not_visible_history() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    set_policy(&repo, &promotion_policy(&[], true));

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[],
    )
    .assert_success();
    repo.write("src/lib.rs", "pub fn a() { println!(\"x\"); }\n");
    repo.write("node_modules/pkg/index.js", "foreign\n");
    repo.git(&["add", "node_modules/pkg/index.js"]);
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

    let (_, manager) = stream_state(&repo);
    assert_eq!(head_file(&repo, "HEAD:src/lib.rs"), "pub fn a() {}");
    assert_eq!(manager.generation, 2);
}

#[test]
fn blocked_promotion_preserves_turn_and_resumes() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    set_policy(&repo, &promotion_policy(&["false"], false));

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
    let blocked = run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "checkpoint"],
        Some(&payloads::checkpoint(&repo.root, "s1", "t1")),
        &[],
    );
    blocked.assert_success();
    assert!(blocked.stdout_string().contains("\"decision\":\"block\""));

    let (stream, _) = stream_state(&repo);
    let turn_path = stream_root(&repo, &stream.stream_id)
        .join("turns")
        .join("k-czE")
        .join("k-dDE.json");
    assert!(turn_path.exists());

    set_policy(&repo, &promotion_policy(&[], true));
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "checkpoint"],
        Some(&payloads::checkpoint(&repo.root, "s1", "t1")),
        &[],
    )
    .assert_success();

    assert_eq!(
        head_file(&repo, "HEAD:src/lib.rs"),
        "pub fn a() { println!(\"x\"); }"
    );
}

#[test]
fn index_sync_failure_does_not_advance_head() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    set_policy(&repo, &promotion_policy(&[], true));

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[],
    )
    .assert_success();
    repo.write("src/lib.rs", "pub fn a() { println!(\"x\"); }\n");
    std::fs::write(repo.root.join(".git/index.lock"), b"lock\n").unwrap();
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

    assert_eq!(head_file(&repo, "HEAD:src/lib.rs"), "pub fn a() {}");
}
