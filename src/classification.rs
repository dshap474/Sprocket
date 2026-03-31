use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::Result;
use globset::{Glob, GlobSetBuilder};

use crate::config::{CommitKind, SprocketConfig};
use crate::{ManagerState, ManifestEntry, now_unix_seconds};

#[derive(Debug, Clone, Default)]
pub(crate) struct Delta {
    pub(crate) added: Vec<String>,
    pub(crate) modified: Vec<String>,
    pub(crate) deleted: Vec<String>,
    pub(crate) changed_paths: Vec<String>,
}

pub(crate) fn diff_manifests(
    old_manifest: &[ManifestEntry],
    new_manifest: &[ManifestEntry],
) -> Delta {
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

pub(crate) fn classify_kind(
    delta: &Delta,
    manager: &ManagerState,
    config: &SprocketConfig,
) -> Result<CommitKind> {
    if delta.changed_paths.is_empty() {
        return Ok(CommitKind::None);
    }
    if !config.docs.enabled {
        return Ok(if should_checkpoint(delta, manager, config) {
            CommitKind::Checkpoint
        } else {
            CommitKind::None
        });
    }

    if manager.docs_backlog {
        return Ok(CommitKind::Milestone);
    }
    if delta.changed_paths.len() as u32 >= config.commit.milestone_file_threshold {
        return Ok(CommitKind::Milestone);
    }
    if matches_milestone_glob(delta, &config.docs.triggers.milestone_globs)? {
        return Ok(CommitKind::Milestone);
    }
    if source_root_layout_changed(delta, &config.docs.triggers.source_roots) {
        return Ok(CommitKind::Milestone);
    }
    if has_changes_under_roots(delta, &config.docs.triggers.source_roots)
        && has_changes_under_roots(delta, &config.docs.triggers.test_roots)
        && manager.pending_turn_count >= 1
    {
        return Ok(CommitKind::Milestone);
    }

    Ok(if should_checkpoint(delta, manager, config) {
        CommitKind::Checkpoint
    } else {
        CommitKind::None
    })
}

fn should_checkpoint(delta: &Delta, manager: &ManagerState, config: &SprocketConfig) -> bool {
    if manager.pending_turn_count + 1 >= config.commit.checkpoint_turn_threshold {
        return true;
    }
    if delta.changed_paths.len() as u32 >= config.commit.checkpoint_file_threshold {
        return true;
    }
    let Some(last_checkpoint_at) = manager.last_checkpoint_at else {
        return false;
    };
    now_unix_seconds().saturating_sub(last_checkpoint_at)
        >= config.commit.checkpoint_age_minutes.saturating_mul(60)
}

fn matches_milestone_glob(delta: &Delta, globs: &[String]) -> Result<bool> {
    let mut builder = GlobSetBuilder::new();
    for pattern in globs {
        builder.add(Glob::new(pattern)?);
    }
    let set = builder.build()?;
    Ok(delta.changed_paths.iter().any(|path| set.is_match(path)))
}

fn source_root_layout_changed(delta: &Delta, source_roots: &[String]) -> bool {
    delta
        .added
        .iter()
        .chain(&delta.deleted)
        .any(|path| is_under_roots(path, source_roots))
}

fn has_changes_under_roots(delta: &Delta, roots: &[String]) -> bool {
    delta
        .changed_paths
        .iter()
        .any(|path| is_under_roots(path, roots))
}

fn is_under_roots(path: &str, roots: &[String]) -> bool {
    let path = Path::new(path);
    roots.iter().any(|root| path.starts_with(root))
}
