use serde::{Deserialize, Serialize};

use crate::domain::ids::hash_hex;
use crate::domain::repopath::RepoPath;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    pub version: u32,
    pub owned: OwnedPolicy,
    pub checkpoint: CheckpointPolicy,
    pub promotion: PromotionPolicy,
    pub guard: GuardPolicy,
    pub compat: CompatPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OwnedPolicy {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointMode {
    HiddenOnly,
    HiddenThenPromote,
    VisibleDirect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckpointPolicy {
    pub mode: CheckpointMode,
    pub turn_threshold: u32,
    pub file_threshold: u32,
    pub age_minutes: u64,
    pub default_area: String,
    pub message_template: String,
    pub lock_timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PromotionPolicy {
    pub enabled: bool,
    pub validators: Vec<String>,
    pub continue_on_failure: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GuardPolicy {
    pub codex_pretool: bool,
    pub git_prepare_commit_msg: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CompatPolicy {
    pub allow_sparse_checkout: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyEpoch(pub String);

impl Default for Policy {
    fn default() -> Self {
        Self {
            version: 2,
            owned: OwnedPolicy::default(),
            checkpoint: CheckpointPolicy::default(),
            promotion: PromotionPolicy::default(),
            guard: GuardPolicy::default(),
            compat: CompatPolicy::default(),
        }
    }
}

impl Default for OwnedPolicy {
    fn default() -> Self {
        Self {
            include: vec![".".to_string()],
            exclude: vec![
                ":(exclude).git".to_string(),
                ":(exclude).sprocket".to_string(),
                ":(exclude)node_modules".to_string(),
                ":(exclude)target".to_string(),
                ":(exclude)dist".to_string(),
                ":(exclude)build".to_string(),
                ":(exclude).next".to_string(),
                ":(exclude)coverage".to_string(),
                ":(exclude).venv".to_string(),
            ],
        }
    }
}

impl Default for CheckpointPolicy {
    fn default() -> Self {
        Self {
            mode: CheckpointMode::HiddenOnly,
            turn_threshold: 2,
            file_threshold: 4,
            age_minutes: 20,
            default_area: "core".to_string(),
            message_template: "checkpoint({area}): save current work [auto]".to_string(),
            lock_timeout_seconds: 300,
        }
    }
}

impl Default for PromotionPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            validators: Vec::new(),
            continue_on_failure: true,
        }
    }
}

impl Default for GuardPolicy {
    fn default() -> Self {
        Self {
            codex_pretool: true,
            git_prepare_commit_msg: true,
        }
    }
}

impl Policy {
    pub fn policy_epoch(&self) -> PolicyEpoch {
        let mut payload = String::from("snapshot-schema:v1\n");
        for rule in self.git_include_pathspecs() {
            payload.push_str("include:");
            payload.push_str(&rule);
            payload.push('\n');
        }
        for rule in &self.owned.exclude {
            payload.push_str("exclude:");
            payload.push_str(rule);
            payload.push('\n');
        }
        payload.push_str("allow_sparse_checkout:");
        payload.push_str(if self.compat.allow_sparse_checkout {
            "true"
        } else {
            "false"
        });
        PolicyEpoch(format!("policy:{}", hash_hex(payload.as_bytes())))
    }

    pub fn git_include_pathspecs(&self) -> Vec<String> {
        if self.owned.include.is_empty() {
            vec![".".to_string()]
        } else {
            self.owned.include.clone()
        }
    }

    pub fn matches_owned_path(&self, path: &RepoPath) -> bool {
        let included = self
            .owned
            .include
            .iter()
            .any(|rule| matches_rule(rule, path, false));
        let excluded = self
            .owned
            .exclude
            .iter()
            .any(|rule| matches_rule(rule, path, true));
        included && !excluded
    }

    pub fn checkpoint_subject(&self) -> String {
        self.checkpoint
            .message_template
            .replace("{area}", &self.checkpoint.default_area)
    }

    pub fn hidden_only_mode(&self) -> bool {
        matches!(self.checkpoint.mode, CheckpointMode::HiddenOnly)
    }
}

fn matches_rule(rule: &str, path: &RepoPath, exclude_rule: bool) -> bool {
    let raw = if exclude_rule {
        rule.strip_prefix(":(exclude)").unwrap_or(rule)
    } else {
        rule
    };
    if raw == "." {
        return true;
    }

    let raw = raw.trim_matches('/');
    if raw.is_empty() {
        return true;
    }

    let candidate = path.as_bytes();
    let raw_bytes = raw.as_bytes();
    candidate == raw_bytes
        || candidate
            .strip_prefix(raw_bytes)
            .is_some_and(|suffix| suffix.first() == Some(&b'/'))
}
