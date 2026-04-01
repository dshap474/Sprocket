use std::collections::BTreeMap;

use crate::domain::manifest::StrictEntry;

pub fn changed_path_count(old: &[StrictEntry], new: &[StrictEntry]) -> usize {
    let mut old_map = BTreeMap::new();
    let mut new_map = BTreeMap::new();

    for entry in old {
        old_map.insert(entry.path.clone(), (entry.mode, entry.git_oid.as_str()));
    }
    for entry in new {
        new_map.insert(entry.path.clone(), (entry.mode, entry.git_oid.as_str()));
    }

    let mut changed = 0usize;
    for (path, old_sig) in &old_map {
        match new_map.get(path) {
            None => changed += 1,
            Some(new_sig) if new_sig != old_sig => changed += 1,
            _ => {}
        }
    }
    for path in new_map.keys() {
        if !old_map.contains_key(path) {
            changed += 1;
        }
    }
    changed
}
