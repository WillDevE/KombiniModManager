//! Streaming writer for single-file VPK v2 archives with embedded data.

use std::collections::{BTreeMap, HashMap};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use md5::{Digest, Md5};

use crate::error::{Result, VpkError};
use crate::extract::safe_relpath;
use crate::read::{EMBEDDED, MAX_TREE_SIZE, Reader, SIGNATURE, VpkArchive};

const COPY_BUF: usize = 1024 * 1024;

/// Where the bytes of one output entry come from.
pub enum EntrySource {
    Bytes(Vec<u8>),
    File(PathBuf),
    FromVpk {
        archive: Arc<VpkArchive>,
        path: String,
    },
}

/// Summary of a finished write.
#[derive(Debug, Clone)]
pub struct WriteReport {
    pub entries: usize,
    /// Total size of the written file.
    pub bytes: u64,
    pub output: PathBuf,
}

struct Item {
    path: String,
    ext: String,
    dir: String,
    base: String,
    source: EntrySource,
    size: u64,
    offset: u32,
}

/// Normalize an output entry path (`\` to `/`, leading `/` stripped) and validate it can be
/// represented in a VPK tree.
pub(crate) fn normalize_entry_path(name: &str) -> Result<String> {
    let replaced = name.replace('\\', "/");
    let trimmed = replaced.trim_start_matches('/');
    let safe = safe_relpath(trimmed).ok_or_else(|| VpkError::UnsafePath(name.to_string()))?;
    let (dir, file) = safe.rsplit_once('/').unwrap_or(("", safe.as_str()));
    let Some((base, ext)) = file.rsplit_once('.') else {
        return Err(VpkError::Invalid(format!(
            "VPK entry has no extension: {safe}"
        )));
    };
    if ext.is_empty() || ext == " " {
        return Err(VpkError::Invalid(format!(
            "VPK entry has an empty extension: {safe}"
        )));
    }
    if base.is_empty() {
        return Err(VpkError::Invalid(format!(
            "VPK entry has an empty file name: {safe}"
        )));
    }
    if dir == " " {
        return Err(VpkError::Invalid(format!(
            "VPK entry has an unrepresentable directory: {safe}"
        )));
    }
    Ok(safe)
}

fn source_size(src: &EntrySource) -> Result<u64> {
    match src {
        EntrySource::Bytes(b) => Ok(b.len() as u64),
        EntrySource::File(p) => {
            let meta = std::fs::metadata(p).map_err(|e| VpkError::io(p, e))?;
            if !meta.is_file() {
                return Err(VpkError::Invalid(format!("not a file: {}", p.display())));
            }
            Ok(meta.len())
        }
        EntrySource::FromVpk { archive, path } => {
            let entries = archive.entries_for(path);
            if entries.is_empty() {
                return Err(VpkError::Invalid(format!(
                    "{}: entry not found: {path}",
                    archive.path().display()
                )));
            }
            Ok(entries.iter().map(|e| e.size()).sum())
        }
    }
}

type Readers<'a> = HashMap<usize, Reader<'a>>;

fn stream_source<'a>(
    src: &'a EntrySource,
    readers: &mut Readers<'a>,
    buf: &mut Vec<u8>,
    sink: &mut dyn FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    match src {
        EntrySource::Bytes(b) => sink(b),
        EntrySource::File(p) => {
            let mut f = File::open(p).map_err(|e| VpkError::io(p, e))?;
            if buf.len() < COPY_BUF {
                buf.resize(COPY_BUF, 0);
            }
            loop {
                let n = f.read(buf).map_err(|e| VpkError::io(p, e))?;
                if n == 0 {
                    return Ok(());
                }
                sink(&buf[..n])?;
            }
        }
        EntrySource::FromVpk { archive, path } => {
            let reader = readers
                .entry(Arc::as_ptr(archive) as usize)
                .or_insert_with(|| Reader::new(archive));
            for entry in archive.entries_for(path) {
                reader.stream_entry(entry, sink)?;
            }
            Ok(())
        }
    }
}

fn temp_suffix() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    h.write_u64(nanos);
    h.write_u32(std::process::id());
    (h.finish() & 0xFFFF_FFFF) as u32
}

