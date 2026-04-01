use std::collections::BTreeSet;

use anyhow::{Result, anyhow};

use crate::domain::manager::PendingSource;
use crate::domain::manifest::StrictSnapshot;
use crate::domain::policy::PolicyEpoch;
use crate::domain::session::{HeadState, StreamClass, StreamIdentity};
use crate::infra::git::GitBackend;

pub struct PreparedHiddenCheckpoint {
    pub commit_oid: String,
}

#[derive(Debug, Clone)]
pub struct CheckpointMetadata {
    pub generation: u64,
    pub policy_epoch: String,
    pub stream_class: StreamClass,
    pub observed_head_ref: Option<String>,
    pub observed_head_oid: Option<String>,
    pub materialized_fingerprint: String,
    pub observed_fingerprint: Option<String>,
}

pub struct CheckpointMessageContext<'a> {
    pub subject: &'a str,
    pub generation: u64,
    pub source: PendingSource,
    pub snapshot: &'a StrictSnapshot,
    pub head: &'a HeadState,
    pub stream: &'a StreamIdentity,
    pub policy_epoch: &'a PolicyEpoch,
}

pub fn prepare_hidden_checkpoint(
    git: &dyn GitBackend,
    head_oid: Option<&str>,
    previous_hidden_oid: Option<&str>,
    head_owned_paths: &[crate::domain::repopath::RepoPath],
    snapshot: &StrictSnapshot,
    message: &str,
) -> Result<PreparedHiddenCheckpoint> {
    let temp_index = git.create_temp_index()?;
    if let Some(head) = head_oid {
        git.read_tree_into_index(temp_index.path(), head)?;
    }

    let strict_paths: BTreeSet<_> = snapshot
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    if head_oid.is_some() {
        let deleted: Vec<_> = head_owned_paths
            .iter()
            .filter(|path| !strict_paths.contains(*path))
            .cloned()
            .collect();
        if !deleted.is_empty() {
            git.force_remove_from_index(temp_index.path(), &deleted)?;
        }
    }

    for entry in &snapshot.entries {
        git.update_index_cacheinfo(temp_index.path(), entry.mode, &entry.git_oid, &entry.path)?;
    }

    let tree_oid = git.write_tree_from_index(temp_index.path())?;
    let mut parents = Vec::new();
    if let Some(previous) = previous_hidden_oid {
        parents.push(previous.to_string());
    } else if let Some(head) = head_oid {
        parents.push(head.to_string());
    }
    let commit_oid = git.commit_tree(&tree_oid, &parents, message)?;
    Ok(PreparedHiddenCheckpoint { commit_oid })
}

pub fn build_checkpoint_message(ctx: CheckpointMessageContext<'_>) -> String {
    let stream_class = match &ctx.stream.class {
        StreamClass::Branch { symref } => format!("branch:{symref}"),
        StreamClass::DetachedHead => "detached_head".to_string(),
    };
    format!(
        "{subject}\n\nSprocket-Generation: {generation}\nSprocket-Source: {source:?}\nSprocket-Policy-Epoch: {policy_epoch}\nSprocket-Stream-Class: {stream_class}\nSprocket-Observed-Head-Ref: {observed_head_ref}\nSprocket-Observed-Head-Oid: {observed_head_oid}\nSprocket-Materialized-Fingerprint: {materialized_fingerprint}\n{observed_footer}Sprocket-Stream: {stream_name}\nSprocket-Worktree: {worktree}\n",
        subject = ctx.subject,
        generation = ctx.generation,
        source = ctx.source,
        policy_epoch = ctx.policy_epoch.0,
        stream_class = stream_class,
        observed_head_ref = ctx
            .head
            .symref
            .clone()
            .unwrap_or_else(|| "none".to_string()),
        observed_head_oid = ctx.head.oid.clone().unwrap_or_else(|| "none".to_string()),
        materialized_fingerprint = ctx.snapshot.materialized_fingerprint,
        observed_footer = ctx
            .snapshot
            .observed_fingerprint
            .as_ref()
            .map(|value| format!("Sprocket-Observed-Fingerprint: {value}\n"))
            .unwrap_or_default(),
        stream_name = ctx.stream.display_name,
        worktree = ctx.stream.worktree_id,
    )
}

pub fn parse_checkpoint_metadata(message: &str) -> Result<CheckpointMetadata> {
    let footers = message
        .lines()
        .filter_map(|line| line.split_once(": "))
        .collect::<std::collections::BTreeMap<_, _>>();
    let generation = footers
        .get("Sprocket-Generation")
        .ok_or_else(|| anyhow!("missing Sprocket-Generation footer"))?
        .parse()?;
    let policy_epoch = footers
        .get("Sprocket-Policy-Epoch")
        .ok_or_else(|| anyhow!("missing Sprocket-Policy-Epoch footer"))?
        .to_string();
    let stream_class = parse_stream_class(
        footers
            .get("Sprocket-Stream-Class")
            .ok_or_else(|| anyhow!("missing Sprocket-Stream-Class footer"))?,
    )?;
    let observed_head_ref = optional_footer(footers.get("Sprocket-Observed-Head-Ref").copied());
    let observed_head_oid = optional_footer(footers.get("Sprocket-Observed-Head-Oid").copied());
    let materialized_fingerprint = footers
        .get("Sprocket-Materialized-Fingerprint")
        .ok_or_else(|| anyhow!("missing Sprocket-Materialized-Fingerprint footer"))?
        .to_string();
    let observed_fingerprint =
        optional_footer(footers.get("Sprocket-Observed-Fingerprint").copied());

    Ok(CheckpointMetadata {
        generation,
        policy_epoch,
        stream_class,
        observed_head_ref,
        observed_head_oid,
        materialized_fingerprint,
        observed_fingerprint,
    })
}

fn parse_stream_class(value: &str) -> Result<StreamClass> {
    if let Some(symref) = value.strip_prefix("branch:") {
        return Ok(StreamClass::Branch {
            symref: symref.to_string(),
        });
    }
    if value == "detached_head" {
        return Ok(StreamClass::DetachedHead);
    }
    Err(anyhow!("unsupported Sprocket-Stream-Class footer: {value}"))
}

fn optional_footer(value: Option<&str>) -> Option<String> {
    value.and_then(|current| {
        if current == "none" {
            None
        } else {
            Some(current.to_string())
        }
    })
}
