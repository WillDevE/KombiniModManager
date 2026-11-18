//! File-level conflict detection (ARCHITECTURE §4.3).

use crate::ids::ModId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    /// Lowercased game path.
    pub path: String,
    /// Owning mods in load order; `owners[0]` is the winner.
    pub owners: Vec<ModId>,
    pub winner: ModId,
}

/// `ordered` = enabled mods in load order with the game paths of their mounted
/// roots. Paths are compared lowercase with `\` normalized to `/`. A mod never
/// conflicts with itself. Ignored paths (lowercase) are skipped.
pub fn find_conflicts(
    ordered: &[(ModId, Vec<String>)],
    ignored: &BTreeSet<String>,
) -> Vec<Conflict> {
    let mut owners: BTreeMap<String, Vec<ModId>> = BTreeMap::new();
    for (id, paths) in ordered {
        let unique: BTreeSet<String> = paths
            .iter()
            .map(|p| p.replace('\\', "/").to_lowercase())
            .collect();
        for path in unique {
            let list = owners.entry(path).or_default();
            if !list.contains(id) {
                list.push(id.clone());
            }
        }
    }
    owners
        .into_iter()
        .filter(|(path, list)| list.len() > 1 && !ignored.contains(path))
        .map(|(path, list)| Conflict {
            winner: list[0].clone(),
            owners: list,
            path,
        })
        .collect()
}

/// Per-mod-pair summary used by the conflicts view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictPair {
    pub winner: ModId,
    pub loser: ModId,
    pub paths: usize,
    pub sample: Vec<String>,
}

pub fn summarize_pairs(conflicts: &[Conflict]) -> Vec<ConflictPair> {
    let mut pairs: BTreeMap<(ModId, ModId), (usize, Vec<String>)> = BTreeMap::new();
    for c in conflicts {
        for loser in c.owners.iter().skip(1) {
            let entry = pairs
                .entry((c.winner.clone(), loser.clone()))
                .or_insert((0, Vec::new()));
            entry.0 += 1;
            if entry.1.len() < 10 {
                entry.1.push(c.path.clone());
            }
        }
    }
    let mut out: Vec<ConflictPair> = pairs
        .into_iter()
        .map(|((winner, loser), (paths, sample))| ConflictPair {
            winner,
            loser,
            paths,
            sample,
        })
        .collect();
    out.sort_by(|a, b| b.paths.cmp(&a.paths).then_with(|| a.winner.cmp(&b.winner)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, paths: &[&str]) -> (ModId, Vec<String>) {
        (
            ModId::from(id),
            paths.iter().map(|s| (*s).to_owned()).collect(),
        )
    }

    #[test]
    fn winner_is_first_in_order() {
        let ordered = vec![
            entry("a", &["Models/X.vmdl_c", "only_a.txt"]),
            entry("b", &["models\\x.vmdl_c", "models/x.vmdl_c"]),
            entry("c", &["models/x.vmdl_c"]),
        ];
        let conflicts = find_conflicts(&ordered, &BTreeSet::new());
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].path, "models/x.vmdl_c");
        assert_eq!(conflicts[0].winner, ModId::from("a"));
        assert_eq!(conflicts[0].owners.len(), 3);
        let pairs = summarize_pairs(&conflicts);
        assert_eq!(pairs.len(), 2);
    }

    #[test]
    fn same_mod_twice_is_not_a_conflict() {
        let ordered = vec![entry("a", &["x"]), entry("a", &["x"])];
        assert!(find_conflicts(&ordered, &BTreeSet::new()).is_empty());
    }

    #[test]
    fn ignored_paths_hidden() {
        let ordered = vec![entry("a", &["x"]), entry("b", &["X"])];
        let ignored: BTreeSet<String> = ["x".to_owned()].into_iter().collect();
        assert!(find_conflicts(&ordered, &ignored).is_empty());
    }
}
