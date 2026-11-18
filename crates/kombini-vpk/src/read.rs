//! VPK v1/v2 header and directory-tree reader. File data is read lazily.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::error::{Result, VpkError};
use crate::paths::root_base;

/// VPK signature (`0x55AA1234`).
pub const SIGNATURE: u32 = 0x55AA_1234;
/// Archive index meaning "data is embedded after the tree in the root file".
pub const EMBEDDED: u16 = 0x7FFF;
/// Largest directory tree accepted.
pub const MAX_TREE_SIZE: u32 = 256 * 1024 * 1024;
/// Largest number of tree entries accepted.
pub const MAX_ENTRIES: usize = 5_000_000;

const COPY_BUF: usize = 1024 * 1024;
/// Upper bound for speculative buffer pre-allocation (untrusted sizes).
const PREALLOC_LIMIT: u64 = 16 * 1024 * 1024;

/// One directory-tree entry (a path may have several entries; see [`VpkArchive::read_file`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct VpkEntry {
    /// `dir/sub/name.ext` (`name.ext` at the root; no extension gives `dir/name`).
    pub path: String,
    pub crc32: u32,
    /// Offset of the preload bytes within the tree buffer.
    pub preload_offset: u32,
    pub preload_len: u16,
    pub archive_index: u16,
    pub offset: u32,
    pub length: u32,
}

impl VpkEntry {
    /// Total bytes of this entry (preload + payload).
    pub fn size(&self) -> u64 {
        u64::from(self.preload_len) + u64::from(self.length)
    }

    /// True when the payload is embedded in the root file after the tree.
    pub fn is_embedded(&self) -> bool {
        self.archive_index == EMBEDDED
    }
}

/// Result of [`VpkArchive::verify`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct VerifyReport {
    pub checked: usize,
    /// Entry paths whose data could not be read or whose CRC did not match.
    pub failures: Vec<String>,
}

/// A parsed VPK directory tree. Payload data is read on demand.
pub struct VpkArchive {
    path: PathBuf,
    version: u32,
    header_size: u64,
    tree_size: u32,
    tree: Vec<u8>,
    entries: Vec<VpkEntry>,
    chunk_indices: Vec<u16>,
    #[allow(dead_code)]
    file_len: u64,
    path_index: std::sync::OnceLock<HashMap<String, Vec<u32>>>,
}

impl std::fmt::Debug for VpkArchive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VpkArchive")
            .field("path", &self.path)
            .field("version", &self.version)
            .field("entries", &self.entries.len())
            .field("chunk_indices", &self.chunk_indices)
            .finish_non_exhaustive()
    }
}

/// True when the file starts with the VPK signature.
pub fn is_vpk(path: &Path) -> bool {
    let mut buf = [0u8; 4];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut buf))
        .map(|()| u32::from_le_bytes(buf) == SIGNATURE)
        .unwrap_or(false)
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Cursor over the tree buffer; every access is bounds-checked.
struct TreeCursor<'a> {
    tree: &'a [u8],
    pos: usize,
}

impl<'a> TreeCursor<'a> {
    fn string(&mut self) -> std::result::Result<&'a [u8], String> {
        let rest = self
            .tree
            .get(self.pos..)
            .ok_or("tree string starts past the tree")?;
        let len = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or("unterminated string")?;
        self.pos += len + 1;
        Ok(&rest[..len])
    }
}

fn lossy(bytes: &[u8]) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(bytes)
}

struct ParseCtx {
    data_start: u64,
    file_len: u64,
}

