#[path = "../support/mod.rs"]
mod support;

use sprocket::domain::ids::compute_stream_identity;
use sprocket::infra::git::GitBackend;
use sprocket::infra::git_cli::GitCli;

use support::assertions::{decode_runtime_key, read_journal, runtime_root, stream_root};
use support::cmd::run;
use support::payloads;
use support::repo::TestRepo;

#[test]
fn malformed_policy_fails_closed() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    std::fs::create_dir_all(repo.root.join(".sprocket")).unwrap();
    std::fs::write(repo.root.join(".sprocket/policy.toml"), "version = [\n").unwrap();

    let output = run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[],
    );
    assert!(!output.output.status.success());
    assert!(output.stderr_string().contains("failed to load"));

    let git = GitCli::discover(&repo.root).unwrap();
    let stream = compute_stream_identity(&repo.root, &git.head_state().unwrap());
    let journal = read_journal(&stream_root(&repo, &stream.stream_id));
    assert!(journal.iter().any(|event| matches!(
        event,
        sprocket::domain::journal::JournalEvent::HookError { hook, reason, .. }
            if hook == "session-start" && reason.contains("invalid-policy")
    )));
}

#[test]
fn runtime_keys_are_encoded() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "../session id")),
        &[],
    )
    .assert_success();
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "baseline"],
        Some(&payloads::baseline(
            &repo.root,
            "../session id",
            "turn/../$ weird",
        )),
        &[],
    )
    .assert_success();

    let session_dir = runtime_root(&repo)
        .join("turns")
        .read_dir()
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let session_key = session_dir
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let turn_key = session_dir
        .read_dir()
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .to_string();

    assert_eq!(decode_runtime_key(&session_key), "../session id");
    assert_eq!(decode_runtime_key(&turn_key), "turn/../$ weird");
}

#[test]
fn prepare_commit_msg_guard_and_pretool_guard_work() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");

    run(
        &repo.root,
        &repo.hermetic,
        &[
            "install",
            "codex",
            "--target-repo",
            repo.root.to_str().unwrap(),
        ],
        None,
        &[],
    )
    .assert_success();

    repo.write("src/lib.rs", "pub fn a() { println!(\"x\"); }\n");
    repo.git(&["add", "."]);
    let commit = std::process::Command::new("git")
        .args(["commit", "-m", "manual"])
        .current_dir(&repo.root)
        .output()
        .unwrap();
    assert!(!commit.status.success());
    assert!(String::from_utf8_lossy(&commit.stderr).contains("Commits are owned by Sprocket"));

    let pretool = run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "pre-tool-use"],
        Some(&payloads::pre_tool_use(&repo.root, "git commit -m hi")),
        &[],
    );
    pretool.assert_success();
    assert!(
        pretool
            .stdout_string()
            .contains("\"permissionDecision\":\"deny\"")
    );
}

#[test]
fn journal_records_noop_error_and_skip_paths() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");

    repo.git(&["config", "core.sparseCheckout", "true"]);
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[],
    )
    .assert_success();
    repo.git(&["config", "core.sparseCheckout", "false"]);
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[],
    )
    .assert_success();
    repo.write("src/lib.rs", "pub fn a() { println!(\"x\"); }\n");
    std::fs::create_dir_all(repo.root.join(".sprocket")).unwrap();
    std::fs::write(
        repo.root.join(".sprocket/policy.toml"),
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
message_template = "checkpoint({area}): save current work [auto]"
lock_timeout_seconds = 300
[promotion]
enabled = true
validators = []
continue_on_failure = true
"#,
    )
    .unwrap();
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

    let git = GitCli::discover(&repo.root).unwrap();
    let stream = compute_stream_identity(&repo.root, &git.head_state().unwrap());
    let journal = read_journal(&stream_root(&repo, &stream.stream_id));
    assert!(journal.iter().any(|event| matches!(
        event,
        sprocket::domain::journal::JournalEvent::HookNoop { hook, reason, .. }
            if hook == "session-start" && reason == "sparse-checkout-unsupported"
    )));
    assert!(journal.iter().any(|event| matches!(
        event,
        sprocket::domain::journal::JournalEvent::HookNoop { hook, reason, .. }
            if hook == "baseline" && reason == "checkpoint-mode-unsupported"
    )));
}

#[test]
fn payload_variants_are_accepted() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");

    let session_payload = serde_json::json!({
        "workingDirectory": repo.root.display().to_string(),
        "sessionId": "camel-session",
    });
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&session_payload),
        &[],
    )
    .assert_success();

    let baseline_payload = serde_json::json!({
        "working_dir": repo.root.display().to_string(),
        "sessionId": "camel-session",
        "requestId": "camel-turn",
    });
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "baseline"],
        Some(&baseline_payload),
        &[],
    )
    .assert_success();

    let pretool_payload = serde_json::json!({
        "workingDirectory": repo.root.display().to_string(),
        "toolInput": {
            "command": "git reset --hard HEAD"
        }
    });
    let pretool = run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "pre-tool-use"],
        Some(&pretool_payload),
        &[],
    );
    pretool.assert_success();
    assert!(
        pretool
            .stdout_string()
            .contains("\"permissionDecision\":\"deny\"")
    );

    let checkpoint_payload = serde_json::json!({
        "working_dir": repo.root.display().to_string(),
        "sessionId": "camel-session",
        "requestId": "camel-turn",
    });
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "checkpoint"],
        Some(&checkpoint_payload),
        &[],
    )
    .assert_success();
}