/// Write a VPK v2 with all data embedded. Output is deterministic for identical input.
pub fn write_vpk(
    output: &Path,
    entries: Vec<(String, EntrySource)>,
    cancel: &dyn Fn() -> bool,
) -> Result<WriteReport> {
    write_vpk_inner(output, entries, cancel).map(|(report, _)| report)
}

/// Like [`write_vpk`] but also returns the CRC32 written for each normalized path.
pub(crate) fn write_vpk_inner(
    output: &Path,
    entries: Vec<(String, EntrySource)>,
    cancel: &dyn Fn() -> bool,
) -> Result<(WriteReport, HashMap<String, u32>)> {
    if entries.is_empty() {
        return Err(VpkError::Invalid("cannot write an empty VPK".to_string()));
    }
    let mut items: Vec<Item> = Vec::with_capacity(entries.len());
    for (name, source) in entries {
        let path = normalize_entry_path(&name)?;
        let (dir, file) = path.rsplit_once('/').unwrap_or(("", path.as_str()));
        let (base, ext) = file.rsplit_once('.').unwrap_or((file, ""));
        let (dir, base, ext) = (dir.to_string(), base.to_string(), ext.to_string());
        items.push(Item {
            path,
            ext,
            dir,
            base,
            source,
            size: 0,
            offset: 0,
        });
    }
    items.sort_by(|a, b| a.path.cmp(&b.path));
    if let Some(w) = items.windows(2).find(|w| w[0].path == w[1].path) {
        return Err(VpkError::Collision(w[0].path.clone()));
    }

    // Sizes and offsets.
    let mut running: u64 = 0;
    for item in &mut items {
        item.size = source_size(&item.source)?;
        if item.size > u64::from(u32::MAX) {
            return Err(VpkError::Invalid(format!(
                "entry is too large for a VPK: {}",
                item.path
            )));
        }
        item.offset = u32::try_from(running).map_err(|_| data_too_large())?;
        running += item.size;
        if running > u64::from(u32::MAX) {
            return Err(data_too_large());
        }
    }
    let data_size = running;

    // First pass: CRC of every source.
    let mut crcs: Vec<u32> = Vec::with_capacity(items.len());
    {
        let mut readers: Readers<'_> = HashMap::new();
        let mut buf: Vec<u8> = Vec::new();
        for item in &items {
            if cancel() {
                return Err(VpkError::Cancelled);
            }
            let mut hasher = crc32fast::Hasher::new();
            let mut seen: u64 = 0;
            stream_source(&item.source, &mut readers, &mut buf, &mut |b| {
                if cancel() {
                    return Err(VpkError::Cancelled);
                }
                hasher.update(b);
                seen += b.len() as u64;
                Ok(())
            })?;
            if seen != item.size {
                return Err(source_changed(&item.path));
            }
            crcs.push(hasher.finalize());
        }
    }

    // Tree: extension -> directory (" " for root) -> basenames, all sorted bytewise.
    let mut grouped: BTreeMap<&str, BTreeMap<&str, Vec<usize>>> = BTreeMap::new();
    for (i, item) in items.iter().enumerate() {
        let dir = if item.dir.is_empty() {
            " "
        } else {
            item.dir.as_str()
        };
        grouped
            .entry(item.ext.as_str())
            .or_default()
            .entry(dir)
            .or_default()
            .push(i);
    }
    let mut tree: Vec<u8> = Vec::new();
    for (ext, dirs) in &mut grouped {
        tree.extend_from_slice(ext.as_bytes());
        tree.push(0);
        for (dir, idxs) in dirs {
            tree.extend_from_slice(dir.as_bytes());
            tree.push(0);
            idxs.sort_by(|&a, &b| items[a].base.cmp(&items[b].base));
            for &i in idxs.iter() {
                let item = &items[i];
                tree.extend_from_slice(item.base.as_bytes());
                tree.push(0);
                tree.extend_from_slice(&crcs[i].to_le_bytes());
                tree.extend_from_slice(&0u16.to_le_bytes());
                tree.extend_from_slice(&EMBEDDED.to_le_bytes());
                tree.extend_from_slice(&item.offset.to_le_bytes());
                tree.extend_from_slice(&(item.size as u32).to_le_bytes());
                tree.extend_from_slice(&0xFFFFu16.to_le_bytes());
            }
            tree.push(0);
        }
        tree.push(0);
    }
    tree.push(0);
    let tree_size = u32::try_from(tree.len())
        .ok()
        .filter(|&n| n <= MAX_TREE_SIZE)
        .ok_or_else(|| VpkError::Invalid("VPK directory tree is too large".to_string()))?;

    let mut header = Vec::with_capacity(28);
    for v in [SIGNATURE, 2, tree_size, data_size as u32, 0, 48, 0] {
        header.extend_from_slice(&v.to_le_bytes());
    }

    // Second pass: write to a temp file next to the output.
    let parent = match output.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    std::fs::create_dir_all(&parent).map_err(|e| VpkError::io(&parent, e))?;
    let file_name = output
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| VpkError::Invalid(format!("invalid output path: {}", output.display())))?;
    let (temp_path, temp_file) = create_temp(&parent, &file_name)?;

    let result = write_body(temp_file, &temp_path, &header, &tree, &items, &crcs, cancel);
    let total = match result {
        Ok(total) => total,
        Err(e) => {
            let _ = std::fs::remove_file(&temp_path);
            return Err(e);
        }
    };
    if let Err(e) = std::fs::rename(&temp_path, output) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(VpkError::io(output, e));
    }
    let crc_map = items
        .iter()
        .zip(crcs)
        .map(|(i, c)| (i.path.clone(), c))
        .collect();
    Ok((
        WriteReport {
            entries: items.len(),
            bytes: total,
            output: output.to_path_buf(),
        },
        crc_map,
    ))
}

