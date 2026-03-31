use std::path::{Path, PathBuf};

use anyhow::Result;
use base64::Engine;
use serde::{Serialize, de::DeserializeOwned};

use crate::domain::manager::ManagerState;
use crate::domain::session::SessionState;
use crate::domain::turn::TurnState;
use crate::infra::atomic_write::{atomic_write_json, read_json};
use crate::infra::git::GitBackend;
use crate::infra::journal_store::JournalStore;
use crate::infra::manifest_store::ManifestStore;

#[derive(Debug, Clone)]
pub struct RuntimeLayout {
    pub root: PathBuf,
    pub local_config_path: PathBuf,
    pub streams_root: PathBuf,
    pub lock_path: PathBuf,
}

impl RuntimeLayout {
    pub fn from_git(git: &dyn GitBackend) -> Result<Self> {
        let root = git.git_path("sprocket")?;
        Ok(Self {
            local_config_path: root.join("local.toml"),
            streams_root: root.join("streams"),
            lock_path: root.join("checkpoint.lock"),
            root,
        })
    }

    pub fn stream_root(&self, stream_id: &str) -> PathBuf {
        self.streams_root.join(stream_id)
    }
}

#[derive(Debug, Clone)]
pub struct Stores {
    pub runtime: RuntimeLayout,
    pub manager: ManagerStore,
    pub turns: TurnStore,
    pub sessions: SessionStore,
    pub manifests: ManifestStore,
    pub journal: JournalStore,
    pub lock_path: PathBuf,
}

impl Stores {
    pub fn for_stream(runtime: RuntimeLayout, stream_id: &str) -> Self {
        let stream_root = runtime.stream_root(stream_id);
        Self {
            lock_path: runtime.lock_path.clone(),
            manager: ManagerStore::new(&stream_root),
            turns: TurnStore::new(&stream_root),
            sessions: SessionStore::new(&stream_root),
            manifests: ManifestStore::new(&stream_root),
            journal: JournalStore::new(&stream_root),
            runtime,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ManagerStore {
    path: PathBuf,
}

impl ManagerStore {
    pub fn new(stream_root: &Path) -> Self {
        Self {
            path: stream_root.join("manager.json"),
        }
    }

    pub fn load(&self) -> Result<Option<ManagerState>> {
        if !self.path.exists() {
            return Ok(None);
        }
        Ok(Some(read_json(&self.path)?))
    }

    pub fn save(&self, manager: &ManagerState) -> Result<()> {
        atomic_write_json(&self.path, manager)
    }
}

#[derive(Debug, Clone)]
pub struct TurnStore {
    root: PathBuf,
}

impl TurnStore {
    pub fn new(stream_root: &Path) -> Self {
        Self {
            root: stream_root.join("turns"),
        }
    }

    pub fn load(&self, session_id: &str, turn_id: &str) -> Result<Option<TurnState>> {
        let path = self.path(session_id, turn_id);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(read_json(&path)?))
    }

    pub fn save(&self, turn: &TurnState) -> Result<()> {
        atomic_write_json(&self.path(&turn.session_id, &turn.turn_id), turn)
    }

    pub fn delete(&self, session_id: &str, turn_id: &str) -> Result<()> {
        let path = self.path(session_id, turn_id);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    pub fn session_dir(&self, session_id: &str) -> PathBuf {
        self.root.join(encode_runtime_key(session_id))
    }

    fn path(&self, session_id: &str, turn_id: &str) -> PathBuf {
        self.session_dir(session_id)
            .join(format!("{}.json", encode_runtime_key(turn_id)))
    }
}

#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    pub fn new(stream_root: &Path) -> Self {
        Self {
            root: stream_root.join("sessions"),
        }
    }

    pub fn load(&self, session_id: &str) -> Result<Option<SessionState>> {
        let path = self.path(session_id);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(read_json(&path)?))
    }

    pub fn save(&self, session: &SessionState) -> Result<()> {
        atomic_write_json(&self.path(&session.session_id), session)
    }

    pub fn delete(&self, session_id: &str) -> Result<()> {
        let path = self.path(session_id);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    fn path(&self, session_id: &str) -> PathBuf {
        self.root
            .join(format!("{}.json", encode_runtime_key(session_id)))
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LocalConfig {
    pub version: u32,
    pub binary_path: String,
    pub install_version: String,
    pub worktree_id: String,
}

pub fn save_local_config(runtime: &RuntimeLayout, local: &LocalConfig) -> Result<()> {
    let content = toml::to_string_pretty(local)?;
    crate::infra::atomic_write::atomic_write_bytes(&runtime.local_config_path, content.as_bytes())
}

pub fn load_toml<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let content = std::fs::read_to_string(path)?;
    Ok(toml::from_str(&content)?)
}

pub fn save_toml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut content = toml::to_string_pretty(value)?;
    content.push('\n');
    crate::infra::atomic_write::atomic_write_bytes(path, content.as_bytes())
}

pub(crate) fn encode_runtime_key(value: &str) -> String {
    format!(
        "k-{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.as_bytes())
    )
}
