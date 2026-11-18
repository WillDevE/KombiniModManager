//! Grouping of `.vpk` files into bundles (a root plus its chunk files).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::paths::{parse_chunk_name, root_base};
use crate::read::is_vpk;

/// A root VPK and the chunk files that belong to it (same directory, same base name).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct VpkBundle {
    pub root: PathBuf,
    /// `(chunk index, path)` sorted by index.
    pub chunks: Vec<(u16, PathBuf)>,
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn sort_key(path: &Path) -> (String, PathBuf) {
    (path.to_string_lossy().to_lowercase(), path.to_path_buf())
}

/// Group `.vpk` files into bundles per directory.
///
/// Files that look like chunks but have no root in their directory become standalone roots when
/// they start with a valid VPK signature; otherwise they are returned as orphans. Bundles and
/// orphans are sorted by path (case-insensitive).
pub fn group_bundles(paths: &[PathBuf]) -> (Vec<VpkBundle>, Vec<PathBuf>) {
    let mut seen: HashSet<String> = HashSet::new();
    // directory (lowercase) -> (roots, chunks by lowercase base)
    type Chunks = BTreeMap<String, Vec<(u16, PathBuf)>>;
    let mut dirs: BTreeMap<String, (Vec<PathBuf>, Chunks)> = BTreeMap::new();
    for path in paths {
        let name = file_name_of(path);
        if !name.to_lowercase().ends_with(".vpk") {
            continue;
        }
        if !seen.insert(path.to_string_lossy().to_lowercase()) {
            continue;
        }
        let dir_key = path
            .parent()
            .map(|p| p.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let entry = dirs.entry(dir_key).or_default();
        match parse_chunk_name(&name) {
            Some((base, index)) => {
                entry
                    .1
                    .entry(base.to_lowercase())
                    .or_default()
                    .push((index, path.clone()));
            }
            None => entry.0.push(path.clone()),
        }
    }

    let mut bundles: Vec<VpkBundle> = Vec::new();
    let mut orphans: Vec<PathBuf> = Vec::new();
    for (roots, chunks_by_base) in dirs.into_values() {
        let mut used: HashSet<String> = HashSet::new();
        for root in roots {
            let base = root_base(&file_name_of(&root)).to_lowercase();
            let mut chunks = chunks_by_base.get(&base).cloned().unwrap_or_default();
            if !chunks.is_empty() {
                used.insert(base);
            }
            chunks.sort_by(|a, b| {
                a.0.cmp(&b.0)
                    .then_with(|| sort_key(&a.1).cmp(&sort_key(&b.1)))
            });
            bundles.push(VpkBundle { root, chunks });
        }
        for (base, chunks) in chunks_by_base {
            if used.contains(&base) {
                continue;
            }
            for (_, path) in chunks {
                if is_vpk(&path) {
                    bundles.push(VpkBundle {
                        root: path,
                        chunks: Vec::new(),
                    });
                } else {
                    orphans.push(path);
                }
            }
        }
    }
    bundles.sort_by_key(|b| sort_key(&b.root));
    orphans.sort_by_key(|p| sort_key(p));
    (bundles, orphans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_directory_grouping_without_files() {
        let paths: Vec<PathBuf> = [
            "a/pak01_dir.vpk",
            "a/pak01_001.vpk",
            "a/pak01_000.vpk",
            "b/pak01_dir.vpk",
            "b/readme.txt",
            "c/PAK01_000.VPK",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        let (bundles, orphans) = group_bundles(&paths);
        assert_eq!(bundles.len(), 2);
        assert_eq!(bundles[0].root, PathBuf::from("a/pak01_dir.vpk"));
        assert_eq!(
            bundles[0].chunks,
            vec![
                (0, PathBuf::from("a/pak01_000.vpk")),
                (1, PathBuf::from("a/pak01_001.vpk"))
            ]
        );
        assert!(bundles[1].chunks.is_empty());
        // `c/PAK01_000.VPK` does not exist on disk, so it is not a valid VPK.
        assert_eq!(orphans, vec![PathBuf::from("c/PAK01_000.VPK")]);
    }
}
