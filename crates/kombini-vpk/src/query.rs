//! Paged, filtered and folder-tree queries over a VPK directory (for the inspector).

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::read::{EMBEDDED, VpkArchive};

/// Column to sort by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SortKey {
    Path,
    Size,
    Crc,
    Archive,
}

/// A paged query.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct EntryQuery {
    pub filter: String,
    pub offset: usize,
    pub limit: usize,
    pub sort: SortKey,
    pub descending: bool,
}

/// One unique path in the inspector table.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EntryRow {
    pub path: String,
    pub size: u64,
    pub crc32: u32,
    pub archive_index: Option<u16>,
    pub preload: u16,
}

/// One page of results.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EntryPage {
    pub total: usize,
    pub offset: usize,
    pub rows: Vec<EntryRow>,
}

/// An immediate child of a folder.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FolderNode {
    pub name: String,
    pub is_dir: bool,
    pub file_count: usize,
    pub size: u64,
}

/// Lowercased-path index over an archive with one row per unique path.
pub struct VpkIndex {
    archive: VpkArchive,
    rows: Vec<EntryRow>,
    lower: Vec<String>,
    /// Row indices ordered by (lowercase path, path).
    by_path: Vec<u32>,
}

impl VpkIndex {
    pub fn new(archive: VpkArchive) -> Self {
        let mut rows: Vec<EntryRow> = Vec::with_capacity(archive.entries().len());
        let mut lookup: HashMap<&str, usize> = HashMap::with_capacity(archive.entries().len());
        for e in archive.entries() {
            match lookup.get(e.path.as_str()) {
                Some(&i) => {
                    let row = &mut rows[i];
                    row.size += e.size();
                    row.preload = row.preload.saturating_add(e.preload_len);
                }
                None => {
                    lookup.insert(e.path.as_str(), rows.len());
                    rows.push(EntryRow {
                        path: e.path.clone(),
                        size: e.size(),
                        crc32: e.crc32,
                        archive_index: (e.archive_index != EMBEDDED).then_some(e.archive_index),
                        preload: e.preload_len,
                    });
                }
            }
        }
        drop(lookup);
        let lower: Vec<String> = rows.iter().map(|r| r.path.to_lowercase()).collect();
        let mut by_path: Vec<u32> = (0..rows.len() as u32).collect();
        by_path.sort_by(|&a, &b| {
            lower[a as usize]
                .cmp(&lower[b as usize])
                .then_with(|| rows[a as usize].path.cmp(&rows[b as usize].path))
        });
        Self {
            archive,
            rows,
            lower,
            by_path,
        }
    }

    pub fn archive(&self) -> &VpkArchive {
        &self.archive
    }

    /// Number of unique paths.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Filter (case-insensitive substring, all whitespace-separated terms must match), sort and
    /// page. `limit` is clamped to `1..=1000`.
    pub fn query(&self, q: &EntryQuery) -> EntryPage {
        let limit = q.limit.clamp(1, 1000);
        let terms: Vec<String> = q.filter.split_whitespace().map(str::to_lowercase).collect();
        let mut matches: Vec<u32> = if terms.is_empty() {
            self.by_path.clone()
        } else {
            self.by_path
                .iter()
                .copied()
                .filter(|&i| {
                    let hay = &self.lower[i as usize];
                    terms.iter().all(|t| hay.contains(t.as_str()))
                })
                .collect()
        };
        let rows = &self.rows;
        match q.sort {
            SortKey::Path => {
                if q.descending {
                    matches.reverse();
                }
            }
            SortKey::Size => sort_stable(&mut matches, q.descending, |a, b| {
                rows[a as usize].size.cmp(&rows[b as usize].size)
            }),
            SortKey::Crc => sort_stable(&mut matches, q.descending, |a, b| {
                rows[a as usize].crc32.cmp(&rows[b as usize].crc32)
            }),
            SortKey::Archive => sort_stable(&mut matches, q.descending, |a, b| {
                rows[a as usize]
                    .archive_index
                    .cmp(&rows[b as usize].archive_index)
            }),
        }
        let total = matches.len();
        let start = q.offset.min(total);
        let end = start.saturating_add(limit).min(total);
        let page = matches[start..end]
            .iter()
            .map(|&i| rows[i as usize].clone())
            .collect();
        EntryPage {
            total,
            offset: q.offset,
            rows: page,
        }
    }

    /// Immediate children of `prefix` (`""` is the root): directories with file counts first,
    /// then files. Prefix matching is case-sensitive.
    pub fn folders(&self, prefix: &str) -> Vec<FolderNode> {
        let normalized = prefix.replace('\\', "/");
        let prefix = normalized.trim_matches('/');
        let mut dirs: Vec<FolderNode> = Vec::new();
        let mut dir_index: HashMap<&str, usize> = HashMap::new();
        let mut files: Vec<FolderNode> = Vec::new();
        for &i in &self.by_path {
            let row = &self.rows[i as usize];
            let rest = if prefix.is_empty() {
                row.path.as_str()
            } else {
                match row
                    .path
                    .strip_prefix(prefix)
                    .and_then(|r| r.strip_prefix('/'))
                {
                    Some(r) => r,
                    None => continue,
                }
            };
            match rest.split_once('/') {
                Some((dir, _)) => {
                    let idx = *dir_index.entry(dir).or_insert_with(|| {
                        dirs.push(FolderNode {
                            name: dir.to_string(),
                            is_dir: true,
                            file_count: 0,
                            size: 0,
                        });
                        dirs.len() - 1
                    });
                    dirs[idx].file_count += 1;
                    dirs[idx].size += row.size;
                }
                None => files.push(FolderNode {
                    name: rest.to_string(),
                    is_dir: false,
                    file_count: 1,
                    size: row.size,
                }),
            }
        }
        dirs.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.name.cmp(&b.name))
        });
        dirs.extend(files);
        dirs
    }
}

fn sort_stable(v: &mut [u32], descending: bool, cmp: impl Fn(u32, u32) -> Ordering) {
    if descending {
        v.sort_by(|&a, &b| cmp(b, a));
    } else {
        v.sort_by(|&a, &b| cmp(a, b));
    }
}