fn parse_tree(
    cur: &mut TreeCursor<'_>,
    ctx: &ParseCtx,
    entries: &mut Vec<VpkEntry>,
    seen_chunks: &mut [bool],
) -> std::result::Result<(), String> {
    loop {
        let ext_raw = cur.string()?;
        if ext_raw.is_empty() {
            return Ok(());
        }
        let ext_lossy = lossy(ext_raw);
        let ext: &str = if ext_lossy == " " { "" } else { &ext_lossy };
        loop {
            let dir_raw = cur.string()?;
            if dir_raw.is_empty() {
                break;
            }
            let dir_lossy = lossy(dir_raw);
            let dir: &str = if dir_lossy == " " { "" } else { &dir_lossy };
            loop {
                let base_raw = cur.string()?;
                if base_raw.is_empty() {
                    break;
                }
                let base = lossy(base_raw);
                let mut full = String::with_capacity(dir.len() + base.len() + ext.len() + 2);
                if !dir.is_empty() {
                    full.push_str(dir);
                    full.push('/');
                }
                full.push_str(&base);
                if !ext.is_empty() {
                    full.push('.');
                    full.push_str(ext);
                }
                loop {
                    if cur.pos + 18 > cur.tree.len() {
                        return Err(format!("truncated entry for {full}"));
                    }
                    let p = cur.pos;
                    let crc32 = u32_at(cur.tree, p);
                    let preload_len = u16_at(cur.tree, p + 4);
                    let archive_index = u16_at(cur.tree, p + 6);
                    let length = u32_at(cur.tree, p + 12);
                    // Zero-length entries carry no data; some packers write a
                    // placeholder offset (0xFFFFFFFF) for them, which the game
                    // ignores. Normalize it so it is never range-checked.
                    let offset = if length == 0 {
                        0
                    } else {
                        u32_at(cur.tree, p + 8)
                    };
                    let terminator = u16_at(cur.tree, p + 16);
                    cur.pos = p + 18;
                    if terminator != 0 && terminator != 0xFFFF {
                        return Err(format!(
                            "invalid entry terminator {terminator:#06x} for {full}"
                        ));
                    }
                    let preload_offset = cur.pos;
                    if preload_offset + usize::from(preload_len) > cur.tree.len() {
                        return Err(format!("truncated preload data for {full}"));
                    }
                    cur.pos += usize::from(preload_len);
                    if archive_index == EMBEDDED {
                        let end = ctx.data_start + u64::from(offset) + u64::from(length);
                        if end > ctx.file_len {
                            return Err(format!(
                                "embedded data for {full} extends past end of file"
                            ));
                        }
                    } else {
                        seen_chunks[usize::from(archive_index)] = true;
                    }
                    if entries.len() >= MAX_ENTRIES {
                        return Err("too many entries".to_string());
                    }
                    entries.push(VpkEntry {
                        path: full.clone(),
                        crc32,
                        // The tree is at most 256 MiB, so this always fits.
                        preload_offset: preload_offset as u32,
                        preload_len,
                        archive_index,
                        offset,
                        length,
                    });
                    if terminator == 0xFFFF {
                        break;
                    }
                }
            }
        }
    }
}

