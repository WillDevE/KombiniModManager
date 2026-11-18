//! Error type shared by every module in this crate.

use std::path::PathBuf;

/// Errors produced while reading, writing, extracting or merging VPK files.
#[derive(Debug, thiserror::Error)]
pub enum VpkError {
    #[error("{path}: file is too small to be a VPK")]
    TooSmall { path: PathBuf },
    #[error("{path}: not a VPK (bad signature)")]
    BadSignature { path: PathBuf },
    #[error("{path}: unsupported VPK version {version}")]
    UnsupportedVersion { path: PathBuf, version: u32 },
    #[error("{path}: malformed VPK tree: {reason}")]
    Malformed { path: PathBuf, reason: String },
    #[error("{path}: missing chunk file {chunk}")]
    MissingChunk { path: PathBuf, chunk: String },
    #[error("{path}: entry {entry} is truncated")]
    Truncated { path: PathBuf, entry: String },
    #[error("{path}: CRC mismatch for {entry} (expected {expected:08x}, got {actual:08x})")]
    CrcMismatch {
        path: PathBuf,
        entry: String,
        expected: u32,
        actual: u32,
    },
    #[error("unsafe entry path: {0}")]
    UnsafePath(String),
    #[error("path collision: {0}")]
    Collision(String),
    #[error("{0}")]
    Invalid(String),
    #[error("cancelled")]
    Cancelled,
    #[error("I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, VpkError>;

impl VpkError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        VpkError::Io {
            path: path.into(),
            source,
        }
    }
}
