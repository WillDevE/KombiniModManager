//! Application services for the Kombini Deadlock mod manager.
//!
//! The [`Engine`] owns one game install: its SQLite state, the blob store and
//! the addon folders. Every change to managed files goes through
//! [`Engine::mutate`], which diffs the old and new library state, applies the
//! file operations as one journaled transaction ([`txn`]) and commits the new
//! state atomically with the transaction id.
//!
//! The API is synchronous; UI layers call it from a background executor.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod bisect;
pub mod fsops;
pub mod health;
pub mod install;
pub mod launch;
pub mod library;
pub mod merge;
pub mod paths;
pub mod reconcile;
pub mod repair;
pub mod roster;
pub mod scan;
pub mod settings;
pub mod snapshots;
mod sound_pack;
pub mod timeline;
pub mod txn;
pub mod watch;

use kombini_core::{
    AppError, BlobHash, ErrorCode, ForeignFile, GameId, ModId, Placement, assign_layout,
    effective_order,
};
use kombini_gameinfo::{GameInfoError, GameInfoFile, WriteReport};
use kombini_storage::{BlobStore, LibraryState, Storage};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use paths::{AppDirs, GamePaths};
pub use settings::Settings;

pub type Result<T> = std::result::Result<T, AppError>;

/// Progress sink for long operations. Implementations must be cheap; the
/// engine may call `progress` often (the UI layer throttles).
pub trait Progress: Send + Sync {
    fn stage(&self, _label: &str) {}
    fn progress(&self, _done: u64, _total: u64) {}
    fn cancelled(&self) -> bool {
        false
    }
}

/// A [`Progress`] that ignores everything.
pub struct NoProgress;
impl Progress for NoProgress {}

/// Answers "is Deadlock running?". Swappable for tests.
pub trait ProcessCheck: Send + Sync {
    fn is_running(&self) -> bool;
}

/// The real process check (sysinfo-backed probe from `kombini-steam`).
pub struct SystemProcess(Mutex<kombini_steam::ProcessProbe>);

impl SystemProcess {
    pub fn new() -> Self {
        Self(Mutex::new(kombini_steam::ProcessProbe::new()))
    }
}

impl Default for SystemProcess {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessCheck for SystemProcess {
    fn is_running(&self) -> bool {
        self.0.lock().is_running()
    }
}

pub struct EngineConfig {
    pub dirs: AppDirs,
    /// Deadlock install root (the folder containing `game/`).
    pub game_root: PathBuf,
    pub process: Arc<dyn ProcessCheck>,
}

/// Things worth telling the user after [`Engine::open`].
#[derive(Debug, Clone, Default, Serialize)]
pub struct OpenReport {
    pub recovered: Vec<String>,
    pub corrupt_db_backup: Option<PathBuf>,
    pub game_updated: bool,
    pub gameinfo_reapplied: bool,
    pub missing_files: usize,
    pub foreign_files: usize,
}

/// Summary of one applied change, for toasts and the activity log.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ChangeReport {
    pub summary: String,
    pub moved: usize,
    pub placed: usize,
    pub removed: usize,
    pub mounted_roots: usize,
    pub max_shard: u8,
    pub gameinfo_changed: bool,
    pub warnings: Vec<String>,
}

const LAST_UPDATE_CHECK_KEY: &str = "last_update_check";

pub struct Engine {
    pub(crate) paths: GamePaths,
    pub(crate) dirs: AppDirs,
    pub(crate) game_id: GameId,
    pub(crate) db: Mutex<Storage>,
    pub(crate) blobs: BlobStore,
    pub(crate) state: RwLock<LibraryState>,
    pub(crate) foreign: RwLock<Vec<ForeignFile>>,
    pub(crate) other_managers: RwLock<Vec<String>>,
    pub(crate) settings: RwLock<Settings>,
    /// Serializes every mutation of the game folder and library state.
    pub(crate) writer: Mutex<()>,
    pub(crate) process: Arc<dyn ProcessCheck>,
    /// VPK directory listings by blob, for conflicts and hero detection.
    pub(crate) listing_cache: Mutex<HashMap<BlobHash, Arc<Vec<String>>>>,
    /// Unix ms until which watcher events are attributed to our own writes.
    pub(crate) quiet_until: Mutex<i64>,
}

