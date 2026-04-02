use std::collections::{BTreeMap, BTreeSet};

use crate::domain::manifest::StrictEntry;
use crate::domain::repopath::RepoPath;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathDelta {
    pub path: RepoPath,
    pub old: Option<StrictEntry>,
    pub new: Option<StrictEntry>,
}

pub fn entries_by_path(entries: &[StrictEntry]) -> BTreeMap<RepoPath, StrictEntry> {
    entries
        .iter()
        .cloned()
        .map(|entry| (entry.path.clone(), entry))
        .collect()
}

pub fn changed_paths(old: &[StrictEntry], new: &[StrictEntry]) -> BTreeSet<RepoPath> {
    diff_entries(old, new)
        .into_iter()
        .map(|delta| delta.path)
        .collect()
}

pub fn diff_entries(old: &[StrictEntry], new: &[StrictEntry]) -> Vec<PathDelta> {
    let old_map = entries_by_path(old);
    let new_map = entries_by_path(new);
    let mut all_paths = BTreeSet::new();
    all_paths.extend(old_map.keys().cloned());
    all_paths.extend(new_map.keys().cloned());

    all_paths
        .into_iter()
        .filter_map(|path| {
            let old_entry = old_map.get(&path).cloned();
            let new_entry = new_map.get(&path).cloned();
            if same_entry(old_entry.as_ref(), new_entry.as_ref()) {
                None
            } else {
                Some(PathDelta {
                    path,
                    old: old_entry,
                    new: new_entry,
                })
            }
        })
        .collect()
}

pub fn changed_path_count(old: &[StrictEntry], new: &[StrictEntry]) -> usize {
    diff_entries(old, new).len()
}

fn same_entry(old: Option<&StrictEntry>, new: Option<&StrictEntry>) -> bool {
    match (old, new) {
        (None, None) => true,
        (Some(left), Some(right)) => left.mode == right.mode && left.git_oid == right.git_oid,
        _ => false,
    }
}
