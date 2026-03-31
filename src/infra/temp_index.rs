use std::path::{Path, PathBuf};

use tempfile::TempDir;

#[derive(Debug)]
pub struct TempIndex {
    _dir: TempDir,
    path: PathBuf,
}

impl TempIndex {
    pub fn new() -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("index");
        Ok(Self { _dir: dir, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
