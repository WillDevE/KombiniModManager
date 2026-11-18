//! ZIP extraction (via the `zip` crate) with up-front validation and counted writes.

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

use zip::ZipArchive;
use zip::result::ZipError;

use crate::limits::{Ctx, copy_limited};
use crate::safety::{
    create_output_file, create_parent_dirs, io_err, is_root_entry, join_rel, sanitize_entry,
};
use crate::{ArchiveError, Backend};

/// Unix file type mask and the symlink file type.
const S_IFMT: u32 = 0o170_000;
const S_IFLNK: u32 = 0o120_000;

struct Planned {
    index: usize,
    rel: String,
    is_dir: bool,
}

fn failed(err: ZipError) -> ArchiveError {
    ArchiveError::Failed {
        backend: Backend::Zip,
        message: err.to_string(),
    }
}

/// Extracts `archive_path` into the (empty) directory `dest`.
///
/// Every entry is validated before anything is written: unsafe names, symlinks, encrypted entries
/// and too many entries are rejected. File bytes are charged against the budget as they are
/// written; header-declared sizes are ignored for enforcement.
pub(crate) fn extract_zip(
    archive_path: &Path,
    dest: &Path,
    ctx: &mut Ctx<'_>,
) -> Result<(), ArchiveError> {
    let file = File::open(archive_path).map_err(|e| io_err(archive_path, e))?;
    let mut zip = ZipArchive::new(BufReader::new(file)).map_err(failed)?;

    let count = zip.len();
    if count > ctx.limits.max_entries {
        return Err(ArchiveError::TooManyEntries {
            limit: ctx.limits.max_entries,
        });
    }

    let mut plan = Vec::with_capacity(count);
    let mut declared = 0u64;
    for index in 0..count {
        let entry = zip.by_index_data(index).map_err(failed)?;
        let raw_name = entry.name().map_err(failed)?.into_owned();
        let is_dir = entry.is_dir() || raw_name.ends_with('/') || raw_name.ends_with('\\');
        if is_dir && is_root_entry(&raw_name) {
            continue;
        }
        let rel =
            sanitize_entry(&raw_name).ok_or_else(|| ArchiveError::UnsafePath(raw_name.clone()))?;
        let unix_symlink = entry.unix_mode().is_some_and(|m| m & S_IFMT == S_IFLNK);
        if entry.is_symlink() || unix_symlink {
            return Err(ArchiveError::Link(raw_name));
        }
        if entry.encrypted() {
            return Err(ArchiveError::Encrypted);
        }
        declared = declared.saturating_add(entry.size());
        plan.push(Planned { index, rel, is_dir });
    }
    if declared > 0 {
        ctx.set_total(declared);
    }

    for item in plan {
        ctx.check_cancel()?;
        let out = join_rel(dest, &item.rel);
        if item.is_dir {
            fs::create_dir_all(&out).map_err(|e| io_err(&out, e))?;
            continue;
        }
        create_parent_dirs(&out)?;
        let mut entry = zip.by_index(item.index).map_err(failed)?;
        let file = create_output_file(&out, &item.rel, Backend::Zip)?;
        let mut writer = BufWriter::new(file);
        copy_limited(&mut entry, &mut writer, ctx, Backend::Zip, &out)?;
        writer.flush().map_err(|e| io_err(&out, e))?;
    }
    ctx.report();
    Ok(())
}
