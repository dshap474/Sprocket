use std::path::Path;

use blake3::Hasher;

use crate::domain::session::{HeadState, StreamIdentity};

pub fn compute_stream_identity(worktree_root: &Path, head: &HeadState) -> StreamIdentity {
    let worktree_id = hash_hex(&worktree_bytes(worktree_root));
    let stream_source = match (&head.symref, &head.oid) {
        (Some(symref), _) => format!("symref:{symref}"),
        (None, Some(oid)) => format!("detached:{oid}"),
        (None, None) => "unborn".to_string(),
    };
    let stream_id = hash_hex(stream_source.as_bytes());
    let hidden_ref = format!("refs/sprocket/checkpoints/v2/{worktree_id}/{stream_id}");
    let display_name = head
        .symref
        .clone()
        .or_else(|| head.oid.clone().map(|oid| format!("detached:{oid}")))
        .unwrap_or_else(|| "unborn".to_string());
    StreamIdentity {
        worktree_id,
        stream_id,
        hidden_ref,
        display_name,
    }
}

pub fn hash_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn worktree_bytes(path: &Path) -> Vec<u8> {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;

        canonical.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        canonical.to_string_lossy().as_bytes().to_vec()
    }
}

pub fn snapshot_fingerprint(entries: &[(&[u8], u32, &str)]) -> String {
    let mut hasher = Hasher::new();
    for (path, mode, digest) in entries {
        hasher.update(path);
        hasher.update(&[0]);
        hasher.update(format!("{mode:o}").as_bytes());
        hasher.update(&[0]);
        hasher.update(digest.as_bytes());
        hasher.update(&[0xff]);
    }
    format!("blake3:{}", hasher.finalize().to_hex())
}
