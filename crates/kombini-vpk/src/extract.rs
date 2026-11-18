//! Safe extraction of VPK contents to a directory.

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Component, Path, PathBuf};

use crate::error::{Result, VpkError};
use crate::read::{Reader, VpkArchive, VpkEntry};

/// Normalize an archive member name. Rejects traversal, absolute and drive paths, `:` in any
/// component and NUL; skips empty and `.` components.
pub fn safe_relpath(name: &str) -> Option<String> {
    let name = name.replace('\\', "/");
    if name.is_empty() || name.starts_with('/') || name.contains('\0') || name.contains(':') {
        return None;
    }
    let mut parts = Vec::new();
    for part in name.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            p => parts.push(p),
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

/// Options for [`extract`].
pub struct ExtractOptions<'a> {
    /// Only extract these exact entry paths (`None` extracts everything).
    pub selected: Option<&'a [String]>,
    /// Fail with `CrcMismatch` when the data does not match the stored CRC.
    pub verify_crc: bool,
    pub cancel: &'a dyn Fn() -> bool,
    /// Called after each file with `(done, total)`.
    pub progress: &'a dyn Fn(usize, usize),
}

/// Extract files into `dest`, returning the written paths. Every target is validated to stay
/// inside `dest` before anything is written.
pub fn extract(archive: &VpkArchive, dest: &Path, opts: &ExtractOptions) -> Result<Vec<PathBuf>> {
    let wanted: Option<HashSet<&str>> = opts
        .selected
        .map(|s| s.iter().map(String::as_str).collect());
    let groups = archive.groups();
    let mut work: Vec<(&str, Vec<&VpkEntry>, PathBuf)> = Vec::new();
    for (path, idxs) in groups {
        if wanted.as_ref().is_some_and(|w| !w.contains(path)) {
            continue;
        }
        let safe = safe_relpath(path).ok_or_else(|| VpkError::UnsafePath(path.to_string()))?;
        let rel = Path::new(&safe);
        if !rel.components().all(|c| matches!(c, Component::Normal(_))) {
            return Err(VpkError::UnsafePath(path.to_string()));
        }
        let target = dest.join(rel);
        if !target.starts_with(dest) {
            return Err(VpkError::UnsafePath(path.to_string()));
        }
        let entries = idxs.iter().map(|&i| &archive.entries()[i]).collect();
        work.push((path, entries, target));
    }

    std::fs::create_dir_all(dest).map_err(|e| VpkError::io(dest, e))?;
    let total = work.len();
    let mut reader = Reader::new(archive);
    let mut written = Vec::with_capacity(total);
    for (done, (path, entries, target)) in work.into_iter().enumerate() {
        if (opts.cancel)() {
            return Err(VpkError::Cancelled);
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| VpkError::io(parent, e))?;
        }
        let file = File::create(&target).map_err(|e| VpkError::io(&target, e))?;
        let mut out = BufWriter::new(file);
        let result = reader
            .stream_group(&entries, &mut |b| {
                out.write_all(b).map_err(|e| VpkError::io(&target, e))
            })
            .and_then(|check| {
                out.flush().map_err(|e| VpkError::io(&target, e))?;
                Ok(check)
            });
        drop(out);
        match result {
            Ok(check) if !opts.verify_crc || check.ok() => {}
            Ok(check) => {
                let _ = std::fs::remove_file(&target);
                return Err(VpkError::CrcMismatch {
                    path: archive.path().to_path_buf(),
                    entry: path.to_string(),
                    expected: check.expected,
                    actual: check.actual,
                });
            }
            Err(err) => {
                let _ = std::fs::remove_file(&target);
                return Err(err);
            }
        }
        written.push(target);
        (opts.progress)(done + 1, total);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_relpath_rules() {
        assert_eq!(safe_relpath("a/b.txt").as_deref(), Some("a/b.txt"));
        assert_eq!(safe_relpath("a\\b\\c.txt").as_deref(), Some("a/b/c.txt"));
        assert_eq!(safe_relpath("./a//b.txt").as_deref(), Some("a/b.txt"));
        assert_eq!(safe_relpath("../x.txt"), None);
        assert_eq!(safe_relpath("a/../x.txt"), None);
        assert_eq!(safe_relpath("/abs.txt"), None);
        assert_eq!(safe_relpath("\\abs.txt"), None);
        assert_eq!(safe_relpath("C:/x.txt"), None);
        assert_eq!(safe_relpath("a/b:stream"), None);
        assert_eq!(safe_relpath("a/b\0.txt"), None);
        assert_eq!(safe_relpath(""), None);
        assert_eq!(safe_relpath("./"), None);
    }
}
