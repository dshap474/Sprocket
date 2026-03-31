use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(not(target_os = "macos"))]
use std::ffi::OsString;

use tempfile::TempDir;

use super::env::HermeticEnv;

pub struct TestRepo {
    _dir: TempDir,
    pub root: PathBuf,
    pub hermetic: HermeticEnv,
}

impl TestRepo {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        let hermetic = HermeticEnv::new(dir.path());
        hermetic.ensure_dirs();
        let repo = Self {
            _dir: dir,
            root,
            hermetic,
        };
        repo.git(&["init"]);
        repo.git(&["config", "user.name", "Test User"]);
        repo.git(&["config", "user.email", "test@example.com"]);
        repo
    }

    pub fn for_existing(root: PathBuf, hermetic: HermeticEnv) -> Self {
        Self {
            _dir: tempfile::tempdir().unwrap(),
            root,
            hermetic,
        }
    }

    pub fn write(&self, rel: &str, contents: &str) {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[cfg(not(target_os = "macos"))]
    pub fn write_os(&self, rel: OsString, contents: &[u8]) {
        let path = self.root.join(&rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    pub fn symlink(&self, rel: &str, target: &str) {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, path).unwrap();
    }

    pub fn make_executable(&self, rel: &str) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let path = self.root.join(rel);
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(path, perms).unwrap();
        }
    }

    pub fn commit_all(&self, message: &str) {
        self.git(&["add", "."]);
        self.git(&["commit", "-m", message]);
    }

    pub fn git(&self, args: &[&str]) -> String {
        run_git(&self.root, args)
    }

    pub fn git_path(&self, name: &str) -> PathBuf {
        let raw = self.git(&["rev-parse", "--git-path", name]);
        let path = PathBuf::from(raw);
        if path.is_absolute() {
            path
        } else {
            self.root.join(path)
        }
    }

    pub fn worktree_add(&self, path: &Path, branch: &str) {
        run_git(
            &self.root,
            &["worktree", "add", path.to_str().unwrap(), "-b", branch],
        );
    }
}

pub fn run_git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    if !output.status.success() {
        panic!(
            "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}
