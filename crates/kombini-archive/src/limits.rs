//! Extraction limits and the byte/entry accounting shared by all backends.

use std::io::{ErrorKind, Read, Write};
use std::path::Path;

use crate::{ArchiveError, Backend};

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// How many bytes are written between two cancellation checks / progress reports.
const CHECK_INTERVAL: u64 = MIB;

/// Size of the copy buffer.
const COPY_BUF_LEN: usize = 64 * 1024;

/// Zip-bomb limits for one extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Limits {
    /// Maximum total number of extracted bytes.
    pub max_bytes: u64,
    /// Maximum number of archive entries.
    pub max_entries: usize,
}

impl Limits {
    /// `archive_size * 100` clamped to `[256 MiB, 16 GiB]`; at most 10,000 entries.
    pub fn for_archive_size(archive_size: u64) -> Self {
        Limits {
            max_bytes: archive_size.saturating_mul(100).clamp(256 * MIB, 16 * GIB),
            max_entries: 10_000,
        }
    }
}

/// Running state of one extraction: limits, byte counter, cancellation and progress.
pub(crate) struct Ctx<'a> {
    pub(crate) limits: Limits,
    cancel: &'a dyn Fn() -> bool,
    progress: &'a dyn Fn(u64, Option<u64>),
    bytes: u64,
    total: Option<u64>,
    unreported: u64,
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(
        limits: Limits,
        cancel: &'a dyn Fn() -> bool,
        progress: &'a dyn Fn(u64, Option<u64>),
    ) -> Self {
        Ctx {
            limits,
            cancel,
            progress,
            bytes: 0,
            total: None,
            unreported: 0,
        }
    }

    /// Sets the total number of bytes expected (used for progress only).
    pub(crate) fn set_total(&mut self, total: u64) {
        self.total = Some(total);
    }

    pub(crate) fn cancel_fn(&self) -> &'a dyn Fn() -> bool {
        self.cancel
    }

    /// Returns `Err(Cancelled)` when the caller asked to cancel.
    pub(crate) fn check_cancel(&self) -> Result<(), ArchiveError> {
        if (self.cancel)() {
            Err(ArchiveError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Charges `n` bytes against the budget. Fails with `TooLarge` when the budget is exceeded.
    /// Every `CHECK_INTERVAL` bytes it also checks cancellation and reports progress.
    pub(crate) fn charge(&mut self, n: u64) -> Result<(), ArchiveError> {
        self.bytes = self.bytes.saturating_add(n);
        if self.bytes > self.limits.max_bytes {
            return Err(ArchiveError::TooLarge {
                limit: self.limits.max_bytes,
            });
        }
        self.unreported = self.unreported.saturating_add(n);
        if self.unreported >= CHECK_INTERVAL {
            self.unreported = 0;
            self.check_cancel()?;
            (self.progress)(self.bytes, self.total);
        }
        Ok(())
    }

    /// Reports the current byte count to the progress callback.
    pub(crate) fn report(&mut self) {
        self.unreported = 0;
        (self.progress)(self.bytes, self.total);
    }
}

/// Copies `reader` into `writer` in 64 KiB chunks, charging every chunk against the budget
/// BEFORE it is written (so header sizes are never trusted). Read failures become
/// `Failed { backend, .. }`; write failures become `Io { path: out_path, .. }`.
pub(crate) fn copy_limited(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    ctx: &mut Ctx<'_>,
    backend: Backend,
    out_path: &Path,
) -> Result<u64, ArchiveError> {
    let mut buf = vec![0u8; COPY_BUF_LEN];
    let mut written = 0u64;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => {
                return Err(ArchiveError::Failed {
                    backend,
                    message: format!("failed to read entry data: {e}"),
                });
            }
        };
        ctx.charge(n as u64)?;
        writer
            .write_all(&buf[..n])
            .map_err(|source| ArchiveError::Io {
                path: out_path.to_path_buf(),
                source,
            })?;
        written += n as u64;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_archive_size_clamps() {
        assert_eq!(Limits::for_archive_size(1024).max_bytes, 256 * MIB);
        assert_eq!(Limits::for_archive_size(1024).max_entries, 10_000);
        assert_eq!(Limits::for_archive_size(GIB).max_bytes, 16 * GIB);
        assert_eq!(Limits::for_archive_size(u64::MAX).max_bytes, 16 * GIB);
        assert_eq!(Limits::for_archive_size(5 * MIB).max_bytes, 500 * MIB);
    }
}
