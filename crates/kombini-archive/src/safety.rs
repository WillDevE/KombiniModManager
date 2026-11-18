//! Path sanitising, link rejection and small filesystem helpers shared by all backends.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::{ArchiveError, Backend};

/// `FILE_ATTRIBUTE_REPARSE_POINT`: symlinks, junctions and other reparse points on Windows.
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

pub(crate) fn io_err(path: &Path, source: io::Error) -> ArchiveError {
    ArchiveError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Normalize an entry name: `\` becomes `/`, `.` components are dropped; returns `None` for empty
/// names, absolute names (leading `/`), any `..` component, any component containing `:` (drive
/// letters, NTFS alternate data streams) or NUL, and components ending in `.` or a space (Windows
/// trims those, which enables aliasing). Otherwise returns the `/`-joined relative path.
pub fn sanitize_entry(name: &str) -> Option<String> {
    if name.is_empty() || name.contains('\0') {
        return None;
    }
    let normalized = name.replace('\\', "/");
    if normalized.starts_with('/') {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for component in normalized.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".."
            || component.contains(':')
            || component.ends_with('.')
            || component.ends_with(' ')
            || is_reserved_device_name(component)
            || component
                .chars()
                .any(|c| matches!(c, '<' | '>' | '"' | '|' | '?' | '*') || (c as u32) < 0x20)
        {
            return None;
        }
        parts.push(component);
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// Windows reserves these device names regardless of extension (`NUL.txt`).
fn is_reserved_device_name(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .trim_end()
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ((stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.len() == 4
        && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}

/// True for directory entries that only name the extraction root itself (`./`, `.`), which are
/// harmless and are skipped instead of being rejected.
pub(crate) fn is_root_entry(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let normalized = name.replace('\\', "/");
    !normalized.starts_with('/') && normalized.split('/').all(|c| c.is_empty() || c == ".")
}

/// Joins a sanitized `/`-separated relative path onto `dest`.
pub(crate) fn join_rel(dest: &Path, rel: &str) -> PathBuf {
    let mut out = dest.to_path_buf();
    for component in rel.split('/') {
        out.push(component);
    }
    out
}

/// True if the metadata (obtained without following links) describes a symlink or, on Windows,
/// any reparse point (junction, mount point, ...).
pub(crate) fn is_link(meta: &Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// Fails with `Link` when any existing ancestor directory of `dest/rel` (or the target itself)
/// is a link. Used before a backend writes through a path it did not create itself.
pub(crate) fn ensure_no_link_ancestors(dest: &Path, rel: &str) -> Result<(), ArchiveError> {
    let mut current = dest.to_path_buf();
    for component in rel.split('/') {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                if is_link(&meta) {
                    return Err(ArchiveError::Link(rel.to_string()));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(io_err(&current, e)),
        }
    }
    Ok(())
}

/// Creates the parent directories of `out`.
pub(crate) fn create_parent_dirs(out: &Path) -> Result<(), ArchiveError> {
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
    }
    Ok(())
}

/// Creates `out` exclusively (never overwriting or writing through an existing file or link).
pub(crate) fn create_output_file(
    out: &Path,
    rel: &str,
    backend: Backend,
) -> Result<File, ArchiveError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out)
        .map_err(|e| {
            if e.kind() == io::ErrorKind::AlreadyExists {
                ArchiveError::Failed {
                    backend,
                    message: format!("duplicate or conflicting entry: {rel}"),
                }
            } else {
                io_err(out, e)
            }
        })
}

/// Removes every child of `dir` (the directory itself is kept). Links are removed, not followed.
pub(crate) fn clear_dir(dir: &Path) -> Result<(), ArchiveError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io_err(dir, e)),
    };
    for entry in entries {
        let entry = entry.map_err(|e| io_err(dir, e))?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).map_err(|e| io_err(&path, e))?;
        let removed = if meta.is_dir() && !is_link(&meta) {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path).or_else(|_| fs::remove_dir(&path))
        };
        removed.map_err(|e| io_err(&path, e))?;
    }
    Ok(())
}

fn walk_err(root: &Path, err: walkdir::Error) -> ArchiveError {
    let path = err.path().unwrap_or(root).to_path_buf();
    ArchiveError::Io {
        path,
        source: io::Error::from(err),
    }
}

fn rel_string(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Walks `dest` without following links. Any symlink, or on Windows any reparse point, fails with
/// `Link(relative path)`. Returns all regular files (relative `/` paths, sorted) and their total
/// size in bytes.
pub fn post_validate(dest: &Path) -> Result<(Vec<String>, u64), ArchiveError> {
    let mut files = Vec::new();
    let mut total = 0u64;
    for entry in WalkDir::new(dest).follow_links(false).min_depth(1) {
        let entry = entry.map_err(|e| walk_err(dest, e))?;
        let rel = rel_string(dest, entry.path());
        let meta = entry.metadata().map_err(|e| walk_err(dest, e))?;
        if is_link(&meta) {
            return Err(ArchiveError::Link(rel));
        }
        if meta.is_file() {
            total = total.saturating_add(meta.len());
            files.push(rel);
        }
    }
    files.sort();
    Ok((files, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_accepts_and_normalizes() {
        assert_eq!(sanitize_entry("a/b.vpk").as_deref(), Some("a/b.vpk"));
        assert_eq!(sanitize_entry("./a/./b.vpk").as_deref(), Some("a/b.vpk"));
        assert_eq!(sanitize_entry("a\\b.vpk").as_deref(), Some("a/b.vpk"));
        assert_eq!(sanitize_entry("dir/").as_deref(), Some("dir"));
        assert_eq!(sanitize_entry("a//b").as_deref(), Some("a/b"));
    }

    #[test]
    fn sanitize_rejects_unsafe() {
        for bad in [
            "../x",
            "a/../../x",
            "a/../x",
            "/abs",
            "\\abs",
            "C:/x",
            "a/C:x",
            "a/b:stream",
            "a/b./c",
            "a/b /c",
            "",
            ".",
            "./",
            "a\0b",
            "..\\x",
        ] {
            assert_eq!(sanitize_entry(bad), None, "{bad:?} must be rejected");
        }
    }

    #[test]
    fn root_entries() {
        assert!(is_root_entry("./"));
        assert!(is_root_entry("."));
        assert!(!is_root_entry(""));
        assert!(!is_root_entry("/"));
        assert!(!is_root_entry("a/"));
    }
}

#[cfg(test)]
mod reserved_tests {
    use super::sanitize_entry;

    #[test]
    fn rejects_windows_reserved_and_invalid_names() {
        for bad in [
            "NUL",
            "con.txt",
            "a/COM1.vpk",
            "lpt9",
            "a/b?.vpk",
            "x*y",
            "tab	here",
        ] {
            assert_eq!(sanitize_entry(bad), None, "{bad}");
        }
        for ok in [
            "console.vpk",
            "com10.txt",
            "nullable/pak01_dir.vpk",
            "LPT/x.vpk",
        ] {
            assert!(sanitize_entry(ok).is_some(), "{ok}");
        }
    }
}
