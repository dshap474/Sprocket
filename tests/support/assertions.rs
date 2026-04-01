use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;

use sprocket::domain::journal::JournalEvent;
use sprocket::domain::manager::ManagerState;
use sprocket::domain::manifest::StrictSnapshot;
use sprocket::infra::atomic_write::{read_json, read_zstd_json};

use super::repo::TestRepo;

pub fn runtime_root(repo: &TestRepo) -> PathBuf {
    repo.git_path("sprocket")
}

pub fn manager_for_stream(stream_root: &Path) -> ManagerState {
    read_json(&stream_root.join("manager.json")).unwrap()
}

pub fn manifest_for_stream(stream_root: &Path, manifest_id: &str) -> StrictSnapshot {
    read_zstd_json(
        &stream_root
            .join("manifests")
            .join(format!("{manifest_id}.json.zst")),
    )
    .unwrap()
}

pub fn stream_root(repo: &TestRepo, stream_id: &str) -> PathBuf {
    runtime_root(repo).join("streams").join(stream_id)
}

pub fn turn_path(repo: &TestRepo, session_id: &str, turn_id: &str) -> PathBuf {
    runtime_root(repo)
        .join("turns")
        .join(encode_runtime_key(session_id))
        .join(format!("{}.json", encode_runtime_key(turn_id)))
}

pub fn hidden_ref_oid(repo: &TestRepo, refname: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("rev-parse")
        .arg("--verify")
        .arg("-q")
        .arg(refname)
        .current_dir(&repo.root)
        .output()
        .unwrap();
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

pub fn read_journal(stream_root: &Path) -> Vec<JournalEvent> {
    let path = stream_root.join("journal/events.ndjson");
    if !path.exists() {
        return Vec::new();
    }
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

pub fn head_file(repo: &TestRepo, spec: &str) -> String {
    repo.git(&["show", spec])
}

pub fn decode_runtime_key(value: &str) -> String {
    let encoded = value.strip_prefix("k-").unwrap_or(value);
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .unwrap();
    String::from_utf8(bytes).unwrap()
}

pub fn encode_runtime_key(value: &str) -> String {
    format!(
        "k-{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.as_bytes())
    )
}
