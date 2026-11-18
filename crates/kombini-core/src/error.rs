//! Stable, user-safe application errors shared by the engine, CLI and UI.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Stable error codes. Never rename a variant: the UI and logs key off them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    GameNotConfigured,
    GameNotFound,
    GameRunning,
    GameinfoMissingSearchPaths,
    GameinfoMarkersCorrupt,
    GameinfoReadOnly,
    GameinfoWriteFailed,
    InvalidVpk,
    NoVpkInArchive,
    UnsupportedArchive,
    ArchiveUnsafePath,
    ArchiveTooLarge,
    ArchiveEncrypted,
    ExtractFailed,
    LayoutTooManyRoots,
    LayoutConflict,
    TransactionFailed,
    ModNotFound,
    ProfileNotFound,
    LastProfile,
    SnapshotNotFound,
    Io,
    PermissionDenied,
    Network,
    RateLimited,
    HashMismatch,
    Cancelled,
    InvalidInput,
    Storage,
    Internal,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GameNotConfigured => "game_not_configured",
            Self::GameNotFound => "game_not_found",
            Self::GameRunning => "game_running",
            Self::GameinfoMissingSearchPaths => "gameinfo_missing_search_paths",
            Self::GameinfoMarkersCorrupt => "gameinfo_markers_corrupt",
            Self::GameinfoReadOnly => "gameinfo_read_only",
            Self::GameinfoWriteFailed => "gameinfo_write_failed",
            Self::InvalidVpk => "invalid_vpk",
            Self::NoVpkInArchive => "no_vpk_in_archive",
            Self::UnsupportedArchive => "unsupported_archive",
            Self::ArchiveUnsafePath => "archive_unsafe_path",
            Self::ArchiveTooLarge => "archive_too_large",
            Self::ArchiveEncrypted => "archive_encrypted",
            Self::ExtractFailed => "extract_failed",
            Self::LayoutTooManyRoots => "layout_too_many_roots",
            Self::LayoutConflict => "layout_conflict",
            Self::TransactionFailed => "transaction_failed",
            Self::ModNotFound => "mod_not_found",
            Self::ProfileNotFound => "profile_not_found",
            Self::LastProfile => "last_profile",
            Self::SnapshotNotFound => "snapshot_not_found",
            Self::Io => "io",
            Self::PermissionDenied => "permission_denied",
            Self::Network => "network",
            Self::RateLimited => "rate_limited",
            Self::HashMismatch => "hash_mismatch",
            Self::Cancelled => "cancelled",
            Self::InvalidInput => "invalid_input",
            Self::Storage => "storage",
            Self::Internal => "internal",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An error that is safe to show to the user: a stable code, a human message,
/// and optional technical detail (path, OS error, tool output) for logs and the
/// expandable "details" area in the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
    pub detail: Option<String>,
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.detail {
            Some(detail) => write!(f, "{} ({detail})", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidInput, message)
    }

    pub fn storage(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Storage, message)
    }

    pub fn game_running() -> Self {
        Self::new(
            ErrorCode::GameRunning,
            "Deadlock is running. Close the game before changing mods.",
        )
    }

    pub fn mod_not_found(id: &crate::ModId) -> Self {
        Self::new(ErrorCode::ModNotFound, format!("Mod {id} was not found"))
    }

    pub fn cancelled() -> Self {
        Self::new(ErrorCode::Cancelled, "Cancelled")
    }

    /// Maps an IO error to a user-facing error with advice for the common
    /// Windows failure modes (access denied, sharing violation, disk full).
    pub fn from_io(context: &str, path: &std::path::Path, err: &std::io::Error) -> Self {
        let detail = format!("{}: {err}", path.display());
        match (err.kind(), err.raw_os_error()) {
            (std::io::ErrorKind::PermissionDenied, _) | (_, Some(5)) => Self::new(
                ErrorCode::PermissionDenied,
                format!(
                    "{context}: access was denied. Check that the file is not read-only, that \
                     antivirus (Controlled Folder Access) or OneDrive is not blocking it, and \
                     that Deadlock is closed."
                ),
            )
            .with_detail(detail),
            (_, Some(32 | 33)) => Self::new(
                ErrorCode::Io,
                format!("{context}: the file is in use by another program (is Deadlock running?)"),
            )
            .with_detail(detail),
            (_, Some(112)) | (std::io::ErrorKind::StorageFull, _) => {
                Self::new(ErrorCode::Io, format!("{context}: the disk is full")).with_detail(detail)
            }
            (std::io::ErrorKind::NotFound, _) => {
                Self::new(ErrorCode::Io, format!("{context}: file not found")).with_detail(detail)
            }
            _ => Self::new(ErrorCode::Io, format!("{context}: {err}")).with_detail(detail),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_serialize_snake_case() {
        let json = serde_json::to_string(&ErrorCode::GameinfoReadOnly).unwrap();
        assert_eq!(json, "\"gameinfo_read_only\"");
        assert_eq!(ErrorCode::GameinfoReadOnly.as_str(), "gameinfo_read_only");
    }

    #[test]
    fn io_mapping() {
        let err = std::io::Error::from_raw_os_error(5);
        let app = AppError::from_io("Moving mod", std::path::Path::new("x.vpk"), &err);
        assert_eq!(app.code, ErrorCode::PermissionDenied);
        assert!(app.detail.unwrap().contains("x.vpk"));
        let err = std::io::Error::from_raw_os_error(32);
        let app = AppError::from_io("Moving mod", std::path::Path::new("x.vpk"), &err);
        assert!(app.message.contains("in use"));
    }
}