impl VpkArchive {
    /// Read the header and directory tree, validating every bound.
    pub fn open(path: &Path) -> Result<Self> {
        let io = |e| VpkError::io(path, e);
        let mut file = File::open(path).map_err(io)?;
        let file_len = file.metadata().map_err(io)?.len();
        if file_len < 12 {
            return Err(VpkError::TooSmall {
                path: path.to_path_buf(),
            });
        }
        let mut head = [0u8; 28];
        file.read_exact(&mut head[..12]).map_err(io)?;
        let signature = u32_at(&head, 0);
        let version = u32_at(&head, 4);
        let tree_size = u32_at(&head, 8);
        if signature != SIGNATURE {
            return Err(VpkError::BadSignature {
                path: path.to_path_buf(),
            });
        }
        if version != 1 && version != 2 {
            return Err(VpkError::UnsupportedVersion {
                path: path.to_path_buf(),
                version,
            });
        }
        let header_size: u64 = if version == 1 { 12 } else { 28 };
        if file_len < header_size {
            return Err(VpkError::TooSmall {
                path: path.to_path_buf(),
            });
        }
        if version == 2 {
            file.read_exact(&mut head[12..28]).map_err(io)?;
        }
        let malformed = |reason: String| VpkError::Malformed {
            path: path.to_path_buf(),
            reason,
        };
        if tree_size == 0 || tree_size > MAX_TREE_SIZE {
            return Err(malformed(format!("implausible tree size {tree_size}")));
        }
        if header_size + u64::from(tree_size) > file_len {
            return Err(malformed("tree extends past end of file".to_string()));
        }
        let mut tree = vec![0u8; tree_size as usize];
        file.read_exact(&mut tree).map_err(io)?;

        let ctx = ParseCtx {
            data_start: header_size + u64::from(tree_size),
            file_len,
        };
        let mut entries: Vec<VpkEntry> = Vec::new();
        let mut seen_chunks = vec![false; 65536];
        let mut cur = TreeCursor {
            tree: &tree,
            pos: 0,
        };
        parse_tree(&mut cur, &ctx, &mut entries, &mut seen_chunks).map_err(malformed)?;

        let chunk_indices = seen_chunks
            .iter()
            .enumerate()
            .filter(|&(_, &seen)| seen)
            .map(|(i, _)| i as u16)
            .collect();
        Ok(Self {
            path: path.to_path_buf(),
            version,
            header_size,
            tree_size,
            tree,
            entries,
            chunk_indices,
            file_len,
            path_index: std::sync::OnceLock::new(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn entries(&self) -> &[VpkEntry] {
        &self.entries
    }

    /// Sorted unique non-embedded archive indices referenced by the tree.
    pub fn chunk_indices(&self) -> &[u16] {
        &self.chunk_indices
    }

    /// Unique entry paths in first-seen order.
    pub fn file_paths(&self) -> Vec<&str> {
        let mut seen = std::collections::HashSet::with_capacity(self.entries.len());
        let mut out = Vec::new();
        for e in &self.entries {
            if seen.insert(e.path.as_str()) {
                out.push(e.path.as_str());
            }
        }
        out
    }

    /// Path of the chunk file `<base>_<index:03>.vpk` next to the root file.
    pub fn chunk_path(&self, index: u16) -> PathBuf {
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        let base = root_base(name.as_deref().unwrap_or(""));
        self.path.with_file_name(format!("{base}_{index:03}.vpk"))
    }

    /// Referenced chunk files that do not exist.
    pub fn missing_chunks(&self) -> Vec<PathBuf> {
        self.chunk_indices
            .iter()
            .map(|&i| self.chunk_path(i))
            .filter(|p| !p.is_file())
            .collect()
    }

    pub(crate) fn preload(&self, entry: &VpkEntry) -> &[u8] {
        let start = entry.preload_offset as usize;
        self.tree
            .get(start..start + usize::from(entry.preload_len))
            .unwrap_or(&[])
    }

    /// Whether the archive has a file at `path` (case-insensitive fallback).
    pub fn contains(&self, path: &str) -> bool {
        !self.entries_for(path).is_empty()
    }

    /// All entries whose path equals `path` (exact match first, else case-insensitive), in tree
    /// order. Empty when nothing matches.
    pub(crate) fn entries_for(&self, path: &str) -> Vec<&VpkEntry> {
        let index = self.path_index.get_or_init(|| {
            let mut map: HashMap<String, Vec<u32>> = HashMap::with_capacity(self.entries.len());
            for (i, e) in self.entries.iter().enumerate() {
                map.entry(e.path.clone()).or_default().push(i as u32);
            }
            map
        });
        let pick = |key: &str| -> Vec<&VpkEntry> {
            index
                .get(key)
                .map(|v| v.iter().map(|&i| &self.entries[i as usize]).collect())
                .unwrap_or_default()
        };
        let exact = pick(path);
        if !exact.is_empty() {
            return exact;
        }
        let lower = path.to_lowercase();
        match self.entries.iter().find(|e| e.path.to_lowercase() == lower) {
            Some(first) => pick(&first.path),
            None => Vec::new(),
        }
    }

    /// Unique paths in first-seen order with the indices of their entries.
    pub(crate) fn groups(&self) -> Vec<(&str, Vec<usize>)> {
        let mut index: HashMap<&str, usize> = HashMap::with_capacity(self.entries.len());
        let mut out: Vec<(&str, Vec<usize>)> = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            match index.get(e.path.as_str()) {
                Some(&g) => out[g].1.push(i),
                None => {
                    index.insert(e.path.as_str(), out.len());
                    out.push((e.path.as_str(), vec![i]));
                }
            }
        }
        out
    }

    /// Read one entry: preload bytes followed by the payload.
    pub fn read_entry(&self, entry: &VpkEntry) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(entry.size().min(PREALLOC_LIMIT) as usize);
        let mut reader = Reader::new(self);
        reader.stream_entry(entry, &mut |b| {
            out.extend_from_slice(b);
            Ok(())
        })?;
        Ok(out)
    }

    /// Read a whole file by path, concatenating all of its entries in tree order. An exact
    /// (case-sensitive) match is preferred, else a case-insensitive match is used.
    pub fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        let entries = self.entries_for(path);
        if entries.is_empty() {
            return Err(VpkError::Invalid(format!(
                "{}: entry not found: {path}",
                self.path.display()
            )));
        }
        let total: u64 = entries.iter().map(|e| e.size()).sum();
        let mut out = Vec::with_capacity(total.min(PREALLOC_LIMIT) as usize);
        let mut reader = Reader::new(self);
        for entry in entries {
            reader.stream_entry(entry, &mut |b| {
                out.extend_from_slice(b);
                Ok(())
            })?;
        }
        Ok(out)
    }

    /// Check the CRC32 of every unique path. Read problems are reported as failures.
    pub fn verify(&self, cancel: &dyn Fn() -> bool) -> Result<VerifyReport> {
        let mut reader = Reader::new(self);
        let mut failures = Vec::new();
        let groups = self.groups();
        for (path, idxs) in &groups {
            if cancel() {
                return Err(VpkError::Cancelled);
            }
            let entries: Vec<&VpkEntry> = idxs.iter().map(|&i| &self.entries[i]).collect();
            match reader.stream_group(&entries, &mut |_| Ok(())) {
                Ok(check) if check.ok() => {}
                Ok(_) => failures.push((*path).to_string()),
                Err(VpkError::Cancelled) => return Err(VpkError::Cancelled),
                Err(err) => {
                    tracing::warn!(%err, entry = *path, "VPK verify could not read entry");
                    failures.push((*path).to_string());
                }
            }
        }
        Ok(VerifyReport {
            checked: groups.len(),
            failures,
        })
    }
}

/// CRC outcome of streaming all entries of one path.
pub(crate) struct GroupCheck {
    /// CRC declared by the first entry.
    pub expected: u32,
    /// CRC of the concatenated data.
    pub actual: u32,
    /// True when every fragment matched its own entry CRC.
    pub fragments_ok: bool,
}

impl GroupCheck {
    /// A path passes when the whole-file CRC matches the first entry or every fragment matches
    /// its own entry.
    pub fn ok(&self) -> bool {
        self.expected == self.actual || self.fragments_ok
    }
}

/// Sequential data reader that caches open chunk files.
pub(crate) struct Reader<'a> {
    archive: &'a VpkArchive,
    files: HashMap<u16, (File, u64)>,
    buf: Vec<u8>,
}

