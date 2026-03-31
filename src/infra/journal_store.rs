use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::infra::atomic_write::append_ndjson_line;

#[derive(Debug, Clone)]
pub struct JournalStore {
    path: PathBuf,
}

impl JournalStore {
    pub fn new(stream_root: &Path) -> Self {
        Self {
            path: stream_root.join("journal/events.ndjson"),
        }
    }

    pub fn append<T: Serialize>(&self, value: &T) -> Result<()> {
        append_ndjson_line(&self.path, value)
    }
}
