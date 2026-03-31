#[path = "../support/mod.rs"]
mod support;

use sprocket::domain::ids::compute_stream_identity;
use sprocket::infra::git::GitBackend;
use sprocket::infra::git_cli::GitCli;

use support::assertions::{hidden_ref_oid, manager_for_stream, runtime_root, stream_root};
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
