use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub struct RepoLock {
    path: PathBuf,
    _file: File,
}

impl RepoLock {
    pub fn try_acquire(path: &Path, stale_after: Duration) -> anyhow::Result<Option<Self>> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        match OpenOptions::new().create_new(true).write(true).open(path) {
            Ok(mut file) => {
                writeln!(
                    file,
                    "pid={} acquired_at={}",
                    std::process::id(),
                    current_unix()
                )?;
                file.sync_all()?;
                return Ok(Some(Self {
                    path: path.to_path_buf(),
                    _file: file,
                }));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }

        let meta = fs::metadata(path)?;
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let age = SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default();
        if age < stale_after {
            return Ok(None);
        }

        let _ = fs::remove_file(path);
        match OpenOptions::new().create_new(true).write(true).open(path) {
            Ok(mut file) => {
                writeln!(
                    file,
                    "pid={} reacquired_at={}",
                    std::process::id(),
                    current_unix()
                )?;
                file.sync_all()?;
                Ok(Some(Self {
                    path: path.to_path_buf(),
                    _file: file,
                }))
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

impl Drop for RepoLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn current_unix() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
