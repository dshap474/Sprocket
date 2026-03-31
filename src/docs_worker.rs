use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::json;
use wait_timeout::ChildExt;

use crate::classification::Delta;
use crate::config::SprocketConfig;

const DEFAULT_DOCS_INSTRUCTIONS: &str = r#"You are Sprocket's milestone documentation worker.

Treat this as a pre-commit docs maintenance pass.

Rules:
- You may edit only the managed docs outputs listed below.
- Keep docs grounded in code that actually exists in the repository.
- Recreate missing managed docs only when the config says that is allowed.
- Update only stale or missing sections.
- No-op cleanly when the docs are already current.
- Do not touch code, tests, /.sprocket/state/, /.codex/hooks.json, or unrelated docs.
"#;

const PROJECT_RULES: &str = r#"# Project Docs Rules

`docs/project/llms/llms.txt` is the agent-first navigation index.

Keep it:
- short
- descriptive
- navigation-oriented
- grounded in the real repo layout

It should help an agent quickly answer:
- what this project is
- where the main systems live
- which docs are the best starting points
"#;

const ARCHITECTURE_RULES: &str = r#"# Architecture Docs Rules

`docs/ARCHITECTURE.md` should explain the real runtime shape of the repository.

Keep it:
- specific to code that exists now
- organized around major systems and flows
- clear about generated or managed repo surfaces
- free of aspirational architecture that is not implemented
"#;

pub(crate) fn install_managed_rules(repo: &Path) -> Result<()> {
    super::write_text(&repo.join(".sprocket/rules/project.md"), PROJECT_RULES)?;
    super::write_text(
        &repo.join(".sprocket/rules/architecture.md"),
        ARCHITECTURE_RULES,
    )?;
    Ok(())
}

pub(crate) fn backup_docs_outputs(
    repo: &Path,
    managed_outputs: &[String],
) -> Result<BTreeMap<String, Option<Vec<u8>>>> {
    let mut backups = BTreeMap::new();
    for relative_path in managed_outputs {
        let path = repo.join(relative_path);
        let contents = if path.exists() {
            Some(fs::read(&path)?)
        } else {
            None
        };
        backups.insert(relative_path.clone(), contents);
    }
    Ok(backups)
}

pub(crate) fn restore_docs_outputs(
    repo: &Path,
    backups: &BTreeMap<String, Option<Vec<u8>>>,
) -> Result<()> {
    for (relative_path, contents) in backups {
        let path = repo.join(relative_path);
        match contents {
            Some(bytes) => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(path, bytes)?;
            }
            None => {
                if path.exists() {
                    fs::remove_file(path)?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn run_docs_worker(
    repo: &Path,
    delta: &Delta,
    config: &SprocketConfig,
) -> Result<(bool, String)> {
    let docs = &config.docs;
    let prompt = build_docs_prompt(repo, delta, config)?;
    let codex_bin = std::env::var("SPROCKET_CODEX_BIN").unwrap_or_else(|_| "codex".into());
    let mut command = Command::new(codex_bin);
    command
        .arg("exec")
        .arg("--ephemeral")
        .current_dir(repo)
        .arg("--ask-for-approval")
        .arg(&docs.approval)
        .arg("--sandbox")
        .arg(&docs.sandbox)
        .arg("-m")
        .arg(&docs.model)
        .arg("-c")
        .arg(format!(
            "model_reasoning_effort=\"{}\"",
            docs.reasoning_effort
        ))
        .arg("-C")
        .arg(repo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if docs.disable_codex_hooks {
        command.arg("--disable").arg("codex_hooks");
    }
    command.arg(prompt);

    let mut child = command.spawn().context("failed to launch codex exec")?;
    let timeout = Duration::from_secs(docs.timeout_seconds);
    let status = match child.wait_timeout(timeout)? {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Ok((false, "docs worker timed out".into()));
        }
    };
    let output = child.wait_with_output()?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .trim()
    .to_string();
    if status.success() {
        Ok((true, combined))
    } else {
        Ok((
            false,
            if combined.is_empty() {
                "docs worker failed".into()
            } else {
                combined
            },
        ))
    }
}

fn build_docs_prompt(repo: &Path, delta: &Delta, config: &SprocketConfig) -> Result<String> {
    let instructions = if config.docs.instructions.trim().is_empty() {
        DEFAULT_DOCS_INSTRUCTIONS.to_string()
    } else {
        config.docs.instructions.clone()
    };
    let project_rules = read_rule_or_default(repo, "project.md", PROJECT_RULES)?;
    let architecture_rules = read_rule_or_default(repo, "architecture.md", ARCHITECTURE_RULES)?;
    let summary = json!({
        "managed_outputs": config.docs.managed_outputs,
        "recreate_missing": config.docs.recreate_missing,
        "changed_paths": delta.changed_paths,
        "added": delta.added,
        "modified": delta.modified,
        "deleted": delta.deleted,
    });
    Ok(format!(
        "{instructions}\n\nManaged docs outputs:\n- {}\n\nProject rules:\n{project_rules}\n\nArchitecture rules:\n{architecture_rules}\n\nChanged repo state summary:\n{}\n",
        config.docs.managed_outputs.join("\n- "),
        serde_json::to_string_pretty(&summary)?
    ))
}

fn read_rule_or_default(repo: &Path, file_name: &str, fallback: &str) -> Result<String> {
    let path = repo.join(".sprocket/rules").join(file_name);
    if !path.exists() {
        return Ok(fallback.into());
    }
    Ok(fs::read_to_string(path)?)
}
