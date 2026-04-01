use std::path::{Path, PathBuf};

use crate::domain::repopath::RepoPath;
use crate::domain::session::{HeadState, RepoState};
use crate::infra::temp_index::TempIndex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub mode: u32,
    pub kind: String,
    pub oid: String,
    pub path: RepoPath,
}

pub trait GitBackend {
    fn repo_root(&self) -> &Path;
    fn git_path(&self, name: &str) -> anyhow::Result<PathBuf>;
    fn head_state(&self) -> anyhow::Result<HeadState>;
    fn repo_state(&self) -> anyhow::Result<RepoState>;
    fn path_exists_in_worktree(&self, path: &RepoPath) -> bool;
    fn list_tree_entries(
        &self,
        treeish: &str,
        pathspecs: &[String],
    ) -> anyhow::Result<Vec<TreeEntry>>;
    fn list_present_paths(&self, pathspecs: &[String]) -> anyhow::Result<Vec<RepoPath>>;
    fn list_head_owned_paths(
        &self,
        head_oid: &str,
        pathspecs: &[String],
    ) -> anyhow::Result<Vec<RepoPath>>;
    fn hash_object_for_path(&self, path: &RepoPath, bytes: &[u8]) -> anyhow::Result<String>;
    fn create_temp_index(&self) -> anyhow::Result<TempIndex>;
    fn read_tree_into_index(&self, index_path: &Path, treeish: &str) -> anyhow::Result<()>;
    fn update_index_cacheinfo(
        &self,
        index_path: &Path,
        mode: u32,
        oid: &str,
        path: &RepoPath,
    ) -> anyhow::Result<()>;
    fn force_remove_from_index(&self, index_path: &Path, paths: &[RepoPath]) -> anyhow::Result<()>;
    fn write_tree_from_index(&self, index_path: &Path) -> anyhow::Result<String>;
    fn commit_tree(
        &self,
        tree_oid: &str,
        parents: &[String],
        message: &str,
    ) -> anyhow::Result<String>;
    fn update_ref_cas(
        &self,
        refname: &str,
        new_oid: &str,
        old_oid: Option<&str>,
    ) -> anyhow::Result<()>;
    fn rev_parse_ref(&self, refname: &str) -> anyhow::Result<Option<String>>;
    fn show_file_at_commit(&self, commit_oid: &str, path: &RepoPath) -> anyhow::Result<Vec<u8>>;
    fn install_hook_file(&self, hook_name: &str, content: &[u8]) -> anyhow::Result<()>;
    fn staged_paths(&self) -> anyhow::Result<Vec<RepoPath>>;
    fn staged_paths_matching(&self, pathspecs: &[String]) -> anyhow::Result<Vec<RepoPath>>;
    fn commit_message(&self, commit_oid: &str) -> anyhow::Result<String>;
    fn commit_tree_oid(&self, commit_oid: &str) -> anyhow::Result<String>;
    fn advance_head_to_commit(
        &self,
        head: &HeadState,
        new_oid: &str,
        old_oid: Option<&str>,
    ) -> anyhow::Result<()>;
    fn sync_main_index_to_tree(&self, treeish: &str) -> anyhow::Result<()>;
}
