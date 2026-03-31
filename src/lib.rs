use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const SPROCKET_HOOK_MARKER: &str = "--sprocket-managed";
const DEFAULT_CHECKPOINT_MESSAGE: &str = "checkpoint({area}): save current work [auto]";
const DEFAULT_AREA: &str = "core";

pub fn run<I>(args: I) -> Result<()>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let _bin = args.next();
    dispatch(args.map(OsString::from).collect())
}

fn dispatch(args: Vec<OsString>) -> Result<()> {
    let filtered_args: Vec<OsString> = args
        .into_iter()
        .filter(|arg| arg != SPROCKET_HOOK_MARKER)
        .collect();

    if filtered_args.is_empty() {
        print_help();
        return Ok(());
    }

    match filtered_args[0].to_string_lossy().as_ref() {
        "install" => run_install(&filtered_args[1..]),
        "hook" => run_hook_command(&filtered_args[1..]),
        "--help" | "-h" => {
            print_help();
            Ok(())
        }
        "--version" | "-V" => {
            println!("sprocket {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "init" | "migrate" | "doctor" | "repair" | "validate" => {
            println!(
                "{} is not implemented yet.",
                filtered_args[0].to_string_lossy()
            );
            Ok(())
        }
        other => bail!("unknown command: {other}"),
    }
}

fn run_install(args: &[OsString]) -> Result<()> {
    if args.first().map(|arg| arg.to_string_lossy()) != Some("codex".into()) {
        bail!("usage: sprocket install codex [--target-repo <path>]");
    }

    let mut target_repo: Option<PathBuf> = None;
    let mut index = 1;
    while index < args.len() {
        match args[index].to_string_lossy().as_ref() {
            "--target-repo" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| anyhow!("--target-repo requires a path"))?;
                target_repo = Some(PathBuf::from(value));
            }
            unknown => bail!("unknown install argument: {unknown}"),
        }
        index += 1;
    }

    let repo = resolve_repo_from_target(target_repo.as_deref())?;
    install_codex_backend(&repo)?;
    println!("Installed Sprocket Codex adapter into {}", repo.display());
    Ok(())
}

fn run_hook_command(args: &[OsString]) -> Result<()> {
    if args.len() < 2 || args[0].to_string_lossy() != "codex" {
        bail!("usage: sprocket hook codex <baseline|pre-tool-use|checkpoint>");
    }

    let payload = read_payload()?;
    match args[1].to_string_lossy().as_ref() {
        "baseline" => run_codex_baseline(&payload),
        "pre-tool-use" => run_codex_pre_tool_use(&payload),
        "checkpoint" => run_codex_checkpoint(&payload),
        unknown => bail!("unknown codex hook subcommand: {unknown}"),
    }
}

fn print_help() {
    println!("Sprocket");
    println!("Agentic repo bootstrap and self-healing maintenance engine.");
    println!();
    println!("USAGE:");
    println!("  sprocket <COMMAND>");
    println!();
    println!("COMMANDS:");
    println!("  install codex              Install or update the Codex adapter");
    println!("  hook codex baseline        Capture a turn baseline");
    println!("  hook codex pre-tool-use    Block direct git mutations");
    println!("  hook codex checkpoint      Evaluate checkpoint state");
    println!("  init                       Not implemented yet");
    println!("  migrate                    Not implemented yet");
    println!("  doctor                     Not implemented yet");
    println!("  repair                     Not implemented yet");
    println!("  validate                   Not implemented yet");
}

fn install_codex_backend(repo: &Path) -> Result<()> {
    let binary_path =
        std::env::current_exe().context("failed to resolve current Sprocket binary path")?;
    ensure_sprocket_dirs(repo)?;
    write_or_update_config(repo, &binary_path)?;
    ensure_gitignore_rule(repo, ".sprocket/state/")?;
    merge_codex_hooks(repo, &binary_path)?;
    Ok(())
}

fn run_codex_baseline(payload: &Value) -> Result<()> {
    let repo = repo_root_from_payload(payload)?;
    let config = load_config(&repo)?;
    ensure_sprocket_dirs(&repo)?;

    let snapshot = meaningful_snapshot(&repo, &config.commit.owned_paths)?;
    let mut manager = load_manager_state(&repo)?;
    if manager.last_checkpoint_fingerprint.is_none() {
        manager.last_checkpoint_fingerprint = Some(snapshot.fingerprint.clone());
        manager.last_checkpoint_manifest = snapshot.manifest.clone();
        manager.last_checkpoint_commit = current_head(&repo)?;
        manager.last_checkpoint_at = Some(now_unix_seconds());
        save_manager_state(&repo, &manager)?;
    }

    let turn_id = turn_id_from_payload(payload);
    let turn_state = TurnState {
        version: 1,
        turn_id: turn_id.clone(),
        started_at: now_unix_seconds(),
        baseline_fingerprint: snapshot.fingerprint,
        baseline_manifest: snapshot.manifest,
    };
    save_turn_state(&repo, &turn_id, &turn_state)?;
    Ok(())
}

