//! SQLite storage and content-addressed blob store (ARCHITECTURE §5).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod blobs;
pub mod db;
mod schema;

pub use blobs::{BlobStore, GcReport, hash_file};
pub use db::{
    ActionRecord, DownloadRow, GameRow, LaunchRow, LibraryState, OpenReport, SnapshotRow, Storage,
};
pub use schema::SCHEMA_VERSION;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("corrupt record {what}: {source}")]
    Json {
        what: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("I/O error on {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("blob {0} not found")]
    MissingBlob(String),
    #[error("cancelled")]
    Cancelled,
}

impl From<StorageError> for kombini_core::AppError {
    fn from(err: StorageError) -> Self {
        match &err {
            StorageError::Io { path, source } => {
                kombini_core::AppError::from_io("Storage", path, source)
            }
            StorageError::Cancelled => kombini_core::AppError::cancelled(),
            _ => kombini_core::AppError::storage(err.to_string()),
        }
    }
}

pub type Result<T> = std::result::Result<T, StorageError>;

pub(crate) fn io_err(path: &std::path::Path, source: std::io::Error) -> StorageError {
    StorageError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Current unix time in seconds.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Random lowercase hex string of `bytes * 2` characters.
pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::fill(&mut buf[..]);
    hex::encode(buf)
}