impl<'a> Reader<'a> {
    pub fn new(archive: &'a VpkArchive) -> Self {
        Self {
            archive,
            files: HashMap::new(),
            buf: Vec::new(),
        }
    }

    fn target_path(&self, index: u16) -> PathBuf {
        if index == EMBEDDED {
            self.archive.path.clone()
        } else {
            self.archive.chunk_path(index)
        }
    }

    fn ensure_open(&mut self, index: u16) -> Result<()> {
        if self.files.contains_key(&index) {
            return Ok(());
        }
        let path = self.target_path(index);
        let file = File::open(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound && index != EMBEDDED {
                VpkError::MissingChunk {
                    path: self.archive.path.clone(),
                    chunk: path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                }
            } else {
                VpkError::io(&path, e)
            }
        })?;
        let len = file.metadata().map_err(|e| VpkError::io(&path, e))?.len();
        self.files.insert(index, (file, len));
        Ok(())
    }

    /// Stream preload then payload of one entry to `sink` in bounded pieces.
    pub fn stream_entry(
        &mut self,
        entry: &VpkEntry,
        sink: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        let archive = self.archive;
        let preload = archive.preload(entry);
        if !preload.is_empty() {
            sink(preload)?;
        }
        if entry.length == 0 {
            return Ok(());
        }
        let truncated = || VpkError::Truncated {
            path: archive.path.clone(),
            entry: entry.path.clone(),
        };
        let start = if entry.is_embedded() {
            archive.header_size + u64::from(archive.tree_size) + u64::from(entry.offset)
        } else {
            u64::from(entry.offset)
        };
        self.ensure_open(entry.archive_index)?;
        let target = self.target_path(entry.archive_index);
        let Some((file, len)) = self.files.get_mut(&entry.archive_index) else {
            return Err(truncated());
        };
        if start + u64::from(entry.length) > *len {
            return Err(truncated());
        }
        file.seek(SeekFrom::Start(start))
            .map_err(|e| VpkError::io(&target, e))?;
        let mut remaining = entry.length as usize;
        let want = COPY_BUF.min(remaining);
        if self.buf.len() < want {
            self.buf.resize(want, 0);
        }
        while remaining > 0 {
            let n = remaining.min(self.buf.len());
            file.read_exact(&mut self.buf[..n]).map_err(|e| {
                if e.kind() == std::io::ErrorKind::UnexpectedEof {
                    truncated()
                } else {
                    VpkError::io(&target, e)
                }
            })?;
            sink(&self.buf[..n])?;
            remaining -= n;
        }
        Ok(())
    }

    /// Stream every entry of one path in order and compute CRCs.
    pub fn stream_group(
        &mut self,
        entries: &[&VpkEntry],
        sink: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<GroupCheck> {
        let mut whole = crc32fast::Hasher::new();
        let mut fragments_ok = true;
        for entry in entries {
            let mut frag = crc32fast::Hasher::new();
            self.stream_entry(entry, &mut |b| {
                whole.update(b);
                frag.update(b);
                sink(b)
            })?;
            if frag.finalize() != entry.crc32 {
                fragments_ok = false;
            }
        }
        Ok(GroupCheck {
            expected: entries.first().map_or(0, |e| e.crc32),
            actual: whole.finalize(),
            fragments_ok,
        })
    }
}