fn run_codex_pre_tool_use(payload: &Value) -> Result<()> {
    let command_text = extract_command_text(payload);
    if !should_block_git_command(command_text.as_deref()) {
        return Ok(());
    }

    println!(
        "{}",
        serde_json::to_string(&json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "Commits are owned by Sprocket. Make code changes only."
            }
        }))?
    );
    Ok(())
}

fn run_codex_checkpoint(payload: &Value) -> Result<()> {
    let repo = repo_root_from_payload(payload)?;
    let config = load_config(&repo)?;
    ensure_sprocket_dirs(&repo)?;

    let turn_id = turn_id_from_payload(payload);
    let Some((turn_path, turn_state)) = resolve_turn_state(&repo, &turn_id)? else {
        return Ok(());
    };

    let current_snapshot = meaningful_snapshot(&repo, &config.commit.owned_paths)?;
    if current_snapshot.fingerprint == turn_state.baseline_fingerprint {
        cleanup_turn_state(&turn_path)?;
        return Ok(());
    }

    let _lock = match acquire_lock(&repo, config.commit.lock_timeout_seconds)? {
        Some(lock) => lock,
        None => return Ok(()),
    };

    let mut manager = load_manager_state(&repo)?;
    if manager.last_checkpoint_fingerprint.as_deref() == Some(current_snapshot.fingerprint.as_str())
    {
        cleanup_turn_state(&turn_path)?;
        return Ok(());
    }

    let delta = diff_manifests(
        &manager.last_checkpoint_manifest,
        &current_snapshot.manifest,
    );
    if delta.changed_paths.is_empty() {
        cleanup_turn_state(&turn_path)?;
        return Ok(());
    }

    if !should_checkpoint(&delta, &manager, &config.commit) {
        record_pending_turn(&mut manager);
        save_manager_state(&repo, &manager)?;
        cleanup_turn_state(&turn_path)?;
        return Ok(());
    }

    let staged = stage_pathspecs(&repo, &config.commit.owned_paths)?;
    if !staged_changes_exist(&repo, &staged)? {
        cleanup_turn_state(&turn_path)?;
        return Ok(());
    }

    let message = render_commit_message(&config.commit);
    let Some(head) = commit_pathspecs(&repo, &message, &staged)? else {
        cleanup_turn_state(&turn_path)?;
        return Ok(());
    };

    manager.last_checkpoint_fingerprint = Some(current_snapshot.fingerprint);
    manager.last_checkpoint_manifest = current_snapshot.manifest;
    manager.last_checkpoint_commit = Some(head);
    manager.last_checkpoint_at = Some(now_unix_seconds());
    manager.pending_turn_count = 0;
    manager.pending_first_seen_at = None;
    manager.pending_last_seen_at = None;
    manager.generation += 1;
    save_manager_state(&repo, &manager)?;
    cleanup_turn_state(&turn_path)?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SprocketConfig {
    version: u32,
    backend: BackendConfig,
    commit: CommitConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackendConfig {
    codex: CodexBackendConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CodexBackendConfig {
    binary_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CommitConfig {
    owned_paths: Vec<String>,
    checkpoint_turn_threshold: u32,
    checkpoint_file_threshold: u32,
    checkpoint_age_minutes: u64,
    lock_timeout_seconds: u64,
    default_area: String,
    message_template: String,
}

impl Default for SprocketConfig {
    fn default() -> Self {
        Self {
            version: 1,
            backend: BackendConfig {
                codex: CodexBackendConfig {
                    binary_path: String::new(),
                },
            },
            commit: CommitConfig {
                owned_paths: vec!["src".into(), "tests".into()],
                checkpoint_turn_threshold: 2,
                checkpoint_file_threshold: 3,
                checkpoint_age_minutes: 20,
                lock_timeout_seconds: 300,
                default_area: DEFAULT_AREA.into(),
                message_template: DEFAULT_CHECKPOINT_MESSAGE.into(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ManagerState {
    version: u32,
    generation: u64,
    last_checkpoint_fingerprint: Option<String>,
    last_checkpoint_manifest: Vec<ManifestEntry>,
    last_checkpoint_commit: Option<String>,
    last_checkpoint_at: Option<u64>,
    pending_turn_count: u32,
    pending_first_seen_at: Option<u64>,
    pending_last_seen_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TurnState {
    version: u32,
    turn_id: String,
    started_at: u64,
    baseline_fingerprint: String,
    baseline_manifest: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ManifestEntry {
    path: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256: Option<String>,
}

#[derive(Debug, Clone)]
struct Snapshot {
    manifest: Vec<ManifestEntry>,
    fingerprint: String,
}

#[derive(Debug, Clone, Default)]
struct Delta {
    added: Vec<String>,
    modified: Vec<String>,
    deleted: Vec<String>,
    changed_paths: Vec<String>,
}

fn write_or_update_config(repo: &Path, binary_path: &Path) -> Result<()> {
    let path = repo.join(".sprocket/sprocket.toml");
    let mut config = if path.exists() {
        load_config(repo)?
    } else {
        SprocketConfig::default()
    };
    config.backend.codex.binary_path = binary_path.display().to_string();
    let content = toml::to_string_pretty(&config)?;
    write_text(&path, &(content + "\n"))?;
    Ok(())
}

fn load_config(repo: &Path) -> Result<SprocketConfig> {
    let path = repo.join(".sprocket/sprocket.toml");
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read config at {}", path.display()))?;
    let mut config: SprocketConfig = toml::from_str(&content)
        .with_context(|| format!("failed to parse config at {}", path.display()))?;
    if config.commit.owned_paths.is_empty() {
        config.commit.owned_paths = SprocketConfig::default().commit.owned_paths;
    }
    if config.commit.default_area.is_empty() {
        config.commit.default_area = DEFAULT_AREA.into();
    }
    if config.commit.message_template.is_empty() {
        config.commit.message_template = DEFAULT_CHECKPOINT_MESSAGE.into();
    }
    Ok(config)
}

fn merge_codex_hooks(repo: &Path, binary_path: &Path) -> Result<()> {
    let hooks_path = repo.join(".codex/hooks.json");
    let mut root = if hooks_path.exists() {
        let raw = fs::read_to_string(&hooks_path)?;
        serde_json::from_str::<Value>(&raw).with_context(|| {
            format!(
                "existing hooks.json is not valid JSON: {}",
                hooks_path.display()
            )
        })?
    } else {
        json!({})
    };

    let hooks = root
        .as_object_mut()
        .ok_or_else(|| anyhow!("hooks.json root must be a JSON object"))?
        .entry("hooks")
        .or_insert_with(|| json!({}));

    let hooks_object = hooks
        .as_object_mut()
        .ok_or_else(|| anyhow!("hooks.json `hooks` value must be a JSON object"))?;

    for event in ["UserPromptSubmit", "PreToolUse", "Stop"] {
        let current = hooks_object
            .entry(event.to_string())
            .or_insert_with(|| Value::Array(vec![]));
        let groups = current
            .as_array_mut()
            .ok_or_else(|| anyhow!("hooks.json event `{event}` must be an array"))?;
        groups.retain(|group| !group_contains_sprocket_marker(group));
        groups.push(generated_hook_group(event, binary_path));
    }

    let content = serde_json::to_string_pretty(&root)?;
    write_text(&hooks_path, &(content + "\n"))?;
    Ok(())
}

fn generated_hook_group(event: &str, binary_path: &Path) -> Value {
    let base = shell_quote(binary_path);
    match event {
        "UserPromptSubmit" => json!({
            "hooks": [
                {
                    "type": "command",
                    "command": format!("{base} {SPROCKET_HOOK_MARKER} hook codex baseline")
                }
            ]
        }),
        "PreToolUse" => json!({
            "matcher": "Bash",
            "hooks": [
                {
                    "type": "command",
                    "command": format!("{base} {SPROCKET_HOOK_MARKER} hook codex pre-tool-use")
                }
            ]
        }),
        "Stop" => json!({
            "hooks": [
                {
                    "type": "command",
                    "command": format!("{base} {SPROCKET_HOOK_MARKER} hook codex checkpoint"),
                    "timeout": 120,
                    "statusMessage": "Evaluating Sprocket checkpoint state..."
                }
            ]
        }),
        _ => unreachable!(),
    }
}

fn group_contains_sprocket_marker(group: &Value) -> bool {
    let Some(hooks) = group.get("hooks").and_then(Value::as_array) else {
        return false;
    };
    hooks.iter().any(|hook| {
        hook.get("command")
            .and_then(Value::as_str)
            .is_some_and(|command| command.contains(SPROCKET_HOOK_MARKER))
    })
}

fn ensure_gitignore_rule(repo: &Path, rule: &str) -> Result<()> {
    let path = repo.join(".gitignore");
    let mut lines = if path.exists() {
        fs::read_to_string(&path)?
    } else {
        String::new()
    };
    if !lines.lines().any(|line| line.trim() == rule) {
        if !lines.is_empty() && !lines.ends_with('\n') {
            lines.push('\n');
        }
        lines.push_str(rule);
        lines.push('\n');
        write_text(&path, &lines)?;
    }
    Ok(())
}

fn meaningful_snapshot(repo: &Path, owned_paths: &[String]) -> Result<Snapshot> {
    let tracked =
        split_nul_list(git_command(repo, build_git_args("ls-files", &["-z"], owned_paths))?.stdout);
    let deleted = split_nul_list(
        git_command(repo, build_git_args("ls-files", &["-d", "-z"], owned_paths))?.stdout,
    );
    let untracked = split_nul_list(
        git_command(
            repo,
            build_git_args(
                "ls-files",
                &["--others", "--exclude-standard", "-z"],
                owned_paths,
            ),
        )?
        .stdout,
    );

    let deleted_set: HashSet<String> = deleted.into_iter().collect();
    let mut present_paths: Vec<String> = tracked
        .into_iter()
        .chain(untracked)
        .filter(|path| !deleted_set.contains(path))
        .collect();
    present_paths.sort();
    present_paths.dedup();

    let mut manifest = Vec::new();
    for relative_path in present_paths {
        let file_path = repo.join(&relative_path);
        if !file_path.exists() {
            continue;
        }
        let digest = sha256_hex(&fs::read(&file_path)?);
        manifest.push(ManifestEntry {
            path: relative_path,
            status: "present".into(),
            sha256: Some(digest),
        });
    }

    for relative_path in deleted_set {
        manifest.push(ManifestEntry {
            path: relative_path,
            status: "deleted".into(),
            sha256: None,
        });
    }

    manifest.sort_by(|a, b| a.path.cmp(&b.path));
    let serialized = serde_json::to_vec(&manifest)?;
    Ok(Snapshot {
        manifest,
        fingerprint: format!("sha256:{}", sha256_hex(&serialized)),
    })
}

fn build_git_args<'a>(
    subcommand: &'a str,
    extra: &[&'a str],
    owned_paths: &'a [String],
) -> Vec<&'a str> {
    let mut args = vec![subcommand];
    args.extend_from_slice(extra);
    args.push("--");
    for path in owned_paths {
        args.push(path);
    }
    args
}

fn diff_manifests(old_manifest: &[ManifestEntry], new_manifest: &[ManifestEntry]) -> Delta {
    let old_map: BTreeMap<_, _> = old_manifest
        .iter()
        .map(|entry| (&entry.path, entry))
        .collect();
    let new_map: BTreeMap<_, _> = new_manifest
        .iter()
        .map(|entry| (&entry.path, entry))
        .collect();
    let mut delta = Delta::default();

    let all_paths: HashSet<&String> = old_map.keys().chain(new_map.keys()).copied().collect();
    let mut sorted_paths: Vec<&String> = all_paths.into_iter().collect();
    sorted_paths.sort();

    for path in sorted_paths {
        match (old_map.get(path), new_map.get(path)) {
            (None, Some(new_entry)) => {
                if new_entry.status == "deleted" {
                    delta.deleted.push(path.clone());
                } else {
                    delta.added.push(path.clone());
                }
            }
            (Some(old_entry), None) => {
                if old_entry.status != "deleted" {
                    delta.deleted.push(path.clone());
                }
            }
            (Some(old_entry), Some(new_entry)) if old_entry != new_entry => {
                if old_entry.status != "deleted" && new_entry.status == "deleted" {
                    delta.deleted.push(path.clone());
                } else if old_entry.status == "deleted" && new_entry.status != "deleted" {
                    delta.added.push(path.clone());
                } else {
                    delta.modified.push(path.clone());
                }
            }
            _ => {}
        }
    }

    delta.changed_paths = delta
        .added
        .iter()
        .chain(&delta.modified)
        .chain(&delta.deleted)
        .cloned()
        .collect();
    delta.changed_paths.sort();
    delta.changed_paths.dedup();
    delta
}

fn should_checkpoint(delta: &Delta, manager: &ManagerState, config: &CommitConfig) -> bool {
    if delta.changed_paths.is_empty() {
        return false;
    }
    if manager.pending_turn_count + 1 >= config.checkpoint_turn_threshold {
        return true;
    }
    if delta.changed_paths.len() as u32 >= config.checkpoint_file_threshold {
        return true;
    }
    let Some(last_checkpoint_at) = manager.last_checkpoint_at else {
        return false;
    };
    now_unix_seconds().saturating_sub(last_checkpoint_at)
        >= config.checkpoint_age_minutes.saturating_mul(60)
}

fn record_pending_turn(manager: &mut ManagerState) {
    let now = now_unix_seconds();
    if manager.pending_first_seen_at.is_none() {
        manager.pending_first_seen_at = Some(now);
    }
    manager.pending_last_seen_at = Some(now);
    manager.pending_turn_count += 1;
}

fn render_commit_message(config: &CommitConfig) -> String {
    config
        .message_template
        .replace("{area}", &config.default_area)
}

fn stage_pathspecs(repo: &Path, pathspecs: &[String]) -> Result<Vec<String>> {
    let selected = relevant_pathspecs(repo, pathspecs)?;
    if selected.is_empty() {
        return Ok(vec![]);
    }
    let mut args = vec!["add".to_string(), "-A".to_string(), "--".to_string()];
    args.extend(selected.iter().cloned());
    git_command_owned(repo, &args)?;
    Ok(selected)
}

fn relevant_pathspecs(repo: &Path, pathspecs: &[String]) -> Result<Vec<String>> {
    let mut selected = Vec::new();
    for pathspec in pathspecs {
        if repo.join(pathspec).exists() || path_has_status(repo, pathspec)? {
            selected.push(pathspec.clone());
        }
    }
    Ok(selected)
}

fn path_has_status(repo: &Path, pathspec: &str) -> Result<bool> {
    let literal_pathspec = format!(":(literal){pathspec}");
    let result = git_command_owned(
        repo,
        &[
            "status".into(),
            "--porcelain=v1".into(),
            "--untracked-files=all".into(),
            "--".into(),
            literal_pathspec,
        ],
    )?;
    Ok(!result.stdout.is_empty())
}

fn staged_changes_exist(repo: &Path, pathspecs: &[String]) -> Result<bool> {
    if pathspecs.is_empty() {
        return Ok(false);
    }
    let mut args = vec![
        "diff".into(),
        "--cached".into(),
        "--quiet".into(),
        "--".into(),
    ];
    args.extend(pathspecs.iter().cloned());
    let status = git_status(repo, &args)?;
    Ok(status == 1)
}

fn commit_pathspecs(repo: &Path, message: &str, pathspecs: &[String]) -> Result<Option<String>> {
    if pathspecs.is_empty() {
        return Ok(None);
    }

    let mut diff_args = vec![
        "diff".into(),
        "--cached".into(),
        "--name-only".into(),
        "-z".into(),
        "--".into(),
    ];
    diff_args.extend(pathspecs.iter().cloned());
    let result = git_command_owned(repo, &diff_args)?;
    let files = split_nul_list(result.stdout);
    if files.is_empty() {
        return Ok(None);
    }

    let mut args = vec![
        "commit".into(),
        "--no-verify".into(),
        "-m".into(),
        message.into(),
        "--".into(),
    ];
    args.extend(files);
    let output = git_command_capture(repo, &args)?;
    if output.status != 0 {
        let text = format!("{}{}", output.stdout, output.stderr);
        if text.contains("nothing to commit") {
            return Ok(None);
        }
        bail!(text.trim().to_string());
    }

    Ok(current_head(repo)?)
}

fn current_head(repo: &Path) -> Result<Option<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("rev-parse")
        .arg("--short=12")
        .arg("HEAD")
        .output()
        .context("failed to read current git HEAD")?;
    if !output.status.success() {
        return Ok(None);
    }
    let value = String::from_utf8(output.stdout)?.trim().to_string();
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

fn resolve_turn_state(repo: &Path, turn_id: &str) -> Result<Option<(PathBuf, TurnState)>> {
    let exact_path = turn_state_path(repo, turn_id);
    if exact_path.exists() {
        return Ok(Some((exact_path.clone(), read_json(&exact_path)?)));
    }
    let turns_dir = turns_dir(repo);
    let mut candidates: Vec<_> = fs::read_dir(&turns_dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default();
    candidates.sort_by_key(|path| fs::metadata(path).and_then(|meta| meta.modified()).ok());
    let Some(path) = candidates.pop() else {
        return Ok(None);
    };
    Ok(Some((path.clone(), read_json(&path)?)))
}

fn save_turn_state(repo: &Path, turn_id: &str, turn_state: &TurnState) -> Result<()> {
    write_json(&turn_state_path(repo, turn_id), turn_state)
}

fn cleanup_turn_state(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn load_manager_state(repo: &Path) -> Result<ManagerState> {
    let path = manager_state_path(repo);
    if !path.exists() {
        return Ok(ManagerState {
            version: 1,
            ..ManagerState::default()
        });
    }
    read_json(&path)
}

fn save_manager_state(repo: &Path, manager: &ManagerState) -> Result<()> {
    write_json(&manager_state_path(repo), manager)
}

fn acquire_lock(repo: &Path, timeout_seconds: u64) -> Result<Option<LockGuard>> {
    let path = lock_path(repo);
    if let Ok(metadata) = fs::metadata(&path) {
        let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
        let age = SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default()
            .as_secs();
        if age >= timeout_seconds {
            let _ = fs::remove_file(&path);
        }
    }

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o644);
    match options.open(&path) {
        Ok(mut file) => {
            use std::io::Write;
            let payload = json!({
                "pid": std::process::id(),
                "acquired_at": now_unix_seconds(),
            });
            file.write_all(serde_json::to_string(&payload)?.as_bytes())?;
            file.write_all(b"\n")?;
            Ok(Some(LockGuard { path }))
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(None),
        Err(error) => Err(error.into()),
    }
}

struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn should_block_git_command(command_text: Option<&str>) -> bool {
    let Some(command_text) = command_text else {
        return false;
    };
    let Some(argv) = shlex::split(command_text) else {
        return false;
    };
    let Some(subcommand) = git_subcommand(&argv) else {
        return false;
    };
    if matches!(
        subcommand,
        "add" | "commit" | "merge" | "rebase" | "cherry-pick" | "push" | "tag" | "stash" | "am"
    ) {
        return true;
    }
    subcommand == "reset" && argv.iter().any(|arg| arg == "--hard")
}

fn git_subcommand(argv: &[String]) -> Option<&str> {
    if argv.first().map(String::as_str) != Some("git") {
        return None;
    }
    let mut index = 1;
    while index < argv.len() {
        let token = argv[index].as_str();
        if matches!(
            token,
            "-c" | "-C" | "--git-dir" | "--work-tree" | "--namespace" | "--super-prefix"
        ) {
            index += 2;
            continue;
        }
        if token.starts_with("--") || (token.starts_with('-') && token.len() > 1) {
            index += 1;
            continue;
        }
        return Some(token);
    }
    None
}

fn extract_command_text(payload: &Value) -> Option<String> {
    for (parent, child) in [
        ("tool_input", "command"),
        ("toolInput", "command"),
        ("input", "command"),
        ("tool_input", "cmd"),
        ("toolInput", "cmd"),
        ("input", "cmd"),
    ] {
        if let Some(value) = payload
            .get(parent)
            .and_then(|parent_value| parent_value.get(child))
            .and_then(Value::as_str)
        {
            return Some(value.to_string());
        }
    }
    find_first_string(payload, &["command", "cmd"]).map(ToOwned::to_owned)
}

fn read_payload() -> Result<Value> {
    let mut raw = String::new();
    io::stdin().read_to_string(&mut raw)?;
    if raw.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    Ok(serde_json::from_str(&raw)?)
}

fn repo_root_from_payload(payload: &Value) -> Result<PathBuf> {
    let cwd = find_first_string(payload, &["cwd", "working_dir", "workingDirectory"])
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    resolve_repo_from_target(Some(&cwd))
}

fn resolve_repo_from_target(target: Option<&Path>) -> Result<PathBuf> {
    let cwd = target
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir().context("failed to resolve current directory")?);
    let output = Command::new("git")
        .arg("-C")
        .arg(&cwd)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output()
        .with_context(|| format!("failed to resolve git repo from {}", cwd.display()))?;
    if !output.status.success() {
        bail!("target is not inside a git repository: {}", cwd.display());
    }
    Ok(PathBuf::from(String::from_utf8(output.stdout)?.trim()))
}

fn turn_id_from_payload(payload: &Value) -> String {
    find_first_string(
        payload,
        &[
            "turn_id",
            "turnId",
            "conversationTurnId",
            "request_id",
            "requestId",
        ],
    )
    .map(sanitize_identifier)
    .unwrap_or_else(|| "current".into())
}

fn sanitize_identifier(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|char| {
            if char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | '-') {
                char
            } else {
                '_'
            }
        })
        .collect();
    cleaned
        .trim_matches(&['.', '_', '-'][..])
        .to_string()
        .chars()
        .collect::<String>()
        .if_empty_then("current")
}

trait EmptyFallback {
    fn if_empty_then(self, fallback: &str) -> String;
}

impl EmptyFallback for String {
    fn if_empty_then(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.into()
        } else {
            self
        }
    }
}

fn find_first_string<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(string) = map.get(*key).and_then(Value::as_str) {
                    return Some(string);
                }
            }
            for nested in map.values() {
                if let Some(found) = find_first_string(nested, keys) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(|item| find_first_string(item, keys)),
        _ => None,
    }
}

fn git_command(repo: &Path, args: impl IntoIterator<Item = impl AsRef<str>>) -> Result<GitOutput> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo);
    for arg in args {
        command.arg(arg.as_ref());
    }
    let output = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    if !output.status.success() {
        bail!(
            "{}",
            format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
            .trim()
        );
    }
    Ok(GitOutput {
        status: output.status.code().unwrap_or(1),
        stdout: String::from_utf8(output.stdout)?,
        stderr: String::from_utf8(output.stderr)?,
    })
}

fn git_command_owned(repo: &Path, args: &[String]) -> Result<GitOutput> {
    git_command(repo, args.iter().map(String::as_str))
}

fn git_command_capture(repo: &Path, args: &[String]) -> Result<GitOutput> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo);
    command.args(args);
    let output = command.output()?;
    Ok(GitOutput {
        status: output.status.code().unwrap_or(1),
        stdout: String::from_utf8(output.stdout)?,
        stderr: String::from_utf8(output.stderr)?,
    })
}

fn git_status(repo: &Path, args: &[String]) -> Result<i32> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo);
    command.args(args);
    let status = command.status()?;
    Ok(status.code().unwrap_or(1))
}

