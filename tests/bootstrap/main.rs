#[path = "../support/mod.rs"]
mod support;

use serde_json::json;
use sprocket::domain::ids::compute_stream_identity;
use sprocket::infra::git::GitBackend;
use sprocket::infra::git_cli::GitCli;

use support::assertions::{
    hidden_ref_oid, manager_for_stream, read_journal, runtime_root, stream_root,
};
use support::cmd::run;
use support::payloads;
use support::repo::TestRepo;

#[test]
fn install_creates_policy_hooks_and_runtime() {
    let repo = TestRepo::new();
    repo.write("src/main.rs", "fn main() {}\n");
    repo.commit_all("init");

    let output = run(
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
    );
    output.assert_success();

    assert!(repo.root.join(".sprocket/policy.toml").exists());
    assert!(repo.root.join(".codex/hooks.json").exists());
    assert!(runtime_root(&repo).join("local.toml").exists());
    assert!(repo.git_path("hooks").join("prepare-commit-msg").exists());
}

#[test]
fn first_session_start_creates_bootstrap_hidden_anchor() {
    let repo = TestRepo::new();
    repo.write("src/main.rs", "fn main() {}\n");
    repo.commit_all("init");

    let output = run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[],
    );
    output.assert_success();

    let git = GitCli::discover(&repo.root).unwrap();
    let head = git.head_state().unwrap();
    let stream = compute_stream_identity(&repo.root, &head);
    let stream_root = stream_root(&repo, &stream.stream_id);
    let manager = manager_for_stream(&stream_root);
    let hidden_oid = hidden_ref_oid(&repo, &stream.hidden_ref).unwrap();
    assert_eq!(manager.generation, 1);
    assert_eq!(hidden_oid, manager.anchor.checkpoint_commit_oid);
}

#[test]
fn dirty_first_install_does_not_open_pending() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    repo.write("src/lib.rs", "pub fn a() { println!(\"x\"); }\n");

    let output = run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[],
    );
    output.assert_success();

    let git = GitCli::discover(&repo.root).unwrap();
    let stream = compute_stream_identity(&repo.root, &git.head_state().unwrap());
    let manager = manager_for_stream(&stream_root(&repo, &stream.stream_id));
    assert!(manager.pending.is_none());
    assert_eq!(manager.generation, 1);
}

#[test]
fn sparse_checkout_noops_before_mutation() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    repo.git(&["config", "core.sparseCheckout", "true"]);

    let output = run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[],
    );
    output.assert_success();

    let git = GitCli::discover(&repo.root).unwrap();
    let stream = compute_stream_identity(&repo.root, &git.head_state().unwrap());
    assert!(
        !stream_root(&repo, &stream.stream_id)
            .join("manager.json")
            .exists()
    );
    assert!(hidden_ref_oid(&repo, &stream.hidden_ref).is_none());
}

#[test]
fn install_preserves_unrelated_hooks_and_replaces_managed_groups() {
    let repo = TestRepo::new();
    repo.write("src/main.rs", "fn main() {}\n");
    repo.commit_all("init");
    std::fs::create_dir_all(repo.root.join(".codex")).unwrap();
    std::fs::write(
        repo.root.join(".codex/hooks.json"),
        serde_json::to_string_pretty(&json!({
            "hooks": {
                "Stop": [
                    {"hooks": [{"command": "other stop"}]},
                    {"hooks": [{"command": "old --sprocket-managed hook codex checkpoint"}]}
                ],
                "PreToolUse": [
                    {"matcher": "Edit", "hooks": [{"command": "other pretool"}]}
                ]
            }
        }))
        .unwrap(),
    )
    .unwrap();

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

    let hooks: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo.root.join(".codex/hooks.json")).unwrap(),
    )
    .unwrap();
    let stop = hooks["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop.len(), 2);
    assert_eq!(stop[0]["hooks"][0]["command"], "other stop");
    assert!(
        stop[1]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("--sprocket-managed hook codex checkpoint")
    );
    let pretool = hooks["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(pretool.len(), 2);
    assert_eq!(pretool[0]["matcher"], "Edit");
}

#[test]
fn install_respects_core_hookspath() {
    let repo = TestRepo::new();
    repo.write("src/main.rs", "fn main() {}\n");
    repo.commit_all("init");
    repo.git(&["config", "core.hooksPath", ".githooks-custom"]);

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

    assert!(
        repo.root
            .join(".githooks-custom/prepare-commit-msg")
            .exists()
    );
}

#[test]
fn session_start_bootstraps_unborn_head_repo() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[],
    )
    .assert_success();

    let git = GitCli::discover(&repo.root).unwrap();
    let head = git.head_state().unwrap();
    let stream = compute_stream_identity(&repo.root, &head);
    let stream_root = stream_root(&repo, &stream.stream_id);
    let manager = manager_for_stream(&stream_root);
    let hidden_oid = hidden_ref_oid(&repo, &stream.hidden_ref).unwrap();
    assert_eq!(manager.anchor.checkpoint_commit_oid, hidden_oid);
    assert!(manager.anchor.observed_head_oid.is_none());
}

#[test]
fn session_start_supports_sha256_repos_when_git_supports_them() {
    let Some(repo) = TestRepo::try_new_with_init_args(&["init", "--object-format=sha256"]) else {
        return;
    };

    repo.write("src/lib.rs", "pub fn a() {}\n");
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[],
    )
    .assert_success();

    let git = GitCli::discover(&repo.root).unwrap();
    let stream = compute_stream_identity(&repo.root, &git.head_state().unwrap());
    let manager = manager_for_stream(&stream_root(&repo, &stream.stream_id));
    assert_eq!(manager.anchor.checkpoint_commit_oid.len(), 64);
}

#[test]
fn stale_lock_is_reaped_and_journaled_flow_continues() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    std::fs::create_dir_all(repo.root.join(".sprocket")).unwrap();
    std::fs::write(
        repo.root.join(".sprocket/policy.toml"),
        r#"
version = 2
[owned]
include = ["."]
exclude = [":(exclude).git",":(exclude).sprocket",":(exclude)node_modules",":(exclude)target",":(exclude)dist",":(exclude)build",":(exclude).next",":(exclude)coverage",":(exclude).venv"]
[checkpoint]
mode = "hidden_only"
turn_threshold = 2
file_threshold = 4
age_minutes = 20
default_area = "core"
message_template = "checkpoint({area}): save current work [auto]"
lock_timeout_seconds = 0
"#,
    )
    .unwrap();
    std::fs::create_dir_all(runtime_root(&repo)).unwrap();
    std::fs::write(runtime_root(&repo).join("checkpoint.lock"), b"stale\n").unwrap();

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[],
    )
    .assert_success();

    let git = GitCli::discover(&repo.root).unwrap();
    let stream = compute_stream_identity(&repo.root, &git.head_state().unwrap());
    let journal = read_journal(&stream_root(&repo, &stream.stream_id));
    assert!(
        stream_root(&repo, &stream.stream_id)
            .join("manager.json")
            .exists()
    );
    assert!(journal.iter().any(|event| matches!(
        event,
        sprocket::domain::journal::JournalEvent::SessionStart { .. }
    )));
}
