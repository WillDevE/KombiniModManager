//! 7z extraction (via `sevenz-rust2`) with up-front validation and counted writes.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

use sevenz_rust2::{ArchiveEntry, ArchiveReader, EntryDecision, Error as SzError, Password};

use crate::limits::{Ctx, copy_limited};
use crate::safety::{
    create_output_file, create_parent_dirs, io_err, is_root_entry, join_rel, sanitize_entry,
};
use crate::{ArchiveError, Backend};

/// Upper bound on decoder memory (header buffers and codec dictionaries), in KiB (1 GiB).
const MEMORY_LIMIT_KB: usize = 1024 * 1024;

/// `FILE_ATTRIBUTE_REPARSE_POINT`.
const ATTR_REPARSE_POINT: u32 = 0x400;
/// 7-Zip marks that the high 16 bits of the attributes carry a Unix mode with this bit.
const ATTR_UNIX_EXTENSION: u32 = 0x8000;
const S_IFMT: u32 = 0o170_000;
const S_IFLNK: u32 = 0o120_000;

fn map_error(err: SzError) -> ArchiveError {
    match err {
        SzError::PasswordRequired | SzError::MaybeBadPassword(_) => ArchiveError::Encrypted,
        other => ArchiveError::Failed {
            backend: Backend::SevenZ,
            message: other.to_string(),
        },
    }
}

fn is_symlink_entry(entry: &ArchiveEntry) -> bool {
    if !entry.has_windows_attributes {
        return false;
    }
    let attrs = entry.windows_attributes();
    if attrs & ATTR_REPARSE_POINT != 0 {
        return true;
    }
    attrs & ATTR_UNIX_EXTENSION != 0 && (attrs >> 16) & S_IFMT == S_IFLNK
}

/// Extracts `archive_path` into the (empty) directory `dest`.
///
/// Entry names, link attributes, anti-items, encryption and the entry count are validated before
/// anything is written; decoded bytes are charged against the budget as they are written.
pub(crate) fn extract_sevenz(
    archive_path: &Path,
    dest: &Path,
    ctx: &mut Ctx<'_>,
) -> Result<(), ArchiveError> {
    let file = File::open(archive_path).map_err(|e| io_err(archive_path, e))?;
    let mut reader = ArchiveReader::new_with_memory_limit(file, Password::empty(), MEMORY_LIMIT_KB)
        .map_err(map_error)?;

    let mut declared = 0u64;
    {
        let archive = reader.archive();
        if archive.files.len() > ctx.limits.max_entries {
            return Err(ArchiveError::TooManyEntries {
                limit: ctx.limits.max_entries,
            });
        }
        for (index, entry) in archive.files.iter().enumerate() {
            let name = entry.name();
            if entry.is_directory() && is_root_entry(name) {
                continue;
            }
            if sanitize_entry(name).is_none() {
                return Err(ArchiveError::UnsafePath(name.to_string()));
            }
            if entry.is_anti_item() || is_symlink_entry(entry) {
                return Err(ArchiveError::Link(name.to_string()));
            }
            if archive.is_file_encrypted(index) {
                return Err(ArchiveError::Encrypted);
            }
            declared = declared.saturating_add(entry.size());
        }
    }
    if declared > 0 {
        ctx.set_total(declared);
    }

    // Errors that must survive the trip through the 7z crate's own error type.
    let mut pending: Option<ArchiveError> = None;
    let result = reader.for_each_entries_with_decision(|entry, data| {
        let mut step = || -> Result<(), ArchiveError> {
            ctx.check_cancel()?;
            let name = entry.name();
            if entry.is_directory() && is_root_entry(name) {
                return Ok(());
            }
            let rel =
                sanitize_entry(name).ok_or_else(|| ArchiveError::UnsafePath(name.to_string()))?;
            let out = join_rel(dest, &rel);
            if entry.is_directory() {
                fs::create_dir_all(&out).map_err(|e| io_err(&out, e))?;
                return Ok(());
            }
            create_parent_dirs(&out)?;
            let file = create_output_file(&out, &rel, Backend::SevenZ)?;
            let mut writer = BufWriter::new(file);
            copy_limited(data, &mut writer, ctx, Backend::SevenZ, &out)?;
            writer.flush().map_err(|e| io_err(&out, e))?;
            Ok(())
        };
        match step() {
            Ok(()) => Ok(EntryDecision::Continue),
            Err(err) => {
                pending = Some(err);
                Ok(EntryDecision::Stop)
            }
        }
    });
    if let Some(err) = pending {
        return Err(err);
    }
    result.map_err(map_error)?;
    ctx.report();
    Ok(())
}