struct GitOutput {
    status: i32,
    stdout: String,
    stderr: String,
}

fn ensure_sprocket_dirs(repo: &Path) -> Result<()> {
    fs::create_dir_all(turns_dir(repo))?;
    Ok(())
}

fn write_text(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(())
}

fn write_json<T: Serialize>(path: &Path, payload: &T) -> Result<()> {
    let content = serde_json::to_vec_pretty(payload)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, [content, b"\n".to_vec()].concat())?;
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let content = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content)?)
}

fn turns_dir(repo: &Path) -> PathBuf {
    repo.join(".sprocket/state/checkpoint/turns")
}

fn manager_state_path(repo: &Path) -> PathBuf {
    repo.join(".sprocket/state/checkpoint/manager.json")
}

fn lock_path(repo: &Path) -> PathBuf {
    repo.join(".sprocket/state/checkpoint/lock")
}

fn turn_state_path(repo: &Path, turn_id: &str) -> PathBuf {
    turns_dir(repo).join(format!("{}.json", sanitize_identifier(turn_id)))
}

fn split_nul_list(value: String) -> Vec<String> {
    value
        .split('\0')
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn shell_quote(path: &Path) -> String {
    let value = path.display().to_string().replace('\'', "'\"'\"'");
    format!("'{value}'")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    use tempfile::TempDir;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn init_repo() -> (TempDir, PathBuf) {
        let temp = TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        Command::new("git").arg("init").arg(&repo).status().unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["config", "user.name", "Codex Test"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["config", "user.email", "codex@example.com"])
            .status()
            .unwrap();
        write(&repo.join("src/main.py"), "print('hi')\n");
        write(
            &repo.join("tests/test_main.py"),
            "def test_ok():\n    assert True\n",
        );
        write(&repo.join("README.md"), "# Demo\n");
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["add", "."])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["commit", "-m", "base"])
            .status()
            .unwrap();
        (temp, repo)
    }

    fn payload(repo: &Path, pairs: &[(&str, &str)]) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("cwd".into(), Value::String(repo.display().to_string()));
        for (key, value) in pairs {
            map.insert((*key).into(), Value::String((*value).into()));
        }
        Value::Object(map)
    }

    fn set_fast_policy(repo: &Path) {
        let mut config = load_config(repo).unwrap();
        config.commit.checkpoint_turn_threshold = 1;
        config.commit.checkpoint_file_threshold = 1;
        config.commit.checkpoint_age_minutes = 0;
        write_text(
            &repo.join(".sprocket/sprocket.toml"),
            &(toml::to_string_pretty(&config).unwrap() + "\n"),
        )
        .unwrap();
    }

    #[test]
    fn install_creates_codex_hooks_and_sprocket_config() {
        let (_temp, repo) = init_repo();
        install_codex_backend(&repo).unwrap();

        let hooks: Value =
            serde_json::from_str(&fs::read_to_string(repo.join(".codex/hooks.json")).unwrap())
                .unwrap();
        let hooks_object = hooks.get("hooks").unwrap().as_object().unwrap();
        assert!(hooks_object.contains_key("UserPromptSubmit"));
        assert!(hooks_object.contains_key("PreToolUse"));
        assert!(hooks_object.contains_key("Stop"));
        assert!(repo.join(".sprocket/sprocket.toml").exists());
        assert!(repo.join(".sprocket/state/checkpoint/turns").is_dir());
    }

    #[test]
    fn install_merges_existing_hooks_safely() {
        let (_temp, repo) = init_repo();
        write(
            &repo.join(".codex/hooks.json"),
            r#"{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "python3 /tmp/custom_stop.py"
          }
        ]
      }
    ]
  }
}
"#,
        );

        install_codex_backend(&repo).unwrap();
        install_codex_backend(&repo).unwrap();

        let hooks_text = fs::read_to_string(repo.join(".codex/hooks.json")).unwrap();
        assert!(hooks_text.contains("/tmp/custom_stop.py"));
        let hooks: Value = serde_json::from_str(&hooks_text).unwrap();
        let stop_groups = hooks
            .get("hooks")
            .and_then(|value| value.get("Stop"))
            .and_then(Value::as_array)
            .unwrap();
        let managed_stop_groups = stop_groups
            .iter()
            .filter(|group| group_contains_sprocket_marker(group))
            .count();
        assert_eq!(managed_stop_groups, 1);
    }

    #[test]
    fn no_changes_after_baseline_creates_no_commit() {
        let (_temp, repo) = init_repo();
        install_codex_backend(&repo).unwrap();

        let before = current_head(&repo).unwrap().unwrap();
        run_codex_baseline(&payload(&repo, &[("turnId", "turn-1")])).unwrap();
        run_codex_checkpoint(&payload(&repo, &[("turnId", "turn-1")])).unwrap();
        let after = current_head(&repo).unwrap().unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn owned_changes_create_local_checkpoint_commit() {
        let (_temp, repo) = init_repo();
        install_codex_backend(&repo).unwrap();
        set_fast_policy(&repo);

        run_codex_baseline(&payload(&repo, &[("turnId", "turn-1")])).unwrap();
        write(&repo.join("src/main.py"), "print('updated')\n");
        run_codex_checkpoint(&payload(&repo, &[("turnId", "turn-1")])).unwrap();

        let subject = git_command(&repo, ["log", "-1", "--format=%s"])
            .unwrap()
            .stdout;
        assert_eq!(subject.trim(), "checkpoint(core): save current work [auto]");
    }

    #[test]
    fn unowned_changes_are_not_committed() {
        let (_temp, repo) = init_repo();
        install_codex_backend(&repo).unwrap();
        set_fast_policy(&repo);

        let before = current_head(&repo).unwrap().unwrap();
        run_codex_baseline(&payload(&repo, &[("turnId", "turn-1")])).unwrap();
        write(&repo.join("notes.txt"), "unowned\n");
        run_codex_checkpoint(&payload(&repo, &[("turnId", "turn-1")])).unwrap();
        let after = current_head(&repo).unwrap().unwrap();

        assert_eq!(before, after);
        let status = git_command(&repo, ["status", "--short", "--", "notes.txt"])
            .unwrap()
            .stdout;
        assert!(status.contains("?? notes.txt"));
    }

    #[test]
    fn pre_tool_use_blocks_git_commit_but_not_status() {
        let blocked = should_block_git_command(Some("git commit -m test"));
        let allowed = should_block_git_command(Some("git status --short"));
        assert!(blocked);
        assert!(!allowed);
    }

    #[test]
    fn live_lock_prevents_competing_checkpoint_attempt() {
        let (_temp, repo) = init_repo();
        install_codex_backend(&repo).unwrap();
        set_fast_policy(&repo);
        run_codex_baseline(&payload(&repo, &[("turnId", "turn-1")])).unwrap();
        write(&repo.join("src/main.py"), "print('updated')\n");

        let guard = acquire_lock(&repo, 300).unwrap().unwrap();
        run_codex_checkpoint(&payload(&repo, &[("turnId", "turn-1")])).unwrap();
        drop(guard);

        let subject = git_command(&repo, ["log", "-1", "--format=%s"])
            .unwrap()
            .stdout;
        assert_eq!(subject.trim(), "base");
    }

    #[test]
    fn state_is_written_under_sprocket_not_codex() {
        let (_temp, repo) = init_repo();
        install_codex_backend(&repo).unwrap();
        run_codex_baseline(&payload(&repo, &[("turnId", "turn-1")])).unwrap();

        assert!(
            repo.join(".sprocket/state/checkpoint/manager.json")
                .exists()
        );
        assert!(
            repo.join(".sprocket/state/checkpoint/turns/turn-1.json")
                .exists()
        );
        assert!(!repo.join(".codex/state").exists());
    }

    #[test]
    fn stale_lock_is_replaced() {
        let (_temp, repo) = init_repo();
        install_codex_backend(&repo).unwrap();
        let path = lock_path(&repo);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{}").unwrap();
        thread::sleep(Duration::from_millis(2100));
        let guard = acquire_lock(&repo, 1).unwrap();
        assert!(guard.is_some());
    }
}
