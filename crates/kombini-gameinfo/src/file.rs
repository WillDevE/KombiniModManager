//! Reading, status reporting and atomic writing of `gameinfo.gi`.

use std::collections::hash_map::RandomState;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use serde::Serialize;

use crate::KEYBINDING_WARNING;
use crate::edit::{
    EditError, ensure_text, remove_other_manager_mounts_text, repair_markers_text, unpatch_text,
};
use crate::parse::{Analysis, analyze};
use crate::text::{DecodedText, decode, encode};

const RENAME_RETRIES: u32 = 5;
const RENAME_RETRY_DELAY: Duration = Duration::from_millis(150);
const TEMP_NAME_ATTEMPTS: u32 = 16;

/// Machine-readable kind of a status issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueCode {
    Missing,
    Unreadable,
    MissingSearchPaths,
    AddonsNotMounted,
    AddonsBelowBase,
    MissingModWrite,
    ReadOnly,
    MarkersCorrupt,
    OtherManager,
    ObsoleteGameLanguage,
    DuplicateAddonMounts,
}

/// A problem found by [`GameInfoFile::status`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Issue {
    pub code: IssueCode,
    pub message: String,
}

/// Snapshot of the gameinfo.gi file; never fails, problems are listed in `issues`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GameInfoStatus {
    pub path: PathBuf,
    pub exists: bool,
    pub readable: bool,
    pub read_only: bool,
    pub has_bom: bool,
    pub crlf: bool,
    pub analysis: Option<Analysis>,
    pub patched: bool,
    pub issues: Vec<Issue>,
}

