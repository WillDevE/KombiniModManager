//! Discovery of `.vpk` files inside an extracted tree.

use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use walkdir::WalkDir;

/// The little-endian VPK signature `0x55AA1234` as stored in the file.
const VPK_SIGNATURE: [u8; 4] = [0x34, 0x12, 0xAA, 0x55];

/// One `.vpk` file found by [`discover_vpks`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiscoveredVpk {
    /// Path relative to the searched directory, `/`-separated.
    pub rel_path: String,
    /// File size in bytes.
    pub size: u64,
    /// True when the file starts with the VPK signature `0x55AA1234` (little endian).
    pub valid_signature: bool,
}

fn is_skipped(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    name == "__MACOSX"
        || name.starts_with('.')
        || name.eq_ignore_ascii_case("Thumbs.db")
        || name.eq_ignore_ascii_case("desktop.ini")
}

fn has_valid_signature(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let mut head = Vec::with_capacity(4);
    if file.take(4).read_to_end(&mut head).is_err() {
        return false;
    }
    head == VPK_SIGNATURE
}

/// Lists `.vpk` files (case-insensitive extension) under `dir` recursively, skipping any path
/// component named `__MACOSX`, components starting with `.`, and files named `Thumbs.db` /
/// `desktop.ini`. Links are not followed. Sorted by `rel_path`, case-insensitively.
pub fn discover_vpks(dir: &Path) -> Vec<DiscoveredVpk> {
    let walker = WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_skipped(e.file_name()));
    let mut found = Vec::new();
    for entry in walker.filter_map(Result::ok) {
        if entry.depth() == 0 || !entry.file_type().is_file() {
            continue;
        }
        let is_vpk = entry
            .path()
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("vpk"));
        if !is_vpk {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let rel = entry.path().strip_prefix(dir).unwrap_or(entry.path());
        let rel_path = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        found.push(DiscoveredVpk {
            valid_signature: has_valid_signature(entry.path()),
            size: meta.len(),
            rel_path,
        });
    }
    found.sort_by(|a, b| {
        a.rel_path
            .to_lowercase()
            .cmp(&b.rel_path.to_lowercase())
            .then_with(|| a.rel_path.cmp(&b.rel_path))
    });
    found
}
