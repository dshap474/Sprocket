use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::Result;

use crate::domain::ids::snapshot_fingerprint;
use crate::domain::manifest::{StrictEntry, StrictSnapshot};
use crate::domain::policy::Policy;
use crate::infra::git::GitBackend;

pub fn capture_strict_snapshot(
    repo_root: &Path,
    git: &dyn GitBackend,
    policy: &Policy,
) -> Result<StrictSnapshot> {
    let mut entries = Vec::new();
    for path in git.list_present_paths(&policy.git_owned_pathspecs())? {
        let abs = path.join_to(repo_root);
        let meta = fs::symlink_metadata(&abs)?;

        let (mode, bytes) = if meta.file_type().is_symlink() {
            #[cfg(unix)]
            let target = std::fs::read_link(&abs)?;
            #[cfg(unix)]
            let bytes = std::os::unix::ffi::OsStrExt::as_bytes(target.as_os_str()).to_vec();
            (0o120000, bytes)
        } else if meta.is_file() {
            let exec = (meta.permissions().mode() & 0o111) != 0;
            let mode = if exec { 0o100755 } else { 0o100644 };
            (mode, fs::read(&abs)?)
        } else {
            continue;
        };

        let digest = format!("blake3:{}", blake3::hash(&bytes).to_hex());
        let git_oid = git.hash_object_for_path(&path, &bytes)?;
        entries.push(StrictEntry {
            path,
            mode,
            digest,
            git_oid,
        });
    }

    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let fingerprint = snapshot_fingerprint(
        &entries
            .iter()
            .map(|entry| (entry.path.as_bytes(), entry.mode, entry.digest.as_str()))
            .collect::<Vec<_>>(),
    );

    Ok(StrictSnapshot {
        fingerprint: fingerprint.clone(),
        manifest_id: fingerprint,
        entries,
    })
}
