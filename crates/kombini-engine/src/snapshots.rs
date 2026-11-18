//! Content-addressed snapshots (§6.3).
//!
//! A snapshot is a JSON manifest of the library (mods, profiles, autoexec,
//! gameinfo text for reference, foreign files). File content lives in the
//! blob store, so snapshots are instant and deduplicated.

use crate::{ChangeReport, Engine, Progress, Result};
use kombini_core::{AppError, BlobHash, ErrorCode, ForeignFile, Mod, Profile, ProfileId};
use kombini_storage::{LibraryState, SnapshotRow};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotKind {
    Auto,
    Manual,
}

impl SnapshotKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub app_version: String,
    pub game_build: Option<String>,
    pub label: String,
    pub created_at: i64,
    pub active_profile: ProfileId,
    pub mods: Vec<Mod>,
    pub profiles: Vec<Profile>,
    pub autoexec: Option<String>,
    /// For reference only; restore never writes it back.
    pub gameinfo_text: Option<String>,
    pub foreign: Vec<ForeignFile>,
}

impl Manifest {
    pub fn blobs(&self) -> BTreeMap<BlobHash, u64> {
        self.mods
            .iter()
            .flat_map(|m| m.files.iter())
            .map(|f| (f.blob.clone(), f.size))
            .collect()
    }
}

