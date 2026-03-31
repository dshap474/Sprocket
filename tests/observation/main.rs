#[path = "../support/mod.rs"]
mod support;

#[cfg(not(target_os = "macos"))]
use std::ffi::OsString;

use sprocket::domain::ids::compute_stream_identity;
use sprocket::domain::repopath::RepoPath;
use sprocket::infra::git::GitBackend;
use sprocket::infra::git_cli::GitCli;

use support::assertions::{manager_for_stream, manifest_for_stream, stream_root};
use support::cmd::run;
use support::payloads;
use support::repo::TestRepo;

fn set_file_threshold_one(repo: &TestRepo) {
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
turn_threshold = 9
file_threshold = 1
age_minutes = 20
default_area = "core"
message_template = "checkpoint({area}): save current work [auto]"
lock_timeout_seconds = 300
"#,
    )
    .unwrap();
}

fn bootstrap(repo: &TestRepo) {
    run(
        &repo.root,
        &repo.hermetic,
        &["hook", "codex", "session-start"],
        Some(&payloads::session_start(&repo.root, "s1")),
        &[],
    )
    .assert_success();
}

#[test]
fn tracked_modify_untracked_add_and_tracked_delete_are_observed() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.write("old.txt", "old\n");
    repo.commit_all("init");
    set_file_threshold_one(&repo);

    bootstrap(&repo);
    repo.write("src/lib.rs", "pub fn a() { println!(\"x\"); }\n");
    repo.write("new.txt", "new\n");
    std::fs::remove_file(repo.root.join("old.txt")).unwrap();
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
    let manager = manager_for_stream(&stream_root(&repo, &stream.stream_id));
    let snapshot = manifest_for_stream(
        &stream_root(&repo, &stream.stream_id),
        &manager.anchor.manifest_id,
    );
    assert!(
        snapshot
            .entries
            .iter()
            .any(|entry| entry.path == RepoPath::from("src/lib.rs"))
    );
    assert!(
        snapshot
            .entries
            .iter()
            .any(|entry| entry.path == RepoPath::from("new.txt"))
    );
    assert!(
        !snapshot
            .entries
            .iter()
            .any(|entry| entry.path == RepoPath::from("old.txt"))
    );
}

#[test]
fn symlink_and_executable_bit_are_observed() {
    let repo = TestRepo::new();
    repo.write("bin/tool.sh", "#!/usr/bin/env bash\necho hi\n");
    repo.make_executable("bin/tool.sh");
    repo.symlink("link.txt", "bin/tool.sh");
    repo.commit_all("init");
    set_file_threshold_one(&repo);

    bootstrap(&repo);
    repo.write("bin/tool.sh", "#!/usr/bin/env bash\necho bye\n");
    repo.make_executable("bin/tool.sh");
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
        &[("SPROCKET_TEST_NOW", "500".to_string())],
    )
    .assert_success();

    let git = GitCli::discover(&repo.root).unwrap();
    let stream = compute_stream_identity(&repo.root, &git.head_state().unwrap());
    let manager = manager_for_stream(&stream_root(&repo, &stream.stream_id));
    let blob = git
        .show_file_at_commit(
            &manager.anchor.checkpoint_commit_oid,
            &RepoPath::from("bin/tool.sh"),
        )
        .unwrap();
    let snapshot = manifest_for_stream(
        &stream_root(&repo, &stream.stream_id),
        &manager.anchor.manifest_id,
    );
    let link = snapshot
        .entries
        .iter()
        .find(|entry| entry.path == RepoPath::from("link.txt"))
        .unwrap();
    let exec = snapshot
        .entries
        .iter()
        .find(|entry| entry.path == RepoPath::from("bin/tool.sh"))
        .unwrap();

    assert_eq!(
        String::from_utf8(blob).unwrap(),
        "#!/usr/bin/env bash\necho bye\n"
    );
    assert_eq!(link.mode, 0o120000);
    assert_eq!(exec.mode, 0o100755);
}

#[test]
fn excluded_paths_are_omitted_from_snapshots() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.write("node_modules/pkg/index.js", "ignored\n");
    repo.commit_all("init");

    bootstrap(&repo);
    let git = GitCli::discover(&repo.root).unwrap();
    let stream = compute_stream_identity(&repo.root, &git.head_state().unwrap());
    let manager = manager_for_stream(&stream_root(&repo, &stream.stream_id));
    let snapshot = manifest_for_stream(
        &stream_root(&repo, &stream.stream_id),
        &manager.anchor.manifest_id,
    );
    assert!(
        snapshot
            .entries
            .iter()
            .any(|entry| entry.path == RepoPath::from("src/lib.rs"))
    );
    assert!(
        !snapshot
            .entries
            .iter()
            .any(|entry| entry.path == RepoPath::from("node_modules/pkg/index.js"))
    );
}

#[test]
fn non_utf8_paths_are_supported_where_filesystem_allows() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn a() {}\n");

    #[cfg(not(target_os = "macos"))]
    {
        use std::os::unix::ffi::OsStringExt;

        repo.write_os(
            OsString::from_vec(vec![
                b's', b'r', b'c', b'/', b'f', 0x80, b'.', b't', b'x', b't',
            ]),
            b"hi\n",
        );
    }

    repo.commit_all("init");
    bootstrap(&repo);

    #[cfg(not(target_os = "macos"))]
    {
        let git = GitCli::discover(&repo.root).unwrap();
        let stream = compute_stream_identity(&repo.root, &git.head_state().unwrap());
        let manager = manager_for_stream(&stream_root(&repo, &stream.stream_id));
        let snapshot = manifest_for_stream(
            &stream_root(&repo, &stream.stream_id),
            &manager.anchor.manifest_id,
        );
        assert!(snapshot.entries.iter().any(|entry| {
            entry.path
                == RepoPath::from_bytes(vec![
                    b's', b'r', b'c', b'/', b'f', 0x80, b'.', b't', b'x', b't',
                ])
        }));
    }
}
