use std::path::{Path, PathBuf};

use tempfile::NamedTempFile;

#[derive(Debug)]
pub struct TempIndex {
    _file: NamedTempFile,
    path: PathBuf,
}

impl TempIndex {
    pub fn new() -> anyhow::Result<Self> {
        let file = NamedTempFile::new()?;
        let path = file.path().to_path_buf();
        Ok(Self { _file: file, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