/// List row for the UI (manifest omitted).
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotInfo {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub created_at: i64,
    pub size: i64,
    pub mods: usize,
    pub enabled: usize,
    pub game_build: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RestoreReport {
    pub change: ChangeReport,
    pub safety_snapshot: Option<String>,
    pub missing_blobs: usize,
}

const FORMAT: u32 = 1;

fn not_found(id: &str) -> AppError {
    AppError::new(
        ErrorCode::SnapshotNotFound,
        format!("Snapshot {id} was not found"),
    )
}

fn parse_manifest(text: &str) -> Result<Manifest> {
    serde_json::from_str(text)
        .map_err(|e| AppError::storage(format!("Snapshot manifest is unreadable: {e}")))
}

fn info_from(row: &SnapshotRow, manifest: Option<&Manifest>) -> SnapshotInfo {
    let (mods, enabled, game_build) = manifest
        .map(|m| {
            let enabled = m
                .profiles
                .iter()
                .find(|p| p.id == m.active_profile)
                .map(|p| p.enabled.len())
                .unwrap_or(0);
            (m.mods.len(), enabled, m.game_build.clone())
        })
        .unwrap_or((0, 0, None));
    SnapshotInfo {
        id: row.id.clone(),
        label: row.label.clone(),
        kind: row.kind.clone(),
        created_at: row.created_at,
        size: row.size,
        mods,
        enabled,
        game_build,
    }
}

impl Engine {
    fn manifest_now(&self, label: &str) -> Manifest {
        let state = self.state();
        Manifest {
            format: FORMAT,
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            game_build: Some(kombini_steam::read_build_info(&self.paths.root).fingerprint),
            label: label.to_owned(),
            created_at: kombini_storage::now(),
            active_profile: state.active_profile.clone(),
            mods: state.mods.values().cloned().collect(),
            profiles: state.profiles.clone(),
            autoexec: self.read_autoexec(),
            gameinfo_text: std::fs::read(self.paths.gameinfo())
                .ok()
                .map(|b| String::from_utf8_lossy(&b).into_owned()),
            foreign: self.foreign_files(),
        }
    }

    pub fn create_snapshot(&self, label: &str, kind: SnapshotKind) -> Result<SnapshotInfo> {
        let label = if label.trim().is_empty() {
            "Snapshot".to_owned()
        } else {
            label.trim().to_owned()
        };
        let manifest = self.manifest_now(&label);
        let size: u64 = manifest.blobs().values().sum();
        let row = SnapshotRow {
            id: format!("s{}{}", manifest.created_at, kombini_storage::random_hex(3)),
            game_id: self.game_id.as_str().to_owned(),
            label,
            kind: kind.as_str().to_owned(),
            created_at: manifest.created_at,
            manifest: serde_json::to_string(&manifest)
                .map_err(|e| AppError::internal(e.to_string()))?,
            size: i64::try_from(size).unwrap_or(i64::MAX),
        };
        self.db.lock().insert_snapshot(&row)?;
        if kind == SnapshotKind::Auto {
            self.prune_snapshots()?;
        }
        Ok(info_from(&row, Some(&manifest)))
    }

    pub fn list_snapshots(&self) -> Result<Vec<SnapshotInfo>> {
        let rows = self.db.lock().list_snapshots(self.game_id.as_str(), true)?;
        Ok(rows
            .iter()
            .map(|r| info_from(r, parse_manifest(&r.manifest).ok().as_ref()))
            .collect())
    }

    pub fn snapshot_manifest(&self, id: &str) -> Result<Manifest> {
        let row = self.db.lock().snapshot(id)?.ok_or_else(|| not_found(id))?;
        parse_manifest(&row.manifest)
    }

    pub fn delete_snapshot(&self, id: &str) -> Result<()> {
        if !self.db.lock().delete_snapshot(id)? {
            return Err(not_found(id));
        }
        Ok(())
    }

    /// Keeps the newest `snapshot_retention` auto snapshots.
    pub fn prune_snapshots(&self) -> Result<usize> {
        let keep = self.settings().snapshot_retention as usize;
        let db = self.db.lock();
        let mut autos: Vec<SnapshotRow> = db
            .list_snapshots(self.game_id.as_str(), false)?
            .into_iter()
            .filter(|r| r.kind == "auto")
            .collect();
        autos.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
        let mut removed = 0;
        for row in autos.iter().skip(keep) {
            if db.delete_snapshot(&row.id)? {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Restores mods, profiles, placements and autoexec. Takes a safety
    /// snapshot first; reapplies (never pastes) the gameinfo block.
    pub fn restore_snapshot(&self, id: &str, progress: &dyn Progress) -> Result<RestoreReport> {
        let manifest = self.snapshot_manifest(id)?;
        progress.stage("Checking stored files");
        let missing_blobs = manifest
            .blobs()
            .keys()
            .filter(|b| !self.blobs.contains(b))
            .count();
        self.guard_not_running()?;
        let safety = self
            .create_snapshot(
                &format!("Before restoring \u{201c}{}\u{201d}", manifest.label),
                SnapshotKind::Auto,
            )
            .map(|s| s.id)
            .ok();
        let label = manifest.label.clone();
        let autoexec = manifest.autoexec.clone();
        let change = self.mutate("snapshot_restore", progress, move |state| {
            let mut profiles = manifest.profiles;
            if profiles.is_empty() {
                profiles.push(Profile::default_profile());
            }
            let active = if profiles.iter().any(|p| p.id == manifest.active_profile) {
                manifest.active_profile
            } else {
                profiles[0].id.clone()
            };
            *state = LibraryState {
                mods: manifest
                    .mods
                    .into_iter()
                    .map(|m| (m.id.clone(), m))
                    .collect(),
                profiles,
                active_profile: active,
            };
            Ok(format!("Restored snapshot \u{201c}{label}\u{201d}"))
        })?;
        if let Some(text) = autoexec
            && let Err(e) = self.write_autoexec(&text)
        {
            tracing::warn!(error = %e, "could not restore autoexec.cfg");
        }
        Ok(RestoreReport {
            change,
            safety_snapshot: safety,
            missing_blobs,
        })
    }

    /// Blobs referenced by the live library or any snapshot of any game.
    fn referenced_blobs(&self) -> Result<BTreeSet<BlobHash>> {
        let mut set: BTreeSet<BlobHash> = BTreeSet::new();
        let db = self.db.lock();
        for doc in db.all_mod_docs()? {
            if let Ok(m) = serde_json::from_str::<Mod>(&doc) {
                set.extend(m.files.into_iter().map(|f| f.blob));
                for v in m.history {
                    set.extend(v.files.into_iter().map(|f| f.blob));
                }
            }
        }
        for text in db.all_snapshot_manifests()? {
            match serde_json::from_str::<Manifest>(&text) {
                Ok(m) => set.extend(m.blobs().into_keys()),
                Err(e) => {
                    // Never collect when we cannot tell what is referenced.
                    return Err(AppError::storage(format!(
                        "A snapshot manifest is unreadable, so cleanup was skipped: {e}"
                    )));
                }
            }
        }
        Ok(set)
    }

    /// Deletes unreferenced blobs.
    pub fn gc_blobs(&self) -> Result<kombini_storage::GcReport> {
        let _writer = self.writer.lock();
        let referenced = self.referenced_blobs()?;
        let report = self.blobs.gc(&referenced)?;
        Ok(report)
    }

    /// Writes a portable `.kmmsnap` (zip: `manifest.json` + `blobs/<hash>`).
    pub fn export_snapshot(&self, id: &str, dest: &Path, progress: &dyn Progress) -> Result<()> {
        let row = self.db.lock().snapshot(id)?.ok_or_else(|| not_found(id))?;
        let manifest = parse_manifest(&row.manifest)?;
        let blobs = manifest.blobs();
        let total: u64 = blobs.values().sum();
        let tmp = dest.with_extension("kmmsnap.part");
        let result = (|| -> Result<()> {
            let file = std::fs::File::create(&tmp)
                .map_err(|e| AppError::from_io("Creating the export", &tmp, &e))?;
            let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
            let deflated = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            let stored = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored)
                .large_file(true);
            let zerr = |e: zip::result::ZipError| AppError::internal(format!("zip: {e}"));
            zip.start_file("manifest.json", deflated).map_err(zerr)?;
            zip.write_all(row.manifest.as_bytes())
                .map_err(|e| AppError::from_io("Writing the export", &tmp, &e))?;
            let mut done = 0u64;
            progress.stage("Exporting");
            for (hash, size) in &blobs {
                if progress.cancelled() {
                    return Err(AppError::cancelled());
                }
                let source = self.blobs.path_for(hash);
                let mut input = std::fs::File::open(&source)
                    .map_err(|e| AppError::from_io("Reading stored file", &source, &e))?;
                zip.start_file(format!("blobs/{hash}"), stored)
                    .map_err(zerr)?;
                std::io::copy(&mut input, &mut zip)
                    .map_err(|e| AppError::from_io("Writing the export", &tmp, &e))?;
                done += size;
                progress.progress(done, total);
            }
            zip.finish().map_err(zerr)?;
            Ok(())
        })();
        if let Err(e) = result {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        if dest.exists() {
            crate::fsops::remove_file(dest)?;
        }
        std::fs::rename(&tmp, dest).map_err(|e| AppError::from_io("Saving the export", dest, &e))
    }

    /// Imports a `.kmmsnap`: verifies and stores every blob, then adds the
    /// snapshot (manual). Restoring it is a separate step.
    pub fn import_snapshot(&self, source: &Path, progress: &dyn Progress) -> Result<SnapshotInfo> {
        let file = std::fs::File::open(source)
            .map_err(|e| AppError::from_io("Opening the snapshot", source, &e))?;
        let bad = |m: String| AppError::new(ErrorCode::InvalidInput, m);
        let mut zip =
            zip::ZipArchive::new(file).map_err(|e| bad(format!("Not a snapshot file: {e}")))?;
        let manifest_text = {
            let mut entry = zip
                .by_name("manifest.json")
                .map_err(|_| bad("The snapshot has no manifest".into()))?;
            if entry.size() > 64 * 1024 * 1024 {
                return Err(bad("The snapshot manifest is too large".into()));
            }
            let mut s = String::new();
            entry
                .read_to_string(&mut s)
                .map_err(|e| bad(format!("Unreadable manifest: {e}")))?;
            s
        };
        let manifest = parse_manifest(&manifest_text)?;
        let wanted = manifest.blobs();
        let total: u64 = wanted.values().sum();
        let mut done = 0u64;
        progress.stage("Importing");
        let tmp_dir = self.dirs.extract_tmp().join(kombini_storage::random_hex(8));
        std::fs::create_dir_all(&tmp_dir)
            .map_err(|e| AppError::from_io("Creating a temporary folder", &tmp_dir, &e))?;
        let result = (|| -> Result<()> {
            for (hash, size) in &wanted {
                if self.blobs.contains(hash) {
                    done += size;
                    continue;
                }
                if progress.cancelled() {
                    return Err(AppError::cancelled());
                }
                let mut entry = zip
                    .by_name(&format!("blobs/{hash}"))
                    .map_err(|_| bad(format!("The snapshot is missing file {hash}")))?;
                let tmp = tmp_dir.join(hash.as_str());
                let mut out = std::fs::File::create(&tmp)
                    .map_err(|e| AppError::from_io("Importing", &tmp, &e))?;
                std::io::copy(&mut (&mut entry).take(size.saturating_add(1)), &mut out)
                    .map_err(|e| AppError::from_io("Importing", &tmp, &e))?;
                drop(out);
                let (got, _) = self.blobs.insert_file(&tmp, &|| false, &|_| {})?;
                let _ = std::fs::remove_file(&tmp);
                if &got != hash {
                    return Err(AppError::new(
                        ErrorCode::HashMismatch,
                        "A file in the snapshot is corrupt",
                    )
                    .with_detail(hash.to_string()));
                }
                done += size;
                progress.progress(done, total);
            }
            Ok(())
        })();
        let _ = std::fs::remove_dir_all(&tmp_dir);
        result?;
        let row = SnapshotRow {
            id: format!(
                "s{}{}",
                kombini_storage::now(),
                kombini_storage::random_hex(3)
            ),
            game_id: self.game_id.as_str().to_owned(),
            label: format!("Imported: {}", manifest.label),
            kind: "manual".into(),
            created_at: kombini_storage::now(),
            manifest: manifest_text,
            size: i64::try_from(total).unwrap_or(i64::MAX),
        };
        self.db.lock().insert_snapshot(&row)?;
        Ok(info_from(&row, Some(&manifest)))
    }
}