/// What a write operation did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WriteReport {
    pub changed: bool,
    pub backup_created: bool,
    pub cleared_read_only: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum GameInfoError {
    #[error("gameinfo.gi not found at {0}")]
    NotFound(PathBuf),
    #[error(transparent)]
    Edit(#[from] EditError),
    #[error("gameinfo.gi is read-only and could not be made writable: {0}")]
    ReadOnly(String),
    #[error("could not write gameinfo.gi: {0}")]
    Io(#[from] io::Error),
}

/// A `gameinfo.gi` on disk.
pub struct GameInfoFile {
    path: PathBuf,
}

fn issue(code: IssueCode, message: impl Into<String>) -> Issue {
    Issue {
        code,
        message: message.into(),
    }
}

impl GameInfoFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `<root>/game/citadel/gameinfo.gi`
    pub fn for_game_root(game_root: &Path) -> Self {
        Self::new(game_root.join("game").join("citadel").join("gameinfo.gi"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `<path>.bak`
    pub fn backup_path(&self) -> PathBuf {
        let mut name = self.path.clone().into_os_string();
        name.push(".bak");
        PathBuf::from(name)
    }

    /// Never errors; `issues` describe problems.
    pub fn status(&self) -> GameInfoStatus {
        let mut status = GameInfoStatus {
            path: self.path.clone(),
            exists: false,
            readable: false,
            read_only: false,
            has_bom: false,
            crlf: false,
            analysis: None,
            patched: false,
            issues: Vec::new(),
        };
        let metadata = match fs::metadata(&self.path) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                status
                    .issues
                    .push(issue(IssueCode::Missing, "gameinfo.gi was not found"));
                return status;
            }
            Err(e) => {
                status.exists = self.path.exists();
                status.issues.push(issue(
                    IssueCode::Unreadable,
                    format!("Could not read gameinfo.gi: {e}"),
                ));
                return status;
            }
        };
        status.exists = true;
        status.read_only = metadata.permissions().readonly();
        let bytes = match fs::read(&self.path) {
            Ok(b) => b,
            Err(e) => {
                status.issues.push(issue(
                    IssueCode::Unreadable,
                    format!("Could not read gameinfo.gi: {e}"),
                ));
                return status;
            }
        };
        status.readable = true;
        let decoded = decode(&bytes);
        status.has_bom = decoded.bom;
        status.crlf = decoded.crlf;
        let analysis = analyze(&decoded.text);
        status.patched = analysis.patched;

        if !analysis.search_paths_found {
            status.issues.push(issue(
                IssueCode::MissingSearchPaths,
                "SearchPaths block not found; the file was left unchanged",
            ));
        } else {
            if !analysis.markers_ok {
                status.issues.push(issue(
                    IssueCode::MarkersCorrupt,
                    "Kombini's managed block markers are damaged; use Repair markers",
                ));
            }
            if analysis.addon_paths.is_empty() {
                status.issues.push(issue(
                    IssueCode::AddonsNotMounted,
                    "citadel/addons is not mounted; mods will not load",
                ));
            } else {
                if !analysis.addons_above_citadel {
                    status.issues.push(issue(
                        IssueCode::AddonsBelowBase,
                        "The addons mount is below the base game mount; mods will be overridden by game files",
                    ));
                }
                if !analysis.missing_roles.is_empty() {
                    status.issues.push(issue(
                        IssueCode::MissingModWrite,
                        format!(
                            "Missing {}; {}",
                            analysis.missing_roles.join(", "),
                            KEYBINDING_WARNING
                        ),
                    ));
                }
            }
        }
        if status.read_only {
            status
                .issues
                .push(issue(IssueCode::ReadOnly, "gameinfo.gi is read-only"));
        }
        if let Some(name) = &analysis.other_manager {
            status.issues.push(issue(
                IssueCode::OtherManager,
                format!(
                    "Another mod manager ({name}) also edits gameinfo.gi; using two managers on one install causes conflicts"
                ),
            ));
        }
        if analysis.obsolete_game_language {
            status.issues.push(issue(
                IssueCode::ObsoleteGameLanguage,
                "gameinfo.gi uses the obsolete Game_Language key",
            ));
        }
        if analysis.duplicate_addon_mounts > 0 {
            status.issues.push(issue(
                IssueCode::DuplicateAddonMounts,
                format!(
                    "{} duplicate addon mount(s) outside the managed block will be removed on reapply",
                    analysis.duplicate_addon_mounts
                ),
            ));
        }
        status.analysis = Some(analysis);
        status
    }

    /// Writes the managed block for `shard_count` shards (clamped to 1..=10).
    pub fn ensure(&self, shard_count: u8) -> Result<WriteReport, GameInfoError> {
        self.apply(|d| ensure_text(&d.text, shard_count, d.crlf))
    }

    /// Removes the managed block.
    pub fn unpatch(&self) -> Result<WriteReport, GameInfoError> {
        self.apply(|d| Ok(unpatch_text(&d.text)))
    }

    /// Removes all Kombini markers and rewrites a fresh managed block.
    pub fn repair_markers(&self, shard_count: u8) -> Result<WriteReport, GameInfoError> {
        self.apply(|d| repair_markers_text(&d.text, shard_count, d.crlf))
    }

    /// Removes another manager's addon mounts (explicit user action only).
    pub fn remove_other_manager_mounts(&self) -> Result<WriteReport, GameInfoError> {
        self.apply(|d| Ok(remove_other_manager_mounts_text(&d.text)))
    }

    fn apply(
        &self,
        edit: impl FnOnce(&DecodedText) -> Result<String, EditError>,
    ) -> Result<WriteReport, GameInfoError> {
        let bytes = match fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(GameInfoError::NotFound(self.path.clone()));
            }
            Err(e) => return Err(GameInfoError::Io(e)),
        };
        let decoded = decode(&bytes);
        let new_text = edit(&decoded)?;
        if new_text == decoded.text {
            return Ok(WriteReport {
                changed: false,
                backup_created: false,
                cleared_read_only: false,
            });
        }

        let backup_created = self.create_backup(&bytes)?;
        let cleared_read_only = self.clear_read_only()?;
        self.write_atomic(&encode(&decoded, &new_text))?;
        tracing::info!(
            path = %self.path.display(),
            backup_created,
            cleared_read_only,
            "wrote gameinfo.gi"
        );
        Ok(WriteReport {
            changed: true,
            backup_created,
            cleared_read_only,
        })
    }

    /// Copies the current bytes to `<path>.bak` unless a backup already exists.
    fn create_backup(&self, bytes: &[u8]) -> Result<bool, GameInfoError> {
        let backup = self.backup_path();
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup)
        {
            Ok(mut file) => {
                if let Err(e) = file.write_all(bytes).and_then(|()| file.sync_all()) {
                    drop(file);
                    let _ = fs::remove_file(&backup);
                    return Err(GameInfoError::Io(e));
                }
                Ok(true)
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(GameInfoError::Io(e)),
        }
    }

    #[allow(clippy::permissions_set_readonly_false)]
    fn clear_read_only(&self) -> Result<bool, GameInfoError> {
        let metadata = fs::metadata(&self.path)?;
        let mut permissions = metadata.permissions();
        if !permissions.readonly() {
            return Ok(false);
        }
        permissions.set_readonly(false);
        fs::set_permissions(&self.path, permissions)
            .map_err(|e| GameInfoError::ReadOnly(e.to_string()))?;
        Ok(true)
    }

    fn write_atomic(&self, data: &[u8]) -> Result<(), GameInfoError> {
        let (temp_path, mut temp_file) = self.create_temp_file()?;
        let written = temp_file
            .write_all(data)
            .and_then(|()| temp_file.sync_all());
        drop(temp_file);
        if let Err(e) = written {
            let _ = fs::remove_file(&temp_path);
            return Err(GameInfoError::Io(e));
        }
        if let Err(e) = rename_with_retry(&temp_path, &self.path) {
            let _ = fs::remove_file(&temp_path);
            return Err(GameInfoError::Io(e));
        }
        Ok(())
    }

    /// Creates `<name>.<random 8 hex>.tmp` next to the target.
    fn create_temp_file(&self) -> Result<(PathBuf, fs::File), GameInfoError> {
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "gameinfo.gi".to_string());
        let dir = self.path.parent().unwrap_or_else(|| Path::new("."));
        let mut last_error = None;
        for _ in 0..TEMP_NAME_ATTEMPTS {
            let suffix = random_hex8();
            let candidate = dir.join(format!("{name}.{suffix}.tmp"));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
            {
                Ok(file) => return Ok((candidate, file)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last_error = Some(e),
                Err(e) => return Err(GameInfoError::Io(e)),
            }
        }
        Err(GameInfoError::Io(last_error.unwrap_or_else(|| {
            io::Error::other("could not create a temporary file")
        })))
    }
}

fn random_hex8() -> String {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u32(std::process::id());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    hasher.write_u32(nanos);
    format!("{:08x}", hasher.finish() & 0xFFFF_FFFF)
}

fn is_transient_lock(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(32 | 33)) || error.kind() == io::ErrorKind::WouldBlock
}

fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    let mut attempt = 0;
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if attempt < RENAME_RETRIES && is_transient_lock(&e) => {
                attempt += 1;
                sleep(RENAME_RETRY_DELAY);
            }
            Err(e) => return Err(e),
        }
    }
}
