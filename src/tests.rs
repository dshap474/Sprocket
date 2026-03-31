use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use tempfile::TempDir;

use crate::app::{baseline, checkpoint, install_codex, pre_tool_use, session_start};
use crate::codex::hooks_json::{group_contains_marker, merge_hooks_json};
use crate::domain::decision::{ClassifyInput, Decision, classify};
use crate::domain::ids::compute_stream_identity;
use crate::domain::manager::{PendingSource, merge_pending_source, reconcile_pending};
use crate::domain::manifest::StrictSnapshot;
use crate::domain::policy::{CheckpointMode, Policy};
use crate::domain::repopath::RepoPath;
use crate::engine::observe::capture_strict_snapshot;
use crate::infra::git::GitBackend;
use crate::infra::git_cli::GitCli;
use crate::infra::store::{RuntimeLayout, Stores, load_toml, save_toml};

struct TestRepo {
    _dir: TempDir,
    root: PathBuf,
}

impl TestRepo {
    fn new() -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("repo");
        fs::create_dir_all(&root)?;
        run_git(&root, ["init"])?;
        run_git(&root, ["config", "user.name", "Test User"])?;
        run_git(&root, ["config", "user.email", "test@example.com"])?;
        Ok(Self { _dir: dir, root })
    }

    fn write(&self, rel: &str, contents: &str) {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn symlink(&self, rel: &str, target: &str) {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        std::os::unix::fs::symlink(target, path).unwrap();
    }

    fn make_executable(&self, rel: &str) {
        use std::os::unix::fs::PermissionsExt;

        let path = self.root.join(rel);
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }

    fn commit_all(&self, message: &str) {
        run_git(&self.root, ["add", "."]).unwrap();
        run_git(&self.root, ["commit", "-m", message]).unwrap();
    }
}

fn payload(repo: &Path, session_id: &str, turn_id: &str) -> serde_json::Value {
    json!({
        "cwd": repo.display().to_string(),
        "session_id": session_id,
        "turn_id": turn_id,
    })
}

fn session_payload(repo: &Path, session_id: &str) -> serde_json::Value {
    json!({
        "cwd": repo.display().to_string(),
        "session_id": session_id,
    })
}

