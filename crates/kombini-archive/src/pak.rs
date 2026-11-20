//! Quake-style `PACK` files: a 12-byte header (`PACK`, directory offset,
//! directory length) and a directory of 64-byte entries (a NUL-padded 56-byte
//! name, a u32 offset and a u32 size). Some Deadlock voice packs ship their
//! WAV files this way.

use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::limits::{Ctx, copy_limited};
use crate::safety::join_rel;
use crate::safety::{create_output_file, create_parent_dirs, ensure_no_link_ancestors, io_err};
use crate::{ArchiveError, Backend, sanitize_entry};

const ENTRY_LEN: u64 = 64;
const NAME_LEN: usize = 56;

fn failed(message: impl Into<String>) -> ArchiveError {
    ArchiveError::Failed {
        backend: Backend::Pak,
        message: message.into(),
    }
}

fn u32_at(b: &[u8], at: usize) -> u64 {
    u64::from(u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]))
}

struct Entry {
    rel: String,
    offset: u64,
    size: u64,
}

/// Reads the directory of the `PACK` file at `path`, validating every entry.
fn read_dir(file: &mut File, path: &Path, max_entries: usize) -> Result<Vec<Entry>, ArchiveError> {
    let len = file.metadata().map_err(|e| io_err(path, e))?.len();
    let mut head = [0u8; 12];
    file.read_exact(&mut head)
        .map_err(|_| failed("file is shorter than a PACK header"))?;
    if &head[0..4] != b"PACK" {
        return Err(failed("missing PACK signature"));
    }
    let dir_off = u32_at(&head, 4);
    let dir_len = u32_at(&head, 8);
    if !dir_len.is_multiple_of(ENTRY_LEN) || dir_off.saturating_add(dir_len) > len {
        return Err(failed("PACK directory is out of range"));
    }
    let count = (dir_len / ENTRY_LEN) as usize;
    if count > max_entries {
        return Err(ArchiveError::TooManyEntries { limit: max_entries });
    }
    file.seek(SeekFrom::Start(dir_off))
        .map_err(|e| io_err(path, e))?;
    let mut dir = vec![0u8; dir_len as usize];
    file.read_exact(&mut dir).map_err(|e| io_err(path, e))?;
    let mut entries = Vec::with_capacity(count);
    for raw in dir.as_chunks::<{ ENTRY_LEN as usize }>().0 {
        let name_bytes = &raw[..NAME_LEN];
        let end = name_bytes.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        let name = String::from_utf8_lossy(&name_bytes[..end]).into_owned();
        let offset = u32_at(raw, NAME_LEN);
        let size = u32_at(raw, NAME_LEN + 4);
        if offset.saturating_add(size) > len {
            return Err(failed(format!("entry {name} is out of range")));
        }
        let rel = sanitize_entry(&name).ok_or_else(|| ArchiveError::UnsafePath(name.clone()))?;
        entries.push(Entry { rel, offset, size });
    }
    Ok(entries)
}

/// Extracts every file of the `PACK` file at `archive` into `dest`.
pub(crate) fn extract_pak(
    archive: &Path,
    dest: &Path,
    ctx: &mut Ctx<'_>,
) -> Result<(), ArchiveError> {
    let mut file = File::open(archive).map_err(|e| io_err(archive, e))?;
    let entries = read_dir(&mut file, archive, ctx.limits.max_entries)?;
    ctx.set_total(entries.iter().map(|e| e.size).sum());
    for entry in entries {
        ctx.check_cancel()?;
        ensure_no_link_ancestors(dest, &entry.rel)?;
        let out = join_rel(dest, &entry.rel);
        create_parent_dirs(&out)?;
        file.seek(SeekFrom::Start(entry.offset))
            .map_err(|e| io_err(archive, e))?;
        let dst = create_output_file(&out, &entry.rel, Backend::Pak)?;
        let mut writer = BufWriter::new(dst);
        copy_limited(
            &mut (&mut file).take(entry.size),
            &mut writer,
            ctx,
            Backend::Pak,
            &out,
        )?;
        writer.flush().map_err(|e| io_err(&out, e))?;
    }
    ctx.report();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Limits;
    use std::fs;

    /// Builds a `PACK` file holding `files`.
    pub(crate) fn make_pak(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = vec![0u8; 12];
        let mut dir = Vec::new();
        for (name, data) in files {
            let offset = out.len() as u32;
            out.extend_from_slice(data);
            let mut e = [0u8; 64];
            e[..name.len()].copy_from_slice(name.as_bytes());
            e[56..60].copy_from_slice(&offset.to_le_bytes());
            e[60..64].copy_from_slice(&(data.len() as u32).to_le_bytes());
            dir.extend_from_slice(&e);
        }
        let dir_off = out.len() as u32;
        out.extend_from_slice(&dir);
        out[0..4].copy_from_slice(b"PACK");
        out[4..8].copy_from_slice(&dir_off.to_le_bytes());
        out[8..12].copy_from_slice(&(dir.len() as u32).to_le_bytes());
        out
    }

    fn run(bytes: &[u8]) -> (tempfile::TempDir, Result<(), ArchiveError>) {
        let tmp = tempfile::tempdir().unwrap();
        let pak = tmp.path().join("x.pak");
        fs::write(&pak, bytes).unwrap();
        let dest = tmp.path().join("out");
        fs::create_dir_all(&dest).unwrap();
        let limits = Limits::for_archive_size(bytes.len() as u64);
        let cancel = || false;
        let progress = |_: u64, _: Option<u64>| {};
        let mut ctx = Ctx::new(limits, &cancel, &progress);
        let r = extract_pak(&pak, &dest, &mut ctx);
        (tmp, r)
    }

    #[test]
    fn extracts_entries() {
        let bytes = make_pak(&[("a.wav", b"hello"), ("sub/b.wav", b"world!")]);
        assert_eq!(crate::detect_bytes(&bytes), crate::ArchiveKind::Pak);
        let (tmp, r) = run(&bytes);
        r.unwrap();
        let out = tmp.path().join("out");
        assert_eq!(fs::read(out.join("a.wav")).unwrap(), b"hello");
        assert_eq!(fs::read(out.join("sub").join("b.wav")).unwrap(), b"world!");
    }

    #[test]
    fn rejects_traversal_and_bad_ranges() {
        let (_t, r) = run(&make_pak(&[("../evil.wav", b"x")]));
        assert!(matches!(r, Err(ArchiveError::UnsafePath(_))));
        let mut bytes = make_pak(&[("a.wav", b"hello")]);
        let n = bytes.len();
        bytes[n - 4..].copy_from_slice(&1000u32.to_le_bytes());
        let (_t, r) = run(&bytes);
        assert!(matches!(r, Err(ArchiveError::Failed { .. })));
    }
}
