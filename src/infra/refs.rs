pub fn checkpoint_ref_path(worktree_id: &str, stream_id: &str) -> String {
    format!("refs/sprocket/checkpoints/v2/{worktree_id}/{stream_id}")
}
