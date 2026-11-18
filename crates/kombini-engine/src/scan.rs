//! Scans the addon shard folders (disk is truth).

use crate::paths::GamePaths;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskFile {
    pub shard: u8,
    pub name: String,
    pub size: u64,
    pub path: PathBuf,
    pub read_only: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DiskScan {
    /// Keyed by (shard, lowercase file name).
    pub files: BTreeMap<(u8, String), DiskFile>,
    pub other_managers: Vec<String>,
    pub addons_exists: bool,
}

impl DiskScan {
    pub fn get(&self, shard: u8, name: &str) -> Option<&DiskFile> {
        self.files.get(&(shard, name.to_ascii_lowercase()))
    }
}

fn is_vpk(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".vpk")
}

pub fn scan(paths: &GamePaths) -> DiskScan {
    let mut out = DiskScan {
        addons_exists: paths.shard(1).is_dir(),
        ..DiskScan::default()
    };
    for (shard, dir) in paths.shards() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                if name.eq_ignore_ascii_case("_kombini") {
                    continue;
                }
                // DMM keeps per-profile folders of pak files inside addons*.
                if let Ok(sub) = std::fs::read_dir(entry.path()) {
                    let has_paks = sub.flatten().any(|e| {
                        let n = e.file_name().to_string_lossy().to_ascii_lowercase();
                        n.starts_with("pak") && n.ends_with("_dir.vpk")
                    });
                    if has_paks {
                        push_unique(
                            &mut out.other_managers,
                            format!(
                                "Another manager's profile folder: {}",
                                entry.path().display()
                            ),
                        );
                    }
                }
                continue;
            }
            if name.eq_ignore_ascii_case(".dmm.json") {
                push_unique(
                    &mut out.other_managers,
                    "Deadlock Mod Manager (DMM) manifest found in the addons folder".to_owned(),
                );
                continue;
            }
            if !is_vpk(&name) || name.starts_with('.') {
                continue;
            }
            out.files.insert(
                (shard, name.to_ascii_lowercase()),
                DiskFile {
                    shard,
                    name,
                    size: meta.len(),
                    path: entry.path(),
                    read_only: meta.permissions().readonly(),
                },
            );
        }
    }
    if paths.citadel.join("addons1").is_dir() {
        push_unique(
            &mut out.other_managers,
            "An addons1 folder exists (used by Grimoire)".to_owned(),
        );
    }
    out
}

fn push_unique(list: &mut Vec<String>, value: String) {
    if !list.contains(&value) {
        list.push(value);
    }
}