/// Stable id for a game install: sha256 of the canonical, lowercased root path.
pub fn game_id_for(root: &Path) -> GameId {
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let text = canonical
        .to_string_lossy()
        .to_lowercase()
        .replace('\\', "/");
    let digest = Sha256::digest(text.as_bytes());
    GameId::new(hex::encode(digest)[..12].to_owned())
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

pub(crate) fn gameinfo_error(err: GameInfoError) -> AppError {
    use kombini_gameinfo::EditError;
    match err {
        GameInfoError::NotFound(path) => AppError::new(
            ErrorCode::GameNotFound,
            "gameinfo.gi was not found. Verify the game files in Steam.",
        )
        .with_detail(path.display().to_string()),
        GameInfoError::Edit(EditError::MissingSearchPaths) => AppError::new(
            ErrorCode::GameinfoMissingSearchPaths,
            "gameinfo.gi has no SearchPaths block. Verify the game files in Steam, then try again.",
        ),
        GameInfoError::Edit(EditError::MarkersCorrupt) => AppError::new(
            ErrorCode::GameinfoMarkersCorrupt,
            "The managed block in gameinfo.gi is damaged. Use Repair markers.",
        ),
        GameInfoError::Edit(e @ EditError::VerificationFailed(_)) => {
            AppError::new(ErrorCode::GameinfoWriteFailed, e.to_string())
        }
        GameInfoError::ReadOnly(detail) => AppError::new(
            ErrorCode::GameinfoReadOnly,
            "gameinfo.gi is read-only and could not be made writable.",
        )
        .with_detail(detail),
        GameInfoError::Io(e) => AppError::new(
            ErrorCode::GameinfoWriteFailed,
            "Could not write gameinfo.gi. Close Deadlock and check antivirus/OneDrive.",
        )
        .with_detail(e.to_string()),
    }
}

impl Engine {
    /// Opens (or creates) the state for `config.game_root`: recovers
    /// interrupted transactions, reconciles with disk and reapplies the
    /// gameinfo block if a game update reset it.
    pub fn open(config: EngineConfig) -> Result<(Self, OpenReport)> {
        let root = kombini_steam::validate_game_root(&config.game_root).map_err(|e| {
            AppError::new(ErrorCode::GameNotFound, e.to_string())
                .with_detail(config.game_root.display().to_string())
        })?;
        let paths = GamePaths::new(&root);
        let game_id = game_id_for(&root);
        std::fs::create_dir_all(&config.dirs.data)
            .map_err(|e| AppError::from_io("Creating the data folder", &config.dirs.data, &e))?;
        let (db, db_report) = Storage::open(&config.dirs.db())?;
        let blobs = BlobStore::new(config.dirs.blobs())?;
        let game_row = db.upsert_game(game_id.as_str(), &root.to_string_lossy())?;
        let settings: Settings = db.get_setting(settings::SETTINGS_KEY)?.unwrap_or_default();
        let state = db.load_library(game_id.as_str())?;

        let mut report = OpenReport {
            corrupt_db_backup: db_report.corrupt_backup,
            ..OpenReport::default()
        };
        report.recovered = txn::recover(&paths, game_row.committed_txn.as_deref());

        let engine = Self {
            paths,
            dirs: config.dirs,
            game_id,
            db: Mutex::new(db),
            blobs,
            state: RwLock::new(state),
            foreign: RwLock::new(Vec::new()),
            other_managers: RwLock::new(Vec::new()),
            settings: RwLock::new(settings.normalized()),
            writer: Mutex::new(()),
            process: config.process,
            listing_cache: Mutex::new(HashMap::new()),
            quiet_until: Mutex::new(0),
        };

        let rescan = engine.rescan()?;
        report.missing_files = rescan.missing_files;
        report.foreign_files = rescan.foreign_files;

        // Game updates overwrite gameinfo.gi; detect and repair.
        let build = kombini_steam::read_build_info(&engine.paths.root);
        if game_row.last_build.as_deref() != Some(build.fingerprint.as_str()) {
            report.game_updated = game_row.last_build.is_some();
            engine
                .db
                .lock()
                .set_last_build(engine.game_id.as_str(), &build.fingerprint)?;
        }
        let pending_vanilla = engine.take_vanilla_pending();
        if !engine.mods_active() {
            if engine.gameinfo_status().patched && !build.update_in_progress {
                match engine.unpatch_gameinfo() {
                    Ok(r) => report.gameinfo_reapplied = r.changed,
                    Err(e) => tracing::warn!(error = %e, "could not keep gameinfo vanilla"),
                }
            }
        } else if engine.settings().auto_reapply_gameinfo || pending_vanilla {
            let has_mounts = engine
                .state
                .read()
                .mods
                .values()
                .any(|m| m.files.iter().any(|f| f.placement.is_mounted()))
                || engine.foreign.read().iter().any(|f| f.slot.is_some());
            if has_mounts && !build.update_in_progress {
                match engine.ensure_gameinfo() {
                    Ok(r) => report.gameinfo_reapplied = r.changed,
                    Err(e) => tracing::warn!(error = %e, "could not reapply gameinfo on open"),
                }
            }
        }
        Ok((engine, report))
    }

    pub fn game_id(&self) -> &GameId {
        &self.game_id
    }

    pub fn paths(&self) -> &GamePaths {
        &self.paths
    }

    pub fn dirs(&self) -> &AppDirs {
        &self.dirs
    }

    pub fn blobs(&self) -> &BlobStore {
        &self.blobs
    }

    /// A clone of the current library state.
    pub fn state(&self) -> LibraryState {
        self.state.read().clone()
    }

    /// Runs `f` with a read lock on the state (no clone).
    pub fn with_state<T>(&self, f: impl FnOnce(&LibraryState) -> T) -> T {
        f(&self.state.read())
    }

    pub fn foreign_files(&self) -> Vec<ForeignFile> {
        self.foreign.read().clone()
    }

    pub fn other_managers(&self) -> Vec<String> {
        self.other_managers.read().clone()
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().clone()
    }

    pub fn set_settings(&self, settings: Settings) -> Result<Settings> {
        let settings = settings.normalized();
        self.db
            .lock()
            .set_setting(settings::SETTINGS_KEY, &settings)?;
        *self.settings.write() = settings.clone();
        Ok(settings)
    }

    pub fn game_running(&self) -> bool {
        self.process.is_running()
    }

    /// True while the engine itself is writing (watcher events are ours).
    pub fn is_quiet(&self) -> bool {
        now_ms() < *self.quiet_until.lock()
    }

    fn mark_quiet(&self) {
        *self.quiet_until.lock() = now_ms() + 2_000;
    }

    pub(crate) fn guard_not_running(&self) -> Result<()> {
        if self.settings.read().block_while_running && self.process.is_running() {
            return Err(AppError::game_running());
        }
        Ok(())
    }

    pub(crate) fn log(&self, action: &str, summary: &str, details: Option<serde_json::Value>) {
        if let Err(e) = self.db.lock().log_action(
            self.game_id.as_str(),
            action,
            summary,
            details.as_ref(),
            None,
        ) {
            tracing::warn!(error = %e, "could not write the activity log");
        }
    }

    /// Slots reserved by foreign files: their own `pakNN_dir.vpk` and any
    /// `pakNN_XXX.vpk` chunk names (so our chunks never collide with them).
    pub(crate) fn foreign_slots(&self, excluding: &[ForeignFile]) -> BTreeSet<(u8, u8)> {
        let mut slots = BTreeSet::new();
        for f in self.foreign.read().iter() {
            if excluding
                .iter()
                .any(|x| x.shard == f.shard && x.file_name.eq_ignore_ascii_case(&f.file_name))
            {
                continue;
            }
            if let Some(slot) = f.slot {
                slots.insert((f.shard, slot));
            } else if let Some((slot, _)) = kombini_core::naming::parse_enabled_chunk(&f.file_name)
            {
                slots.insert((f.shard, slot));
            }
        }
        slots
    }

    pub(crate) fn placement_path(&self, placement: &Placement) -> Option<PathBuf> {
        placement
            .location()
            .map(|(shard, name)| self.paths.shard(shard).join(name))
    }

    /// The single entry point for state changes that may move files.
    ///
    /// `f` edits a copy of the state (mods, profiles, selection, order);
    /// placements in the copy are ignored and recomputed. On success the new
    /// state is live, persisted and on disk; on failure nothing changed.
    pub fn mutate(
        &self,
        action: &str,
        progress: &dyn Progress,
        f: impl FnOnce(&mut LibraryState) -> Result<String>,
    ) -> Result<ChangeReport> {
        self.mutate_with(action, progress, &[], f)
    }

    /// Like [`Engine::mutate`], and also removes the given foreign files in
    /// the same transaction (adoption: the managed copy takes their place).
    pub fn adopt_foreign(
        &self,
        action: &str,
        progress: &dyn Progress,
        originals: &[ForeignFile],
        f: impl FnOnce(&mut LibraryState) -> Result<String>,
    ) -> Result<ChangeReport> {
        {
            let foreign = self.foreign.read();
            for o in originals {
                if !foreign
                    .iter()
                    .any(|x| x.shard == o.shard && x.file_name.eq_ignore_ascii_case(&o.file_name))
                {
                    return Err(AppError::invalid(format!(
                        "{} is no longer an unmanaged file; rescan and try again",
                        o.file_name
                    )));
                }
            }
        }
        let report = self.mutate_with(action, progress, originals, f)?;
        let _writer = self.writer.lock();
        self.rescan_locked()?;
        Ok(report)
    }

    fn mutate_with(
        &self,
        action: &str,
        progress: &dyn Progress,
        removals: &[ForeignFile],
        f: impl FnOnce(&mut LibraryState) -> Result<String>,
    ) -> Result<ChangeReport> {
        let _writer = self.writer.lock();
        self.guard_not_running()?;
        let old = self.state.read().clone();
        let mut new = old.clone();
        let summary = f(&mut new)?;
        let report = self.apply_state(&old, new, progress, removals)?;
        let report = ChangeReport { summary, ..report };
        self.log(action, &report.summary, serde_json::to_value(&report).ok());
        Ok(report)
    }

    /// Diffs `old` → `new`, executes the file transaction and commits.
    /// Caller holds `writer`.
    pub(crate) fn apply_state(
        &self,
        old: &LibraryState,
        mut new: LibraryState,
        progress: &dyn Progress,
        foreign_removals: &[ForeignFile],
    ) -> Result<ChangeReport> {
        // Current placement of each (mod, rel_path) that still has the same content.
        let mut current: BTreeMap<(ModId, String), (BlobHash, Placement)> = BTreeMap::new();
        for m in old.mods.values() {
            for f in &m.files {
                current.insert(
                    (m.id.clone(), f.rel_path.clone()),
                    (f.blob.clone(), f.placement.clone()),
                );
            }
        }
        let existing: BTreeSet<ModId> = new.mods.keys().cloned().collect();
        let order = effective_order(new.profile(), &existing);
        let mods: Vec<&kombini_core::Mod> = new.mods.values().collect();
        let layout = assign_layout(&mods, &order, &self.foreign_slots(foreign_removals))?;

        let mut ops = Vec::new();
        let mut warnings = Vec::new();
        let mut targets: BTreeMap<(ModId, String), Placement> = BTreeMap::new();
        let mut kept: BTreeSet<(ModId, String)> = BTreeSet::new();
        for m in new.mods.values() {
            for f in &m.files {
                let key = (m.id.clone(), f.rel_path.clone());
                let Some(target) = layout.placements.get(&key).cloned() else {
                    continue;
                };
                let Some(to) = self.placement_path(&target) else {
                    continue;
                };
                let now = current
                    .get(&key)
                    .filter(|(blob, _)| *blob == f.blob)
                    .map(|(_, p)| p.clone());
                if now.is_some() {
                    kept.insert(key.clone());
                }
                match now.as_ref().and_then(|p| self.placement_path(p)) {
                    Some(_) if now.as_ref() == Some(&target) => {}
                    Some(from) => ops.push(txn::Op::Move { from, to }),
                    None => {
                        if self.blobs.contains(&f.blob) {
                            ops.push(txn::Op::Place {
                                blob: f.blob.clone(),
                                to,
                            });
                        } else {
                            warnings.push(format!(
                                "{}: {} is missing and its stored copy is gone; reinstall the mod",
                                m.title, f.rel_path
                            ));
                            targets.insert(key, Placement::Missing);
                            continue;
                        }
                    }
                }
                targets.insert(key, target);
            }
        }
        // Files that are no longer wanted (uninstalled, or replaced content).
        for (key, (_, placement)) in &current {
            if kept.contains(key) {
                continue;
            }
            if let Some(from) = self.placement_path(placement)
                && from.exists()
            {
                ops.push(txn::Op::Remove { from });
            }
        }

        for f in foreign_removals {
            let from = self.paths.shard(f.shard).join(&f.file_name);
            if from.exists() {
                ops.push(txn::Op::Remove { from });
            }
        }

        for m in new.mods.values_mut() {
            for f in &mut m.files {
                f.placement = targets
                    .get(&(m.id.clone(), f.rel_path.clone()))
                    .cloned()
                    .unwrap_or(Placement::Missing);
            }
        }

        progress.stage("Applying changes");
        self.mark_quiet();
        let db = &self.db;
        let game_id = self.game_id.as_str();
        let new_ref = &new;
        let txn_report = txn::execute(
            &self.paths,
            &self.blobs,
            ops,
            &|| self.guard_not_running(),
            &mut |id| {
                db.lock()
                    .save_library(game_id, new_ref, Some(id))
                    .map_err(AppError::from)
            },
            &|done, total| progress.progress(done as u64, total as u64),
        )?;
        self.mark_quiet();
        *self.state.write() = new;

        let mut report = ChangeReport {
            moved: txn_report.moved,
            placed: txn_report.placed,
            removed: txn_report.removed,
            mounted_roots: layout.mounted_roots,
            max_shard: layout.max_shard,
            warnings,
            ..ChangeReport::default()
        };
        report.warnings.extend(
            txn_report
                .recovered
                .iter()
                .map(|p| format!("A duplicate file was kept at {p}")),
        );
        if layout.mounted_roots > 0 || self.foreign.read().iter().any(|f| f.slot.is_some()) {
            match self.ensure_gameinfo() {
                Ok(w) => report.gameinfo_changed = w.changed,
                Err(e) => report.warnings.push(format!(
                    "Mods were placed, but gameinfo.gi could not be updated: {e}"
                )),
            }
        }
        Ok(report)
    }

    /// Number of addon shards gameinfo must mount.
    pub(crate) fn needed_shards(&self) -> u8 {
        let ours = self
            .state
            .read()
            .mods
            .values()
            .flat_map(|m| m.files.iter())
            .filter_map(|f| match &f.placement {
                Placement::Mounted { shard, .. } => Some(*shard),
                _ => None,
            })
            .max()
            .unwrap_or(1);
        let foreign = self
            .foreign
            .read()
            .iter()
            .filter(|f| f.slot.is_some())
            .map(|f| f.shard)
            .max()
            .unwrap_or(1);
        ours.max(foreign)
    }

    /// Ensures the managed gameinfo block mounts every shard in use. While
    /// mods are switched off it keeps gameinfo vanilla instead.
    pub fn ensure_gameinfo(&self) -> Result<WriteReport> {
        if !self.mods_active() {
            return self.unpatch_gameinfo();
        }
        self.mark_quiet();
        GameInfoFile::new(self.paths.gameinfo())
            .ensure(self.needed_shards())
            .map_err(gameinfo_error)
    }

    pub fn gameinfo_status(&self) -> kombini_gameinfo::GameInfoStatus {
        GameInfoFile::new(self.paths.gameinfo()).status()
    }

    pub fn unpatch_gameinfo(&self) -> Result<WriteReport> {
        self.mark_quiet();
        GameInfoFile::new(self.paths.gameinfo())
            .unpatch()
            .map_err(gameinfo_error)
    }

    pub fn repair_gameinfo_markers(&self) -> Result<WriteReport> {
        self.mark_quiet();
        GameInfoFile::new(self.paths.gameinfo())
            .repair_markers(self.needed_shards())
            .map_err(gameinfo_error)
    }

    pub fn remove_other_manager_mounts(&self) -> Result<WriteReport> {
        self.mark_quiet();
        GameInfoFile::new(self.paths.gameinfo())
            .remove_other_manager_mounts()
            .map_err(gameinfo_error)
    }

    /// Re-reads the addon folders and updates placements (disk is truth).
    pub fn rescan(&self) -> Result<reconcile::RescanReport> {
        let _writer = self.writer.lock();
        self.rescan_locked()
    }

    pub(crate) fn rescan_locked(&self) -> Result<reconcile::RescanReport> {
        let disk = scan::scan(&self.paths);
        let mut state = self.state.read().clone();
        let outcome = reconcile::reconcile(&mut state, &disk);
        if outcome.changed {
            self.db
                .lock()
                .save_library(self.game_id.as_str(), &state, None)?;
        }
        *self.state.write() = state;
        *self.foreign.write() = outcome.foreign.clone();
        *self.other_managers.write() = disk.other_managers.clone();
        Ok(outcome.report())
    }

    /// When mod updates were last checked automatically (unix seconds).
    pub fn last_update_check(&self) -> Option<i64> {
        self.db
            .lock()
            .get_setting::<i64>(LAST_UPDATE_CHECK_KEY)
            .ok()
            .flatten()
    }

    pub fn set_last_update_check(&self, at: i64) -> Result<()> {
        self.db.lock().set_setting(LAST_UPDATE_CHECK_KEY, &at)?;
        Ok(())
    }

    pub(crate) fn take_vanilla_pending(&self) -> bool {
        let db = self.db.lock();
        let pending = db
            .get_setting::<bool>(launch::VANILLA_PENDING_KEY)
            .ok()
            .flatten()
            .unwrap_or(false);
        if pending {
            let _ = db.set_setting(launch::VANILLA_PENDING_KEY, &false);
        }
        pending
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod testutil;
