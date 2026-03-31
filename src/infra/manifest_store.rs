use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Serialize, de::DeserializeOwned};

use crate::infra::atomic_write::{atomic_write_zstd_json, read_zstd_json};

#[derive(Debug, Clone)]
pub struct ManifestStore {
    root: PathBuf,
}

impl ManifestStore {
    pub fn new(stream_root: &Path) -> Self {
        Self {
            root: stream_root.join("manifests"),
        }
    }

    pub fn put<T: Serialize>(&self, manifest_id: &str, value: &T) -> Result<()> {
        atomic_write_zstd_json(&self.path(manifest_id), value)
    }

    pub fn get<T: DeserializeOwned>(&self, manifest_id: &str) -> Result<T> {
        read_zstd_json(&self.path(manifest_id))
    }

    pub fn path(&self, manifest_id: &str) -> PathBuf {
        self.root.join(format!("{manifest_id}.json.zst"))
    }
}

pub struct TypedManifest<T> {
    _marker: PhantomData<T>,
}
