//! Filesystem primitives with Windows lock retries and read-only handling (§13.2).

use kombini_core::AppError;
use std::path::Path;
use std::time::Duration;

const RETRIES: u32 = 5;
const RETRY_DELAY: Duration = Duration::from_millis(150);

fn is_transient(err: &std::io::Error) -> bool {
    matches!(err.raw_os_error(), Some(32 | 33)) || err.kind() == std::io::ErrorKind::WouldBlock
}

/// Runs `op`, retrying transient sharing/lock violations.
pub fn retry<T>(mut op: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut attempt = 0;
    loop {
        match op() {
            Ok(v) => return Ok(v),
            Err(err) if is_transient(&err) && attempt < RETRIES => {
                attempt += 1;
                tracing::warn!(attempt, error = %err, "transient file lock; retrying");
                std::thread::sleep(RETRY_DELAY);
            }
            Err(err) => return Err(err),
        }
    }
}

/// Clears the read-only attribute if set. Returns true when it was cleared.
pub fn clear_read_only(path: &Path) -> std::io::Result<bool> {
    let meta = std::fs::metadata(path)?;
    let mut perms = meta.permissions();
    if perms.readonly() {
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(path, perms)?;
        return Ok(true);
    }
    Ok(false)
}

/// Renames `from` → `to` (same volume), clearing read-only and retrying locks.
/// Fails if `to` exists.
pub fn rename(from: &Path, to: &Path) -> Result<(), AppError> {
    if to.exists() {
        return Err(AppError::new(
            kombini_core::ErrorCode::TransactionFailed,
            "Refusing to overwrite an existing file",
        )
        .with_detail(to.display().to_string()));
    }
    let _ = clear_read_only(from);
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::from_io("Creating folder", parent, &e))?;
    }
    retry(|| std::fs::rename(from, to)).map_err(|e| AppError::from_io("Moving file", from, &e))
}

/// Copies with lock retries, then fsyncs the destination.
pub fn copy(from: &Path, to: &Path) -> Result<u64, AppError> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::from_io("Creating folder", parent, &e))?;
    }
    let n = retry(|| std::fs::copy(from, to))
        .map_err(|e| AppError::from_io("Copying file", from, &e))?;
    if let Ok(f) = std::fs::OpenOptions::new().write(true).open(to) {
        let _ = f.sync_all();
    }
    Ok(n)
}

pub fn remove_file(path: &Path) -> Result<(), AppError> {
    let _ = clear_read_only(path);
    retry(|| std::fs::remove_file(path)).map_err(|e| AppError::from_io("Deleting file", path, &e))
}

/// Writes a file atomically (temp + fsync + rename) in the same directory.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| AppError::invalid("Path has no parent folder"))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| AppError::from_io("Creating folder", parent, &e))?;
    let tmp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        kombini_storage::random_hex(4)
    ));
    let result = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(AppError::from_io("Writing file", path, &e));
    }
    if path.exists() {
        let _ = clear_read_only(path);
    }
    retry(|| std::fs::rename(&tmp, path)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        AppError::from_io("Writing file", path, &e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_refuses_overwrite_and_handles_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.vpk");
        let b = dir.path().join("sub").join("b.vpk");
        std::fs::write(&a, b"x").unwrap();
        let mut perms = std::fs::metadata(&a).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&a, perms).unwrap();
        rename(&a, &b).unwrap();
        assert!(b.exists() && !a.exists());
        std::fs::write(&a, b"y").unwrap();
        assert!(rename(&a, &b).is_err());
        write_atomic(&dir.path().join("w.txt"), b"hello").unwrap();
        write_atomic(&dir.path().join("w.txt"), b"hello2").unwrap();
        assert_eq!(std::fs::read(dir.path().join("w.txt")).unwrap(), b"hello2");
    }
}
