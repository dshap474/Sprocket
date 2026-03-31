#[path = "../support/mod.rs"]
mod support;

use support::cmd::run;
use support::payloads;
use support::repo::TestRepo;

#[test]
#[ignore = "slow-e2e"]
fn subprocess_only_install_and_hook_smoke_flow() {
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
}
