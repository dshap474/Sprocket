#[path = "../support/mod.rs"]
mod support;

use sprocket::domain::ids::compute_stream_identity;
use sprocket::infra::git::GitBackend;
use sprocket::infra::git_cli::GitCli;

use support::assertions::{head_file, hidden_ref_oid, read_journal, stream_root};
use support::cmd::run;
use support::payloads;
use support::repo::TestRepo;

fn set_policy(repo: &TestRepo, body: &str) {
    std::fs::create_dir_all(repo.root.join(".sprocket")).unwrap();
    std::fs::write(repo.root.join(".sprocket/policy.toml"), body).unwrap();
}

fn promotion_policy() -> String {
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
validators = ["false"]
continue_on_failure = false
"#
    .to_string()
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
fn promotion_modes_are_disabled_by_safety_envelope() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    set_policy(&repo, &promotion_policy());

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
        sprocket::domain::journal::JournalEvent::HookNoop { hook, reason, .. }
            if hook == "session-start" && reason == "checkpoint-mode-unsupported"
    )));
    assert!(hidden_ref_oid(&repo, &stream.hidden_ref).is_none());
}

#[test]
fn populated_real_index_is_preserved_when_promotion_is_requested() {
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

    set_policy(&repo, &promotion_policy());
    repo.write("src/lib.rs", "pub fn a() { println!(\"staged\"); }\n");
    repo.git(&["add", "src/lib.rs"]);
    repo.write("src/lib.rs", "pub fn a() { println!(\"workspace\"); }\n");

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

    assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "src/lib.rs");
    assert_eq!(head_file(&repo, "HEAD:src/lib.rs"), "pub fn a() {}");
}
