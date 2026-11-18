//! Reconciliation of library state with the addon folders (§6.2).
//!
//! Disk is truth. Each managed file keeps its recorded placement if a file
//! with that name and size is there; otherwise it is looked up under its
//! disabled name, then among unclaimed pak files of the same size and hash.
//! Unmatched state files become `Missing`; unclaimed disk files are foreign.
//! Nothing is ever moved or deleted here.

use crate::scan::{DiskFile, DiskScan};
use kombini_core::naming::{disabled_names, parse_enabled_chunk, parse_enabled_root};
use kombini_core::{ForeignFile, Placement};
use kombini_storage::{LibraryState, hash_file};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RescanReport {
    pub missing_files: usize,
    pub relocated: usize,
    pub foreign_files: usize,
    pub other_managers: Vec<String>,
}

pub struct Outcome {
    pub changed: bool,
    pub foreign: Vec<ForeignFile>,
    missing: usize,
    relocated: usize,
    other_managers: Vec<String>,
}

impl Outcome {
    pub fn report(&self) -> RescanReport {
        RescanReport {
            missing_files: self.missing,
            relocated: self.relocated,
            foreign_files: self.foreign.len(),
            other_managers: self.other_managers.clone(),
        }
    }
}

fn placement_for(file: &DiskFile) -> Placement {
    let mounted = file.shard > 1
        || parse_enabled_root(&file.name).is_some()
        || parse_enabled_chunk(&file.name).is_some();
    if mounted {
        Placement::Mounted {
            shard: file.shard,
            file_name: file.name.clone(),
        }
    } else {
        Placement::Disabled {
            file_name: file.name.clone(),
        }
    }
}

pub fn reconcile(state: &mut LibraryState, disk: &DiskScan) -> Outcome {
    let mut claimed: BTreeSet<(u8, String)> = BTreeSet::new();
    let mut changed = false;
    let mut relocated = 0;
    let mut missing = 0;

    // Pass 1: exact recorded locations.
    let mut unresolved: Vec<(kombini_core::ModId, String)> = Vec::new();
    for m in state.mods.values_mut() {
        for f in &mut m.files {
            let hit = f.placement.location().and_then(|(shard, name)| {
                disk.get(shard, name)
                    .filter(|d| d.size == f.size)
                    .map(|d| (shard, d.name.to_ascii_lowercase()))
            });
            match hit {
                Some(key) if !claimed.contains(&key) => {
                    claimed.insert(key);
                }
                _ => unresolved.push((m.id.clone(), f.rel_path.clone())),
            }
        }
    }

    // Pass 2: disabled name, then a same-size, same-hash pak anywhere.
    let mut hash_cache: BTreeMap<(u8, String), Option<String>> = BTreeMap::new();
    for (mod_id, rel) in unresolved {
        let Some(m) = state.mods.get_mut(&mod_id) else {
            continue;
        };
        let names = disabled_names(&m.id, m.files.iter().map(|f| f.rel_path.as_str()));
        let id_prefix = m.id.as_str().to_ascii_lowercase();
        let Some(f) = m.file_mut(&rel) else { continue };
        let mut found: Option<(u8, String)> = None;
        if let Some(name) = names.get(&rel) {
            let key = (1u8, name.to_ascii_lowercase());
            if !claimed.contains(&key) && disk.get(1, name).is_some_and(|d| d.size == f.size) {
                found = Some(key);
            }
        }
        if found.is_none() {
            for (key, d) in &disk.files {
                if claimed.contains(key) || d.size != f.size {
                    continue;
                }
                let lower = &key.1;
                if !(lower.starts_with("pak") || lower.starts_with(&id_prefix)) {
                    continue;
                }
                let hash = hash_cache
                    .entry(key.clone())
                    .or_insert_with(|| hash_file(&d.path).ok().map(|(h, _)| h.to_string()));
                if hash.as_deref() == Some(f.blob.as_str()) {
                    found = Some(key.clone());
                    break;
                }
            }
        }
        let new_placement = match &found {
            Some(key) => {
                claimed.insert(key.clone());
                relocated += 1;
                disk.files
                    .get(key)
                    .map(placement_for)
                    .unwrap_or(Placement::Missing)
            }
            None => {
                missing += 1;
                Placement::Missing
            }
        };
        if f.placement != new_placement {
            f.placement = new_placement;
            changed = true;
        }
    }

    let foreign = disk
        .files
        .iter()
        .filter(|(key, _)| !claimed.contains(*key))
        .map(|(_, d)| ForeignFile {
            shard: d.shard,
            file_name: d.name.clone(),
            size: d.size,
            slot: parse_enabled_root(&d.name),
        })
        .collect();

    Outcome {
        changed,
        foreign,
        missing,
        relocated,
        other_managers: disk.other_managers.clone(),
    }
}
