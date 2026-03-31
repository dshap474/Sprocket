use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub(crate) const DEFAULT_CHECKPOINT_MESSAGE: &str = "checkpoint({area}): save current work [auto]";
pub(crate) const DEFAULT_MILESTONE_MESSAGE: &str =
    "milestone({area}): sync docs and save current work [auto]";
pub(crate) const DEFAULT_AREA: &str = "core";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct SprocketConfig {
    pub(crate) version: u32,
    pub(crate) backend: BackendConfig,
    pub(crate) commit: CommitConfig,
    pub(crate) docs: DocsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub(crate) struct BackendConfig {
    pub(crate) codex: CodexBackendConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub(crate) struct CodexBackendConfig {
    pub(crate) binary_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct CommitConfig {
    pub(crate) owned_paths: Vec<String>,
    pub(crate) checkpoint_turn_threshold: u32,
    pub(crate) checkpoint_file_threshold: u32,
    pub(crate) checkpoint_age_minutes: u64,
    pub(crate) milestone_file_threshold: u32,
    pub(crate) lock_timeout_seconds: u64,
    pub(crate) default_area: String,
    #[serde(alias = "message_template")]
    pub(crate) checkpoint_message_template: String,
    pub(crate) milestone_message_template: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct DocsConfig {
    pub(crate) enabled: bool,
    pub(crate) managed_outputs: Vec<String>,
    pub(crate) timeout_seconds: u64,
    pub(crate) model: String,
    pub(crate) reasoning_effort: String,
    pub(crate) sandbox: String,
    pub(crate) approval: String,
    pub(crate) disable_codex_hooks: bool,
    pub(crate) recreate_missing: bool,
    pub(crate) instructions: String,
    pub(crate) triggers: DocsTriggerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct DocsTriggerConfig {
    pub(crate) source_roots: Vec<String>,
    pub(crate) test_roots: Vec<String>,
    pub(crate) milestone_globs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommitKind {
    None,
    Checkpoint,
    Milestone,
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
            commit: CommitConfig::default(),
            docs: DocsConfig::default(),
        }
    }
}

impl Default for CommitConfig {
    fn default() -> Self {
        Self {
            owned_paths: vec!["src".into(), "tests".into()],
            checkpoint_turn_threshold: 2,
            checkpoint_file_threshold: 3,
            checkpoint_age_minutes: 20,
            milestone_file_threshold: 6,
            lock_timeout_seconds: 300,
            default_area: DEFAULT_AREA.into(),
            checkpoint_message_template: DEFAULT_CHECKPOINT_MESSAGE.into(),
            milestone_message_template: DEFAULT_MILESTONE_MESSAGE.into(),
        }
    }
}

impl Default for DocsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            managed_outputs: vec![
                "docs/ARCHITECTURE.md".into(),
                "docs/project/llms/llms.txt".into(),
            ],
            timeout_seconds: 90,
            model: "gpt-5.3-codex-spark".into(),
            reasoning_effort: "medium".into(),
            sandbox: "workspace-write".into(),
            approval: "never".into(),
            disable_codex_hooks: true,
            recreate_missing: true,
            instructions: String::new(),
            triggers: DocsTriggerConfig::default(),
        }
    }
}

impl Default for DocsTriggerConfig {
    fn default() -> Self {
        Self {
            source_roots: vec!["src".into()],
            test_roots: vec!["tests".into()],
            milestone_globs: vec![
                "pyproject.toml".into(),
                "package.json".into(),
                "Cargo.toml".into(),
                "justfile".into(),
                "Makefile".into(),
                "Dockerfile".into(),
                ".github/workflows/**".into(),
            ],
        }
    }
}

pub(crate) fn write_or_update_config(repo: &Path, binary_path: &Path) -> Result<()> {
    let path = repo.join(".sprocket/sprocket.toml");
    let mut config = if path.exists() {
        load_config(repo)?
    } else {
        SprocketConfig::default()
    };
    config.backend.codex.binary_path = binary_path.display().to_string();
    let content = toml::to_string_pretty(&config)?;
    super::write_text(&path, &(content + "\n"))?;
    Ok(())
}

pub(crate) fn load_config(repo: &Path) -> Result<SprocketConfig> {
    let path = repo.join(".sprocket/sprocket.toml");
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read config at {}", path.display()))?;
    let mut config: SprocketConfig = toml::from_str(&content)
        .with_context(|| format!("failed to parse config at {}", path.display()))?;
    apply_defaults(&mut config);
    validate_config(&config)?;
    Ok(config)
}

pub(crate) fn render_commit_message(commit: &CommitConfig, kind: CommitKind) -> String {
    let template = match kind {
        CommitKind::Milestone => &commit.milestone_message_template,
        CommitKind::Checkpoint | CommitKind::None => &commit.checkpoint_message_template,
    };
    template.replace("{area}", &commit.default_area)
}

fn apply_defaults(config: &mut SprocketConfig) {
    let defaults = SprocketConfig::default();
    if config.version == 0 {
        config.version = defaults.version;
    }
    if config.commit.owned_paths.is_empty() {
        config.commit.owned_paths = defaults.commit.owned_paths;
    }
    if config.commit.checkpoint_turn_threshold == 0 {
        config.commit.checkpoint_turn_threshold = defaults.commit.checkpoint_turn_threshold;
    }
    if config.commit.checkpoint_file_threshold == 0 {
        config.commit.checkpoint_file_threshold = defaults.commit.checkpoint_file_threshold;
    }
    if config.commit.lock_timeout_seconds == 0 {
        config.commit.lock_timeout_seconds = defaults.commit.lock_timeout_seconds;
    }
    if config.commit.milestone_file_threshold == 0 {
        config.commit.milestone_file_threshold = defaults.commit.milestone_file_threshold;
    }
    if config.commit.default_area.trim().is_empty() {
        config.commit.default_area = defaults.commit.default_area;
    }
    if config.commit.checkpoint_message_template.trim().is_empty() {
        config.commit.checkpoint_message_template = defaults.commit.checkpoint_message_template;
    }
    if config.commit.milestone_message_template.trim().is_empty() {
        config.commit.milestone_message_template = defaults.commit.milestone_message_template;
    }

    if config.docs.managed_outputs.is_empty() {
        config.docs.managed_outputs = defaults.docs.managed_outputs;
    }
    if config.docs.timeout_seconds == 0 {
        config.docs.timeout_seconds = defaults.docs.timeout_seconds;
    }
    if config.docs.model.trim().is_empty() {
        config.docs.model = defaults.docs.model;
    }
    if config.docs.reasoning_effort.trim().is_empty() {
        config.docs.reasoning_effort = defaults.docs.reasoning_effort;
    }
    if config.docs.sandbox.trim().is_empty() {
        config.docs.sandbox = defaults.docs.sandbox;
    }
    if config.docs.approval.trim().is_empty() {
        config.docs.approval = defaults.docs.approval;
    }
    if config.docs.triggers.source_roots.is_empty() {
        config.docs.triggers.source_roots = defaults.docs.triggers.source_roots;
    }
    if config.docs.triggers.test_roots.is_empty() {
        config.docs.triggers.test_roots = defaults.docs.triggers.test_roots;
    }
    if config.docs.triggers.milestone_globs.is_empty() {
        config.docs.triggers.milestone_globs = defaults.docs.triggers.milestone_globs;
    }
}

fn validate_config(config: &SprocketConfig) -> Result<()> {
    for owned in &config.commit.owned_paths {
        for output in &config.docs.managed_outputs {
            if pathspecs_overlap(owned, output) {
                bail!("docs.managed_outputs overlaps commit.owned_paths: `{output}` vs `{owned}`");
            }
        }
    }
    Ok(())
}

fn pathspecs_overlap(left: &str, right: &str) -> bool {
    let left = left.trim_matches('/');
    let right = right.trim_matches('/');
    if left.is_empty() || right.is_empty() {
        return false;
    }
    left == right
        || right.starts_with(&format!("{left}/"))
        || left.starts_with(&format!("{right}/"))
}
