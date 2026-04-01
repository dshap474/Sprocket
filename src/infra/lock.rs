use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use uuid::Uuid;

pub struct RepoLock {
    path: PathBuf,
    owner_id: String,
    file: File,
}

impl RepoLock {
    pub fn try_acquire(path: &Path) -> Result<Option<Self>> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)?;
        match file.try_lock() {
            Ok(()) => {
                let owner_id = Uuid::new_v4().to_string();
                file.set_len(0)?;
                file.seek(SeekFrom::Start(0))?;
                writeln!(
                    file,
                    "owner_id={} pid={} acquired_at={}",
                    owner_id,
                    std::process::id(),
                    current_unix()
                )?;
                file.sync_all()?;
                Ok(Some(Self {
                    path: path.to_path_buf(),
                    owner_id,
                    file,
                }))
            }
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn owner_id(&self) -> &str {
        &self.owner_id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RepoLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn current_unix() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
