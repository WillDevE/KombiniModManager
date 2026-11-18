//! Content-addressed, immutable blob store: `<root>/<hash[0..2]>/<hash>`.

use crate::{Result, StorageError, io_err, random_hex};
use kombini_core::BlobHash;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct GcReport {
    pub removed: usize,
    pub freed_bytes: u64,
    pub kept: usize,
}

const BUF: usize = 1 << 20;

impl BlobStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(root.join("tmp")).map_err(|e| io_err(&root, e))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path_for(&self, hash: &BlobHash) -> PathBuf {
        let h = hash.as_str();
        let prefix = h.get(..2).unwrap_or("xx");
        self.root.join(prefix).join(h)
    }

    pub fn contains(&self, hash: &BlobHash) -> bool {
        self.path_for(hash).is_file()
    }

    /// Streams `source` into the store while hashing. Returns the hash and size.
    /// Existing blobs are not rewritten. `progress(bytes)` is called per MiB.
    pub fn insert_file(
        &self,
        source: &Path,
        cancel: &dyn Fn() -> bool,
        progress: &dyn Fn(u64),
    ) -> Result<(BlobHash, u64)> {
        let mut input = std::fs::File::open(source).map_err(|e| io_err(source, e))?;
        self.insert_reader(&mut input, source, cancel, progress)
    }

    pub fn insert_bytes(&self, bytes: &[u8]) -> Result<(BlobHash, u64)> {
        let mut reader = bytes;
        self.insert_reader(&mut reader, Path::new("<memory>"), &|| false, &|_| {})
    }

    fn insert_reader(
        &self,
        input: &mut dyn Read,
        label: &Path,
        cancel: &dyn Fn() -> bool,
        progress: &dyn Fn(u64),
    ) -> Result<(BlobHash, u64)> {
        let tmp = self
            .root
            .join("tmp")
            .join(format!("{}.part", random_hex(8)));
        let result = (|| {
            let mut out = std::fs::File::create(&tmp).map_err(|e| io_err(&tmp, e))?;
            let mut hasher = Sha256::new();
            let mut buf = vec![0u8; BUF];
            let mut total = 0u64;
            loop {
                if cancel() {
                    return Err(StorageError::Cancelled);
                }
                let n = input.read(&mut buf).map_err(|e| io_err(label, e))?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                out.write_all(&buf[..n]).map_err(|e| io_err(&tmp, e))?;
                total += n as u64;
                progress(total);
            }
            out.sync_all().map_err(|e| io_err(&tmp, e))?;
            Ok((BlobHash::new(hex::encode(hasher.finalize())), total))
        })();
        let (hash, size) = match result {
            Ok(v) => v,
            Err(err) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(err);
            }
        };
        let target = self.path_for(&hash);
        if target.is_file() {
            let _ = std::fs::remove_file(&tmp);
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
            }
            if let Err(err) = std::fs::rename(&tmp, &target) {
                let _ = std::fs::remove_file(&tmp);
                if !target.is_file() {
                    return Err(io_err(&target, err));
                }
            }
        }
        Ok((hash, size))
    }

    /// Copies a blob to `dest` (creating parents), verifying the size.
    pub fn copy_to(&self, hash: &BlobHash, dest: &Path) -> Result<u64> {
        let src = self.path_for(hash);
        if !src.is_file() {
            return Err(StorageError::MissingBlob(hash.to_string()));
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
        }
        std::fs::copy(&src, dest).map_err(|e| io_err(dest, e))
    }

    /// Re-hashes a blob and reports whether it still matches its name.
    pub fn verify(&self, hash: &BlobHash) -> Result<bool> {
        let path = self.path_for(hash);
        let mut input = std::fs::File::open(&path).map_err(|e| io_err(&path, e))?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; BUF];
        loop {
            let n = input.read(&mut buf).map_err(|e| io_err(&path, e))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hex::encode(hasher.finalize()) == hash.as_str())
    }

    /// Deletes blobs not in `referenced` and stale temp files.
    pub fn gc(&self, referenced: &BTreeSet<BlobHash>) -> Result<GcReport> {
        let mut report = GcReport::default();
        let entries = std::fs::read_dir(&self.root).map_err(|e| io_err(&self.root, e))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if !path.is_dir() {
                continue;
            }
            if name == "tmp" {
                if let Ok(tmp) = std::fs::read_dir(&path) {
                    for t in tmp.flatten() {
                        let _ = std::fs::remove_file(t.path());
                    }
                }
                continue;
            }
            let Ok(files) = std::fs::read_dir(&path) else {
                continue;
            };
            for file in files.flatten() {
                let hash = BlobHash::new(file.file_name().to_string_lossy().to_string());
                if referenced.contains(&hash) {
                    report.kept += 1;
                    continue;
                }
                let size = file.metadata().map(|m| m.len()).unwrap_or(0);
                if std::fs::remove_file(file.path()).is_ok() {
                    report.removed += 1;
                    report.freed_bytes += size;
                }
            }
        }
        Ok(report)
    }
}

/// Hashes a file without storing it.
pub fn hash_file(path: &Path) -> Result<(BlobHash, u64)> {
    let mut input = std::fs::File::open(path).map_err(|e| io_err(path, e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; BUF];
    let mut total = 0u64;
    loop {
        let n = input.read(&mut buf).map_err(|e| io_err(path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    Ok((BlobHash::new(hex::encode(hasher.finalize())), total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_copy_dedupe_gc() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::new(dir.path().join("blobs")).unwrap();
        let (a, size) = store.insert_bytes(b"hello").unwrap();
        assert_eq!(size, 5);
        assert_eq!(
            a.as_str(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        let (a2, _) = store.insert_bytes(b"hello").unwrap();
        assert_eq!(a, a2);
        let (b, _) = store.insert_bytes(b"world").unwrap();
        assert!(store.contains(&a) && store.contains(&b));
        assert!(store.verify(&a).unwrap());

        let src = dir.path().join("src.bin");
        std::fs::write(&src, vec![7u8; 3 * BUF + 5]).unwrap();
        let (c, size) = store.insert_file(&src, &|| false, &|_| {}).unwrap();
        assert_eq!(size, (3 * BUF + 5) as u64);
        assert_eq!(hash_file(&src).unwrap().0, c);

        let dest = dir.path().join("out").join("x.vpk");
        store.copy_to(&a, &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"hello");

        let keep: BTreeSet<BlobHash> = [a.clone()].into_iter().collect();
        let report = store.gc(&keep).unwrap();
        assert_eq!(report.removed, 2);
        assert!(store.contains(&a) && !store.contains(&b));
        assert!(matches!(
            store.copy_to(&b, &dest),
            Err(StorageError::MissingBlob(_))
        ));
    }

    #[test]
    fn cancel_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::new(dir.path().join("blobs")).unwrap();
        let src = dir.path().join("src.bin");
        std::fs::write(&src, b"data").unwrap();
        let err = store.insert_file(&src, &|| true, &|_| {}).unwrap_err();
        assert!(matches!(err, StorageError::Cancelled));
        assert_eq!(
            std::fs::read_dir(store.root().join("tmp")).unwrap().count(),
            0
        );
    }
}
