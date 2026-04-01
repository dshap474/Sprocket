#[path = "../support/mod.rs"]
mod support;

use serde_json::json;
use sprocket::domain::ids::compute_stream_identity;
use sprocket::infra::git::GitBackend;
use sprocket::infra::git_cli::GitCli;
use sprocket::infra::lock::RepoLock;
use std::process::Command;

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
fn gitattributes_repo_is_rejected_early() {
    let repo = TestRepo::new();
    repo.write(".gitattributes", "* text=auto\n");
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

    let git = GitCli::discover(&repo.root).unwrap();
    let stream = compute_stream_identity(&repo.root, &git.head_state().unwrap());
    let journal = read_journal(&stream_root(&repo, &stream.stream_id));
    assert!(journal.iter().any(|event| matches!(
        event,
        sprocket::domain::journal::JournalEvent::HookNoop { reason, .. }
            if reason == "gitattributes-unsupported"
    )));
    assert!(hidden_ref_oid(&repo, &stream.hidden_ref).is_none());
}

#[test]
fn gitlinks_are_rejected_early() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");

    let submodule = TestRepo::new();
    submodule.write("README.md", "submodule\n");
    submodule.commit_all("init");
    let output = Command::new("git")
        .args([
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            submodule.root.to_str().unwrap(),
            "vendor/submodule",
        ])
        .current_dir(&repo.root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "failed to add submodule\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "add submodule"]);

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
    assert!(journal.iter().any(|event| matches!(
        event,
        sprocket::domain::journal::JournalEvent::HookNoop { reason, .. }
            if reason == "gitlinks-unsupported"
    )));
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
fn lock_contention_is_journaled_and_noops() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    std::fs::create_dir_all(runtime_root(&repo)).unwrap();
    let _lock = RepoLock::try_acquire(&runtime_root(&repo).join("checkpoint.lock"))
        .unwrap()
        .unwrap();

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
    assert!(journal.iter().any(|event| matches!(
        event,
        sprocket::domain::journal::JournalEvent::HookNoop { reason, .. }
            if reason == "lock-busy"
    )));
    assert!(
        !stream_root(&repo, &stream.stream_id)
            .join("manager.json")
            .exists()
    );
}

#[test]
fn existing_hidden_ref_recovers_missing_manager() {
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

    let git = GitCli::discover(&repo.root).unwrap();
    let stream = compute_stream_identity(&repo.root, &git.head_state().unwrap());
    let stream_root = stream_root(&repo, &stream.stream_id);
    let hidden_oid = hidden_ref_oid(&repo, &stream.hidden_ref).unwrap();
    std::fs::remove_file(stream_root.join("manager.json")).unwrap();

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s2")),
        &[],
    )
    .assert_success();

    let manager = manager_for_stream(&stream_root);
    let journal = read_journal(&stream_root);
    assert_eq!(manager.anchor.checkpoint_commit_oid, hidden_oid);
    assert!(journal.iter().any(|event| matches!(
        event,
        sprocket::domain::journal::JournalEvent::Recovery { reason, .. }
            if reason == "rebuilt-caches-from-hidden-ref"
    )));
}

#[test]
fn missing_anchor_manifest_is_recovered_from_hidden_commit() {
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

    let git = GitCli::discover(&repo.root).unwrap();
    let stream = compute_stream_identity(&repo.root, &git.head_state().unwrap());
    let stream_root = stream_root(&repo, &stream.stream_id);
    let manager = manager_for_stream(&stream_root);
    let manifest_path = stream_root
        .join("manifests")
        .join(format!("{}.json.zst", manager.anchor.manifest_id));
    std::fs::remove_file(&manifest_path).unwrap();

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s2")),
        &[],
    )
    .assert_success();

    let repaired = manager_for_stream(&stream_root);
    let journal = read_journal(&stream_root);
    assert!(
        stream_root
            .join("manifests")
            .join(format!("{}.json.zst", repaired.anchor.manifest_id))
            .exists()
    );
    assert!(journal.iter().any(|event| matches!(
        event,
        sprocket::domain::journal::JournalEvent::Recovery { reason, .. }
            if reason == "rebuilt-caches-from-hidden-ref"
    )));
}

#[test]
fn hidden_checkpoint_does_not_require_git_identity_config() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    repo.git(&["config", "--unset", "user.name"]);
    repo.git(&["config", "--unset", "user.email"]);

    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[],
    )
    .assert_success();
}