fn data_too_large() -> VpkError {
    VpkError::Invalid("VPK data section exceeds 4 GiB".to_string())
}

fn source_changed(path: &str) -> VpkError {
    VpkError::Invalid(format!("source changed while writing: {path}"))
}

fn create_temp(parent: &Path, file_name: &str) -> Result<(PathBuf, File)> {
    let mut last_err = None;
    for _ in 0..16 {
        let path = parent.join(format!("{file_name}.{:08x}.tmp", temp_suffix()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => return Ok((path, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last_err = Some((path, e)),
            Err(e) => return Err(VpkError::io(path, e)),
        }
    }
    match last_err {
        Some((path, e)) => Err(VpkError::io(path, e)),
        None => Err(VpkError::Invalid(
            "could not create a temporary file".to_string(),
        )),
    }
}

fn write_body(
    file: File,
    temp_path: &Path,
    header: &[u8],
    tree: &[u8],
    items: &[Item],
    crcs: &[u32],
    cancel: &dyn Fn() -> bool,
) -> Result<u64> {
    let io = |e| VpkError::io(temp_path, e);
    let mut out = BufWriter::with_capacity(COPY_BUF, file);
    let mut whole = Md5::new();
    let mut written: u64 = 0;

    out.write_all(header).map_err(io)?;
    out.write_all(tree).map_err(io)?;
    whole.update(header);
    whole.update(tree);
    written += (header.len() + tree.len()) as u64;

    let mut readers: Readers<'_> = HashMap::new();
    let mut buf: Vec<u8> = Vec::new();
    for (item, &expected_crc) in items.iter().zip(crcs) {
        if cancel() {
            return Err(VpkError::Cancelled);
        }
        let mut hasher = crc32fast::Hasher::new();
        let mut seen: u64 = 0;
        stream_source(&item.source, &mut readers, &mut buf, &mut |b| {
            if cancel() {
                return Err(VpkError::Cancelled);
            }
            out.write_all(b).map_err(io)?;
            whole.update(b);
            hasher.update(b);
            seen += b.len() as u64;
            Ok(())
        })?;
        if seen != item.size || hasher.finalize() != expected_crc {
            return Err(source_changed(&item.path));
        }
        written += seen;
    }

    let tree_md5 = Md5::digest(tree);
    let archive_md5_section_md5 = Md5::digest([]);
    let mut section = Vec::with_capacity(48);
    section.extend_from_slice(&tree_md5);
    section.extend_from_slice(&archive_md5_section_md5);
    whole.update(&section[..32]);
    section.extend_from_slice(&whole.finalize());
    out.write_all(&section).map_err(io)?;
    written += section.len() as u64;

    let file = out
        .into_inner()
        .map_err(|e| VpkError::io(temp_path, e.into_error()))?;
    file.sync_all().map_err(io)?;
    Ok(written)
}
