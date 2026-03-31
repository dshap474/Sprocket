use serde::{Deserialize, Serialize};

use crate::domain::repopath::RepoPath;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrictEntry {
    pub path: RepoPath,
    pub mode: u32,
    pub digest: String,
    pub git_oid: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrictSnapshot {
    pub fingerprint: String,
    pub manifest_id: String,
    pub entries: Vec<StrictEntry>,
}
