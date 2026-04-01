use std::fs;
use std::path::Path;

use anyhow::{Result, anyhow};
use serde_json::json;

use crate::cli::hook_marker;
use crate::codex::hooks_json::merge_hooks_json;
use crate::domain::ids::hash_hex;
use crate::domain::policy::Policy;
use crate::infra::git::GitBackend;
use crate::infra::git_cli::GitCli;
use crate::infra::store::{LocalConfig, RuntimeLayout, load_toml, save_local_config, save_toml};

pub fn run(repo: &Path) -> Result<()> {
    let git = GitCli::discover(repo)?;
    let runtime = RuntimeLayout::from_git(&git)?;
    fs::create_dir_all(&runtime.root)?;

    let policy_path = git.repo_root().join(".sprocket/policy.toml");
    if !policy_path.exists() {
        save_toml(&policy_path, &Policy::default())?;
    }
    let policy: Policy = load_toml(&policy_path)?;

    let binary_path = std::env::current_exe()?;
    let binary_path_string = binary_path
        .as_os_str()
        .to_str()
        .ok_or_else(|| anyhow!("non-utf8 executable path is unsupported"))?
        .to_string();
    let local = LocalConfig {
        version: 2,
        binary_path: binary_path_string.clone(),
        install_version: env!("CARGO_PKG_VERSION").to_string(),
        worktree_id: hash_hex(worktree_bytes(git.repo_root()).as_slice()),
    };
    save_local_config(&runtime, &local)?;

    merge_codex_hooks(&git, &binary_path_string, policy.guard.codex_pretool)?;
    if policy.guard.git_prepare_commit_msg {
        install_prepare_commit_msg(&git)?;
    }
    Ok(())
}

fn merge_codex_hooks(git: &GitCli, binary_path: &str, install_pretool: bool) -> Result<()> {
    let hooks_path = git.repo_root().join(".codex/hooks.json");
    let existing = if hooks_path.exists() {
        Some(serde_json::from_str(&fs::read_to_string(&hooks_path)?)?)
    } else {
        None
    };
    let mut groups = vec![
        (
            "SessionStart".to_string(),
            generated_group("session-start", binary_path, None),
        ),
        (
            "UserPromptSubmit".to_string(),
            generated_group("baseline", binary_path, None),
        ),
        (
            "Stop".to_string(),
            generated_group("checkpoint", binary_path, None),
        ),
    ];
    if install_pretool {
        groups.push((
            "PreToolUse".to_string(),
            generated_group("pre-tool-use", binary_path, Some("Bash")),
        ));
    }
    let merged = merge_hooks_json(existing, &groups, hook_marker())?;
    if let Some(parent) = hooks_path.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::infra::atomic_write::atomic_write_bytes(
        &hooks_path,
        format!("{}\n", serde_json::to_string_pretty(&merged)?).as_bytes(),
    )?;
    Ok(())
}

fn generated_group(command: &str, binary_path: &str, matcher: Option<&str>) -> serde_json::Value {
    let base = shell_quote(binary_path);
    let mut group = json!({
        "hooks": [
            {
                "type": "command",
                "command": format!("{base} {} hook codex {command}", hook_marker()),
            }
        ]
    });
    if command == "checkpoint" {
        group["hooks"][0]["timeout"] = json!(120);
        group["hooks"][0]["statusMessage"] = json!("Evaluating Sprocket checkpoint state...");
    }
    if let Some(matcher) = matcher {
        group["matcher"] = json!(matcher);
    }
    group
}

fn install_prepare_commit_msg(git: &GitCli) -> Result<()> {
    let hook = br#"#!/usr/bin/env bash
set -euo pipefail

if [[ "${SPROCKET_ALLOW_COMMIT:-}" == "1" ]]; then
  exit 0
fi

echo "Commits are owned by Sprocket. Make code changes only." >&2
exit 1
"#;
    git.install_hook_file("prepare-commit-msg", hook)
}

fn shell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\"'\"'"))
}

fn worktree_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;

    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    canonical.as_os_str().as_bytes().to_vec()
}
