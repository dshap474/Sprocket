use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use bstr::ByteSlice;
use tempfile::NamedTempFile;

use crate::domain::repopath::RepoPath;
use crate::domain::session::{HeadState, RepoState};
use crate::infra::atomic_write::atomic_write_bytes;
use crate::infra::git::{GitBackend, TreeEntry};
use crate::infra::temp_index::TempIndex;

#[derive(Debug, Clone)]
pub struct GitCli {
    repo_root: PathBuf,
}

impl GitCli {
    pub fn discover(target: &Path) -> Result<Self> {
        let output = Command::new("git")
            .arg("-C")
            .arg(target)
            .arg("rev-parse")
            .arg("--show-toplevel")
            .output()
            .with_context(|| format!("failed to resolve git repo from {}", target.display()))?;
        if !output.status.success() {
            bail!(
                "target is not inside a git repository: {}",
                target.display()
            );
        }
        let repo_root = path_from_stdout(&output.stdout)?;
        Ok(Self { repo_root })
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    fn run<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_with_env(args, &[], None)
    }

    fn run_with_env<I, S>(
        &self,
        args: I,
        envs: &[(&str, &OsStr)],
        stdin: Option<&[u8]>,
    ) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = Command::new("git");
        cmd.args(args)
            .current_dir(&self.repo_root)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in envs {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn().context("failed to spawn git")?;
        if let Some(input) = stdin {
            use std::io::Write;

            child
                .stdin
                .as_mut()
                .ok_or_else(|| anyhow!("git stdin was not available"))?
                .write_all(input)?;
        }
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(anyhow!(
                "git failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(output)
    }

    fn run_may_fail<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Ok(Command::new("git")
            .args(args)
            .current_dir(&self.repo_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()?)
    }

    fn path_list_from_output(&self, bytes: &[u8]) -> Vec<RepoPath> {
        bytes
            .split_str(b"\0")
            .filter(|entry| !entry.is_empty())
            .map(|entry| RepoPath::from_bytes(entry.to_vec()))
            .collect()
    }

    fn tree_entries_from_tree(
        &self,
        treeish: &str,
        pathspecs: &[String],
    ) -> Result<Vec<TreeEntry>> {
        let mut args: Vec<OsString> = vec![
            OsString::from("ls-tree"),
            OsString::from("-r"),
            OsString::from("-z"),
            OsString::from("--full-tree"),
            OsString::from(treeish),
            OsString::from("--"),
        ];
        args.extend(pathspecs.iter().map(OsString::from));
        let out = self.run(args)?;
        parse_tree_entries(&out.stdout)
    }

    fn status_paths(&self, pathspecs: &[String]) -> Result<Vec<RepoPath>> {
        let mut args: Vec<OsString> = vec![
            OsString::from("status"),
            OsString::from("--porcelain=v1"),
            OsString::from("-z"),
            OsString::from("--untracked-files=all"),
            OsString::from("--no-renames"),
            OsString::from("--"),
        ];
        args.extend(pathspecs.iter().map(OsString::from));
        let out = self.run(args)?;
        Ok(out
            .stdout
            .split_str(b"\0")
            .filter(|entry| entry.len() > 3)
            .map(|entry| RepoPath::from_bytes(entry[3..].to_vec()))
            .collect())
    }

    fn hooks_dir(&self) -> Result<PathBuf> {
        let configured = self.run_may_fail(["config", "--path", "core.hooksPath"])?;
        if configured.status.success() {
            let raw = trim_trailing_newline(&configured.stdout);
            if !raw.is_empty() {
                let path = PathBuf::from(OsString::from_vec(raw.to_vec()));
                return Ok(if path.is_absolute() {
                    path
                } else {
                    self.repo_root.join(path)
                });
            }
        }
        self.git_path("hooks")
    }
}

impl GitBackend for GitCli {
    fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    fn git_path(&self, name: &str) -> Result<PathBuf> {
        let out = self.run(["rev-parse", "--git-path", name])?;
        let path = PathBuf::from(OsString::from_vec(
            trim_trailing_newline(&out.stdout).to_vec(),
        ));
        Ok(if path.is_absolute() {
            path
        } else {
            self.repo_root.join(path)
        })
    }

    fn head_state(&self) -> Result<HeadState> {
        let oid_out = self.run_may_fail(["rev-parse", "--verify", "-q", "HEAD"])?;
        let oid = if oid_out.status.success() {
            Some(String::from_utf8(oid_out.stdout)?.trim().to_string())
        } else {
            None
        };

        let symref_out = self.run_may_fail(["symbolic-ref", "-q", "HEAD"])?;
        let symref = if symref_out.status.success() {
            Some(String::from_utf8(symref_out.stdout)?.trim().to_string())
        } else {
            None
        };

        Ok(HeadState {
            detached: symref.is_none() && oid.is_some(),
            oid,
            symref,
        })
    }

    fn repo_state(&self) -> Result<RepoState> {
        let sparse = self
            .run_may_fail(["config", "--bool", "core.sparseCheckout"])?
            .stdout;
        Ok(RepoState {
            merge_in_progress: self.git_path("MERGE_HEAD")?.exists(),
            rebase_in_progress: self.git_path("rebase-merge")?.exists()
                || self.git_path("rebase-apply")?.exists(),
            cherry_pick_in_progress: self.git_path("CHERRY_PICK_HEAD")?.exists(),
            sequencer_in_progress: self.git_path("sequencer")?.exists(),
            sparse_checkout: String::from_utf8_lossy(&sparse).trim() == "true",
        })
    }

    fn path_exists_in_worktree(&self, path: &RepoPath) -> bool {
        fs::symlink_metadata(path.join_to(&self.repo_root)).is_ok()
    }

    fn list_tree_entries(&self, treeish: &str, pathspecs: &[String]) -> Result<Vec<TreeEntry>> {
        self.tree_entries_from_tree(treeish, pathspecs)
    }

    fn list_present_paths(&self, pathspecs: &[String]) -> Result<Vec<RepoPath>> {
        let mut seen = BTreeSet::new();
        if let Some(head_oid) = self.head_state()?.oid {
            for entry in self.tree_entries_from_tree(&head_oid, pathspecs)? {
                if self.path_exists_in_worktree(&entry.path) {
                    seen.insert(entry.path);
                }
            }
        }
        for path in self.status_paths(pathspecs)? {
            if self.path_exists_in_worktree(&path) {
                seen.insert(path);
            }
        }
        Ok(seen.into_iter().collect())
    }

    fn list_head_owned_paths(&self, head_oid: &str, pathspecs: &[String]) -> Result<Vec<RepoPath>> {
        Ok(self
            .tree_entries_from_tree(head_oid, pathspecs)?
            .into_iter()
            .map(|entry| entry.path)
            .collect())
    }

    fn hash_object_for_path(&self, path: &RepoPath, bytes: &[u8]) -> Result<String> {
        let out = self.run_with_env(
            [
                OsString::from("hash-object"),
                OsString::from("-w"),
                OsString::from("--stdin"),
                OsString::from("--path"),
                path.to_os_string(),
            ],
            &[],
            Some(bytes),
        )?;
        Ok(String::from_utf8(out.stdout)?.trim().to_string())
    }

    fn create_temp_index(&self) -> Result<TempIndex> {
        TempIndex::new()
    }

    fn read_tree_into_index(&self, index_path: &Path, treeish: &str) -> Result<()> {
        let args = vec![OsString::from("read-tree"), OsString::from(treeish)];
        self.run_with_env(args, &[("GIT_INDEX_FILE", index_path.as_os_str())], None)?;
        Ok(())
    }

    fn update_index_cacheinfo(
        &self,
        index_path: &Path,
        mode: u32,
        oid: &str,
        path: &RepoPath,
    ) -> Result<()> {
        let mut stdin = Vec::new();
        stdin.extend_from_slice(format!("{mode:o} {oid}\t").as_bytes());
        stdin.extend_from_slice(path.as_bytes());
        stdin.push(0);
        let args = vec![
            OsString::from("update-index"),
            OsString::from("-z"),
            OsString::from("--index-info"),
        ];
        self.run_with_env(
            args,
            &[("GIT_INDEX_FILE", index_path.as_os_str())],
            Some(&stdin),
        )?;
        Ok(())
    }

    fn force_remove_from_index(&self, index_path: &Path, paths: &[RepoPath]) -> Result<()> {
        let mut stdin = Vec::new();
        for path in paths {
            stdin.extend_from_slice(path.as_bytes());
            stdin.push(0);
        }
        let args = vec![
            OsString::from("update-index"),
            OsString::from("-z"),
            OsString::from("--force-remove"),
            OsString::from("--stdin"),
        ];
        self.run_with_env(
            args,
            &[("GIT_INDEX_FILE", index_path.as_os_str())],
            Some(&stdin),
        )?;
        Ok(())
    }

    fn write_tree_from_index(&self, index_path: &Path) -> Result<String> {
        let out = self.run_with_env(
            vec![OsString::from("write-tree")],
            &[("GIT_INDEX_FILE", index_path.as_os_str())],
            None,
        )?;
        Ok(String::from_utf8(out.stdout)?.trim().to_string())
    }

    fn commit_tree(&self, tree_oid: &str, parents: &[String], message: &str) -> Result<String> {
        let message_file = NamedTempFile::new()?;
        fs::write(message_file.path(), message)?;
        let mut args: Vec<OsString> = vec![
            OsString::from("commit-tree"),
            OsString::from(tree_oid),
            OsString::from("-F"),
            message_file.path().as_os_str().to_owned(),
        ];
        for parent in parents {
            args.push(OsString::from("-p"));
            args.push(OsString::from(parent));
        }
        let author_name = OsStr::new("Sprocket");
        let author_email = OsStr::new("sprocket@local.invalid");
        let out = self.run_with_env(
            args,
            &[
                ("GIT_AUTHOR_NAME", author_name),
                ("GIT_AUTHOR_EMAIL", author_email),
                ("GIT_COMMITTER_NAME", author_name),
                ("GIT_COMMITTER_EMAIL", author_email),
            ],
            None,
        )?;
        Ok(String::from_utf8(out.stdout)?.trim().to_string())
    }

    fn update_ref_cas(&self, refname: &str, new_oid: &str, old_oid: Option<&str>) -> Result<()> {
        let mut args = vec![
            OsString::from("update-ref"),
            OsString::from(refname),
            OsString::from(new_oid),
        ];
        if let Some(old) = old_oid {
            args.push(OsString::from(old));
        }
        self.run(args)?;
        Ok(())
    }

    fn rev_parse_ref(&self, refname: &str) -> Result<Option<String>> {
        let out = self.run_may_fail(["rev-parse", "--verify", "-q", refname])?;
        if !out.status.success() {
            return Ok(None);
        }
        Ok(Some(String::from_utf8(out.stdout)?.trim().to_string()))
    }

    fn show_file_at_commit(&self, commit_oid: &str, path: &RepoPath) -> Result<Vec<u8>> {
        let mut spec = commit_oid.as_bytes().to_vec();
        spec.push(b':');
        spec.extend_from_slice(path.as_bytes());
        let out = self.run([
            OsString::from("cat-file"),
            OsString::from("blob"),
            OsString::from_vec(spec),
        ])?;
        Ok(out.stdout)
    }

    fn install_hook_file(&self, hook_name: &str, content: &[u8]) -> Result<()> {
        let hooks_dir = self.hooks_dir()?;
        fs::create_dir_all(&hooks_dir)?;
        let path = hooks_dir.join(hook_name);
        atomic_write_bytes(&path, content)?;
        let mut perms = fs::metadata(&path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms)?;
        Ok(())
    }

    fn staged_paths(&self) -> Result<Vec<RepoPath>> {
        let out = self.run(["diff", "--cached", "--name-only", "-z"])?;
        Ok(self.path_list_from_output(&out.stdout))
    }

    fn staged_paths_matching(&self, pathspecs: &[String]) -> Result<Vec<RepoPath>> {
        let mut args: Vec<OsString> = vec![
            OsString::from("diff"),
            OsString::from("--cached"),
            OsString::from("--name-only"),
            OsString::from("-z"),
            OsString::from("--"),
        ];
        args.extend(pathspecs.iter().map(OsString::from));
        let out = self.run(args)?;
        Ok(self.path_list_from_output(&out.stdout))
    }

    fn commit_message(&self, commit_oid: &str) -> Result<String> {
        let out = self.run(["log", "-1", "--format=%B", commit_oid])?;
        Ok(String::from_utf8(out.stdout)?.trim_end().to_string())
    }

    fn commit_tree_oid(&self, commit_oid: &str) -> Result<String> {
        let spec = format!("{commit_oid}^{{tree}}");
        let out = self.run(["rev-parse", spec.as_str()])?;
        Ok(String::from_utf8(out.stdout)?.trim().to_string())
    }

    fn advance_head_to_commit(
        &self,
        head: &HeadState,
        new_oid: &str,
        old_oid: Option<&str>,
    ) -> Result<()> {
        if let Some(symref) = &head.symref {
            self.update_ref_cas(symref, new_oid, old_oid)
        } else {
            self.update_ref_cas("HEAD", new_oid, old_oid)
        }
    }

    fn sync_main_index_to_tree(&self, treeish: &str) -> Result<()> {
        self.run(["read-tree", "--reset", treeish])?;
        Ok(())
    }
}

fn trim_trailing_newline(bytes: &[u8]) -> &[u8] {
    bytes
        .strip_suffix(b"\n")
        .or_else(|| bytes.strip_suffix(b"\r\n"))
        .unwrap_or(bytes)
}

fn parse_tree_entries(bytes: &[u8]) -> Result<Vec<TreeEntry>> {
    let mut entries = Vec::new();
    for record in bytes.split_str(b"\0").filter(|entry| !entry.is_empty()) {
        let mut fields = record.splitn(2, |byte| *byte == b'\t');
        let Some(meta) = fields.next() else {
            bail!("unexpected git ls-tree record format");
        };
        let Some(path) = fields.next() else {
            bail!("unexpected git ls-tree record format");
        };
        let mut parts = meta.split_str(b" ");
        let mode = parts
            .next()
            .ok_or_else(|| anyhow!("missing tree entry mode"))?;
        let kind = parts
            .next()
            .ok_or_else(|| anyhow!("missing tree entry type"))?;
        let oid = parts
            .next()
            .ok_or_else(|| anyhow!("missing tree entry oid"))?;
        let mode = u32::from_str_radix(&String::from_utf8_lossy(mode), 8)
            .context("invalid tree entry mode")?;
        entries.push(TreeEntry {
            mode,
            kind: String::from_utf8(kind.to_vec())
                .map_err(|error| anyhow!("invalid tree entry type: {error}"))?,
            oid: String::from_utf8(oid.to_vec())
                .map_err(|error| anyhow!("invalid tree entry oid: {error}"))?,
            path: RepoPath::from_bytes(path.to_vec()),
        });
    }
    Ok(entries)
}

fn path_from_stdout(bytes: &[u8]) -> Result<PathBuf> {
    Ok(PathBuf::from(OsString::from_vec(
        trim_trailing_newline(bytes).to_vec(),
    )))
}