fn run_git<const N: usize>(cwd: &Path, args: [&str; N]) -> anyhow::Result<String> {
    let out = Command::new("git").args(args).current_dir(cwd).output()?;
    if !out.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn current_stream_stores(repo: &Path) -> Stores {
    let git = GitCli::discover(repo).unwrap();
    let head = git.head_state().unwrap();
    let stream = compute_stream_identity(repo, &head);
    Stores::for_stream(RuntimeLayout::from_git(&git).unwrap(), &stream.stream_id)
}

fn current_manager(repo: &Path) -> crate::domain::manager::ManagerState {
    current_stream_stores(repo).manager.load().unwrap().unwrap()
}

fn write_policy(repo: &Path, mut edit: impl FnMut(&mut Policy)) {
    let path = repo.join(".sprocket/policy.toml");
    let mut policy = if path.exists() {
        load_toml::<Policy>(&path).unwrap()
    } else {
        Policy::default()
    };
    edit(&mut policy);
    save_toml(&path, &policy).unwrap();
}

#[test]
fn compute_stream_identity_changes_between_branch_and_detached() {
    let repo = TestRepo::new().unwrap();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");

    let git = GitCli::discover(&repo.root).unwrap();
    let attached = git.head_state().unwrap();
    let attached_stream = compute_stream_identity(&repo.root, &attached);
    run_git(&repo.root, ["checkout", "--detach"]).unwrap();
    let detached = GitCli::discover(&repo.root).unwrap().head_state().unwrap();
    let detached_stream = compute_stream_identity(&repo.root, &detached);

    assert_ne!(attached_stream.stream_id, detached_stream.stream_id);
    assert!(detached_stream.display_name.starts_with("detached:"));
}

#[test]
fn classify_inherited_dirty_materializes_after_threshold() {
    let decision = classify(&ClassifyInput {
        stream_id_now: "stream",
        stream_id_at_start: "stream",
        now_unix: 300,
        anchor_fingerprint: "anchor",
        turn_baseline_fingerprint: "dirty",
        anchor_fingerprint_at_start: "anchor",
        current_fingerprint: "dirty",
        global_changed_paths: 1,
        pending_turn_count: 1,
        pending_first_seen_at: Some(100),
        turn_threshold: 2,
        file_threshold: 9,
        age_seconds: 999,
    });
    assert!(matches!(
        decision,
        Decision::Materialize {
            source: PendingSource::Inherited,
            ..
        }
    ));
}

#[test]
fn reconcile_pending_merges_sources_and_sessions() {
    let snapshot = StrictSnapshot {
        fingerprint: "blake3:1".into(),
        manifest_id: "blake3:1".into(),
        entries: Vec::new(),
    };
    let pending = reconcile_pending(None, "s1", PendingSource::TurnLocal, 10, &snapshot);
    let merged = reconcile_pending(Some(pending), "s2", PendingSource::Inherited, 20, &snapshot);
    assert_eq!(merged.pending_turn_count, 2);
    assert_eq!(merged.source, PendingSource::Mixed);
    assert_eq!(
        merge_pending_source(PendingSource::External, PendingSource::External),
        PendingSource::External
    );
    assert_eq!(
        merged.touched_sessions,
        vec!["s1".to_string(), "s2".to_string()]
    );
}

#[test]
fn hooks_merge_replaces_only_sprocket_groups() {
    let existing = json!({
        "hooks": {
            "Stop": [
                {"hooks":[{"command":"other"}]},
                {"hooks":[{"command":"bin --sprocket-managed hook codex checkpoint"}]}
            ]
        }
    });
    let merged = merge_hooks_json(
        Some(existing),
        &[(
            "Stop".to_string(),
            json!({"hooks":[{"command":"new --sprocket-managed"}]}),
        )],
        "--sprocket-managed",
    )
    .unwrap();
    let stop = merged["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop.len(), 2);
    assert!(group_contains_marker(&stop[1], "--sprocket-managed"));
    assert_eq!(stop[0]["hooks"][0]["command"], "other");
}

#[test]
fn install_creates_policy_hooks_and_runtime() {
    let repo = TestRepo::new().unwrap();
    repo.write("src/main.rs", "fn main() {}\n");
    repo.commit_all("init");

    install_codex::run(&repo.root).unwrap();

    assert!(repo.root.join(".sprocket/policy.toml").exists());
    assert!(repo.root.join(".codex/hooks.json").exists());

    let git = GitCli::discover(&repo.root).unwrap();
    let runtime = RuntimeLayout::from_git(&git).unwrap();
    assert!(runtime.local_config_path.exists());
    assert!(
        git.git_path("hooks")
            .unwrap()
            .join("prepare-commit-msg")
            .exists()
    );
}

#[test]
fn session_start_bootstraps_hidden_anchor_commit() {
    let repo = TestRepo::new().unwrap();
    repo.write("src/main.rs", "fn main() {}\n");
    repo.commit_all("init");

    session_start::run(&session_payload(&repo.root, "s1")).unwrap();

    let manager = current_manager(&repo.root);
    assert_eq!(manager.generation, 1);
    let git = GitCli::discover(&repo.root).unwrap();
    let hidden_oid = git
        .rev_parse_ref(&manager.stream.hidden_ref)
        .unwrap()
        .unwrap();
    assert_eq!(hidden_oid, manager.anchor.checkpoint_commit_oid);
}

#[test]
fn inherited_dirty_state_eventually_materializes() {
    let repo = TestRepo::new().unwrap();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");

    session_start::run(&session_payload(&repo.root, "s1")).unwrap();
    repo.write("src/lib.rs", "pub fn a() { println!(\"x\"); }\n");

    session_start::run(&session_payload(&repo.root, "s2")).unwrap();
    baseline::run(&payload(&repo.root, "s2", "t1")).unwrap();
    checkpoint::run(&payload(&repo.root, "s2", "t1")).unwrap();
    assert!(current_manager(&repo.root).pending.is_some());

    baseline::run(&payload(&repo.root, "s2", "t2")).unwrap();
    checkpoint::run(&payload(&repo.root, "s2", "t2")).unwrap();

    let manager = current_manager(&repo.root);
    assert!(manager.pending.is_none());
    assert_eq!(manager.generation, 2);
}

#[test]
fn stream_change_between_baseline_and_stop_noops() {
    let repo = TestRepo::new().unwrap();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");

    session_start::run(&session_payload(&repo.root, "s1")).unwrap();
    baseline::run(&payload(&repo.root, "s1", "t1")).unwrap();
    run_git(&repo.root, ["checkout", "-b", "feature"]).unwrap();
    checkpoint::run(&payload(&repo.root, "s1", "t1")).unwrap();

    let stores = current_stream_stores(&repo.root);
    assert!(stores.turns.load("s1", "t1").unwrap().is_none());
    assert_eq!(current_manager(&repo.root).generation, 1);
}

#[test]
fn hidden_checkpoint_tree_matches_snapshot_and_exec_mode() {
    let repo = TestRepo::new().unwrap();
    repo.write("bin/tool.sh", "#!/usr/bin/env bash\necho hi\n");
    repo.make_executable("bin/tool.sh");
    repo.commit_all("init");
    write_policy(&repo.root, |policy| {
        policy.checkpoint.file_threshold = 1;
    });

    session_start::run(&session_payload(&repo.root, "s1")).unwrap();
    repo.write("bin/tool.sh", "#!/usr/bin/env bash\necho bye\n");
    repo.make_executable("bin/tool.sh");
    baseline::run(&payload(&repo.root, "s1", "t1")).unwrap();
    checkpoint::run(&payload(&repo.root, "s1", "t1")).unwrap();

    let manager = current_manager(&repo.root);
    let git = GitCli::discover(&repo.root).unwrap();
    let blob = git
        .show_file_at_commit(
            &manager.anchor.checkpoint_commit_oid,
            &RepoPath::from("bin/tool.sh"),
        )
        .unwrap();
    assert_eq!(
        String::from_utf8(blob).unwrap(),
        "#!/usr/bin/env bash\necho bye\n"
    );
}

#[test]
fn promotion_can_advance_visible_history() {
    let repo = TestRepo::new().unwrap();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    write_policy(&repo.root, |policy| {
        policy.checkpoint.file_threshold = 1;
        policy.checkpoint.mode = CheckpointMode::HiddenThenPromote;
        policy.promotion.enabled = true;
    });

    session_start::run(&session_payload(&repo.root, "s1")).unwrap();
    repo.write("src/lib.rs", "pub fn a() { println!(\"x\"); }\n");
    baseline::run(&payload(&repo.root, "s1", "t1")).unwrap();
    checkpoint::run(&payload(&repo.root, "s1", "t1")).unwrap();

    let head_contents = run_git(&repo.root, ["show", "HEAD:src/lib.rs"]).unwrap();
    assert_eq!(head_contents, "pub fn a() { println!(\"x\"); }");
}

#[test]
fn lock_busy_causes_noop() {
    let repo = TestRepo::new().unwrap();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.commit_all("init");
    session_start::run(&session_payload(&repo.root, "s1")).unwrap();

    repo.write("src/lib.rs", "pub fn a() { println!(\"x\"); }\n");
    baseline::run(&payload(&repo.root, "s1", "t1")).unwrap();
    let stores = current_stream_stores(&repo.root);
    if let Some(parent) = stores.lock_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&stores.lock_path, b"busy\n").unwrap();
    checkpoint::run(&payload(&repo.root, "s1", "t1")).unwrap();
    assert_eq!(current_manager(&repo.root).generation, 1);
}

#[test]
fn snapshot_capture_handles_symlinks() {
    let repo = TestRepo::new().unwrap();
    repo.write("src/lib.rs", "pub fn a() {}\n");
    repo.symlink("link.txt", "src/lib.rs");
    repo.commit_all("init");

    let git = GitCli::discover(&repo.root).unwrap();
    let snapshot = capture_strict_snapshot(git.repo_root(), &git, &Policy::default()).unwrap();
    let symlink = snapshot
        .entries
        .iter()
        .find(|entry| entry.path == RepoPath::from("link.txt"))
        .unwrap();
    assert_eq!(symlink.mode, 0o120000);
}

#[test]
fn prettool_guard_blocks_commit_but_not_status() {
    assert!(pre_tool_use::should_block_git_command(Some(
        "git commit -m hi"
    )));
    assert!(pre_tool_use::should_block_git_command(Some(
        "git reset --hard HEAD"
    )));
    assert!(!pre_tool_use::should_block_git_command(Some(
        "git status --short"
    )));
}
