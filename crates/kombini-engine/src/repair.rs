//! Mod issue scan and repairs.
//!
//! The checks and fixes come from `kombini-repair`, a port of vpkmerge-core,
//! the engine behind the Grimoire mod manager's mod repairs (MIT; see the
//! crate docs). Each check compares a mod's copy of a game file with the
//! installed game's own copy (`game/citadel/pak01_dir.vpk`, read only):
//!
//! - **Camera**: model swaps built before Valve re-framed the `root_motion`
//!   attachments break the hero's camera. Repairable.
//! - **Outdated data**: a mod shipping an older `heroes`/`abilities` vdata file
//!   deletes what Valve added since. Repairable by filling the absent fields.
//! - **Localization / Skeleton / Sounds**: missing strings, a skeleton sampling
//!   layout from an older game build, absent model sound bindings.
//!
//! A repair never edits a value the mod author set. It writes a new archive,
//! verifies every entry against the original or the intended change, stores
//! it as a new blob (the original blob stays for snapshots) and swaps the
//! mod's file in one transaction, after an automatic snapshot.

use crate::install::TempDir;
use crate::snapshots::SnapshotKind;
use crate::{Engine, Progress, Result};
use kombini_core::{AppError, BlobHash, ErrorCode, ModId};
use kombini_repair::{
    AttachmentHealer, AttachmentStatus, CompatibilityRepairer, RepairStatus, VdataChecker,
    VdataStatus,
};
use kombini_vpk::VpkArchive;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// What kind of problem a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum IssueKind {
    Camera,
    OutdatedData,
    Localization,
    Skeleton,
    Sounds,
}

impl IssueKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Camera => "Camera",
            Self::OutdatedData => "Outdated game data",
            Self::Localization => "Text",
            Self::Skeleton => "Skeleton",
            Self::Sounds => "Sounds",
        }
    }
}

/// One finding in one file of a mod.
#[derive(Debug, Clone, Serialize)]
pub struct ModIssue {
    /// The mod file (root `rel_path`) the finding is in.
    pub root: String,
    /// The VPK entry, e.g. `scripts/heroes.vdata_c`.
    pub entry: String,
    pub kind: IssueKind,
    pub repairable: bool,
    pub detail: String,
}

/// Scan result for one mod.
#[derive(Debug, Clone, Serialize)]
pub struct ModIssues {
    pub mod_id: ModId,
    pub title: String,
    pub enabled: bool,
    pub issues: Vec<ModIssue>,
    /// The mod could not be checked (unreadable archive and the like).
    pub error: Option<String>,
}

impl ModIssues {
    pub fn repairable(&self) -> usize {
        self.issues.iter().filter(|i| i.repairable).count()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RepairOutcome {
    pub mod_id: ModId,
    pub title: String,
    /// Entries rewritten across all of the mod's files.
    pub entries: usize,
    /// Mod files replaced by a repaired copy.
    pub files: usize,
    /// Findings that remain after the repair (not automatically repairable).
    pub remaining: Vec<ModIssue>,
}

const ANIMATION_EXTS: [&str; 6] = [
    ".vnmskel_c",
    ".vnmclip_c",
    ".vnmgraph_c",
    ".vanmgrph_c",
    ".vanim_c",
    ".vagrp_c",
];

fn is_animation(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    ANIMATION_EXTS.iter().any(|e| p.ends_with(e))
}

fn repair_err(context: &str, e: &anyhow::Error) -> AppError {
    AppError::new(ErrorCode::InvalidVpk, format!("{context}: {e:#}"))
}

/// A selected root of a mod, linked with its chunks into a scratch folder so
/// split VPKs resolve.
struct Staged {
    rel_path: String,
    path: PathBuf,
}

/// Size and modification time of the game pak, to notice a game update that
/// lands while a repair runs.
fn stamp(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()?))
}

impl Engine {
    /// The game's own content archive the checks compare against.
    pub fn game_pak(&self) -> PathBuf {
        self.paths().citadel.join("pak01_dir.vpk")
    }

    fn require_game_pak(&self) -> Result<PathBuf> {
        let base = self.game_pak();
        if base.is_file() {
            Ok(base)
        } else {
            Err(AppError::new(
                ErrorCode::GameNotFound,
                format!("Deadlock's game files were not found at {}", base.display()),
            ))
        }
    }

    /// Links the selected roots of `id` (with chunks) into `tmp`.
    fn stage_roots(&self, id: &ModId, tmp: &Path) -> Result<Vec<Staged>> {
        self.with_state(|s| -> Result<Vec<Staged>> {
            let m = s.mods.get(id).ok_or_else(|| AppError::mod_not_found(id))?;
            let mut out = Vec::new();
            for (r, root) in m.selected_roots().enumerate() {
                let base = format!("r{r:02}");
                let dir = tmp.join(format!("{base}_dir.vpk"));
                crate::merge::link_or_copy(&self.blobs().path_for(&root.blob), &dir)?;
                for chunk in m.chunks_of(&root.rel_path) {
                    let idx = chunk.chunk_index.unwrap_or(0);
                    crate::merge::link_or_copy(
                        &self.blobs().path_for(&chunk.blob),
                        &tmp.join(format!("{base}_{idx:03}.vpk")),
                    )?;
                }
                out.push(Staged {
                    rel_path: root.rel_path.clone(),
                    path: dir,
                });
            }
            Ok(out)
        })
    }

    /// Whether an enabled mod other than `except` ships animation files. The
    /// skeleton repair is skipped when another mod drives the animations.
    fn animation_owners(&self) -> BTreeSet<ModId> {
        let roots: Vec<(ModId, PathBuf)> = self.with_state(|s| {
            let profile = s.profile();
            s.mods
                .values()
                .filter(|m| profile.is_enabled(&m.id))
                .flat_map(|m| {
                    m.selected_roots()
                        .map(|r| (m.id.clone(), self.blobs().path_for(&r.blob)))
                        .collect::<Vec<_>>()
                })
                .collect()
        });
        let mut owners = BTreeSet::new();
        for (id, path) in roots {
            if owners.contains(&id) {
                continue;
            }
            // The tree is enough to list paths; chunks are not needed.
            match VpkArchive::open(&path) {
                Ok(a) => {
                    if a.file_paths().iter().any(|p| is_animation(p)) {
                        owners.insert(id);
                    }
                }
                Err(_) => {
                    owners.insert(id);
                }
            }
        }
        owners
    }

    /// Checks every installed mod (enabled first) against the game files.
    /// Read only: nothing in the game or the library changes.
    pub fn scan_mod_issues(&self, progress: &dyn Progress) -> Result<Vec<ModIssues>> {
        let base = self.require_game_pak()?;
        progress.stage("Reading the game's files");
        let animation_owners = self.animation_owners();
        let mods: Vec<(ModId, String, bool)> = self.with_state(|s| {
            let profile = s.profile();
            let mut v: Vec<(ModId, String, bool)> = s
                .mods
                .values()
                .map(|m| (m.id.clone(), m.title.clone(), profile.is_enabled(&m.id)))
                .collect();
            v.sort_by(|a, b| {
                b.2.cmp(&a.2)
                    .then(a.1.to_lowercase().cmp(&b.1.to_lowercase()))
            });
            v
        });
        let mut vdata = VdataChecker::new(&base);
        let mut camera = AttachmentHealer::new(&base);
        let mut compat = CompatibilityRepairer::new(&base)
            .map_err(|e| repair_err("Opening the game files", &e))?;
        let total = mods.len() as u64;
        let mut out = Vec::with_capacity(mods.len());
        for (i, (id, title, enabled)) in mods.into_iter().enumerate() {
            if progress.cancelled() {
                return Err(AppError::cancelled());
            }
            progress.stage(&format!("Checking {title}"));
            progress.progress(i as u64, total);
            let overrides = animation_owners.iter().any(|o| o != &id);
            let result = (|| -> Result<Vec<ModIssue>> {
                let tmp = TempDir::new(self)?;
                let mut issues = Vec::new();
                for staged in self.stage_roots(&id, &tmp.0)? {
                    issues.extend(check_root(
                        &staged,
                        &mut vdata,
                        &mut camera,
                        &mut compat,
                        overrides,
                    ));
                }
                Ok(issues)
            })();
            out.push(match result {
                Ok(issues) => ModIssues {
                    mod_id: id,
                    title,
                    enabled,
                    issues,
                    error: None,
                },
                Err(e) => ModIssues {
                    mod_id: id,
                    title,
                    enabled,
                    issues: Vec::new(),
                    error: Some(e.message.clone()),
                },
            });
        }
        progress.progress(total, total);
        Ok(out)
    }

    /// Repairs every repairable finding in `id`'s selected files.
    pub fn repair_mod(&self, id: &ModId, progress: &dyn Progress) -> Result<RepairOutcome> {
        let base = self.require_game_pak()?;
        let before = stamp(&base);
        let title = self.with_state(|s| s.mods.get(id).map(|m| m.title.clone()));
        let title = title.ok_or_else(|| AppError::mod_not_found(id))?;
        progress.stage(&format!("Repairing {title}"));
        let overrides = self.animation_owners().iter().any(|o| o != id);
        let tmp = TempDir::new(self)?;
        let staged = self.stage_roots(id, &tmp.0)?;
        let mut camera = AttachmentHealer::new(&base);
        let mut compat = CompatibilityRepairer::new(&base)
            .map_err(|e| repair_err("Opening the game files", &e))?;
        // rel_path -> repaired file
        let mut replaced: Vec<(String, PathBuf)> = Vec::new();
        let mut entries = 0usize;
        for (n, root) in staged.iter().enumerate() {
            if progress.cancelled() {
                return Err(AppError::cancelled());
            }
            let mut current = root.path.clone();
            let mut changed: BTreeSet<String> = BTreeSet::new();
            let healed_path = tmp.0.join(format!("heal{n:02}_dir.vpk"));
            let healed = camera
                .heal(&current, &healed_path)
                .map_err(|e| repair_err("Repairing the camera data", &e))?;
            if !healed.is_empty() && healed_path.is_file() {
                changed.extend(healed.iter().map(|h| h.entry.clone()));
                current = healed_path;
            }
            let compat_path = tmp.0.join(format!("compat{n:02}_dir.vpk"));
            let reports = compat
                .heal(&current, &compat_path, overrides)
                .map_err(|e| repair_err("Repairing game data", &e))?;
            let repaired: Vec<_> = reports
                .iter()
                .filter(|r| matches!(r.status, RepairStatus::Repairable))
                .collect();
            if !repaired.is_empty() && compat_path.is_file() {
                changed.extend(repaired.iter().map(|r| r.entry.clone()));
                current = compat_path;
            }
            if current != root.path {
                verify_only_changed(&root.path, &current, &changed)?;
                entries += changed.len();
                replaced.push((root.rel_path.clone(), current));
            }
        }
        if stamp(&base) != before {
            return Err(AppError::invalid(
                "Deadlock was updated during the repair. Try again.",
            ));
        }
        if replaced.is_empty() {
            return Ok(RepairOutcome {
                mod_id: id.clone(),
                title,
                entries: 0,
                files: 0,
                remaining: self.remaining_issues(id)?,
            });
        }
        progress.stage("Saving the repaired files");
        let mut blobs: HashMap<String, (BlobHash, u64)> = HashMap::new();
        for (rel, path) in &replaced {
            let (hash, size) = self
                .blobs()
                .insert_file(path, &|| progress.cancelled(), &|_| {})
                .map_err(AppError::from)?;
            blobs.insert(rel.clone(), (hash, size));
        }
        if self.settings().auto_snapshot
            && let Err(e) =
                self.create_snapshot(&format!("Before repairing {title}"), SnapshotKind::Auto)
        {
            tracing::warn!(error = %e, "auto snapshot before repair failed");
        }
        let files = blobs.len();
        let mod_id = id.clone();
        self.mutate("repair mod", progress, move |state| {
            let m = state
                .mods
                .get_mut(&mod_id)
                .ok_or_else(|| AppError::mod_not_found(&mod_id))?;
            for (rel, (hash, size)) in &blobs {
                // The repaired archive holds all data in one file: drop chunks.
                m.files
                    .retain(|f| !(f.is_chunk && f.parent.as_deref() == Some(rel.as_str())));
                if let Some(root) = m
                    .files
                    .iter_mut()
                    .find(|f| !f.is_chunk && &f.rel_path == rel)
                {
                    root.blob = hash.clone();
                    root.size = *size;
                }
            }
            Ok(format!("Repaired {}", m.title))
        })?;
        Ok(RepairOutcome {
            mod_id: id.clone(),
            title,
            entries,
            files,
            remaining: self.remaining_issues(id)?,
        })
    }

    /// Findings left in one mod (used after a repair).
    fn remaining_issues(&self, id: &ModId) -> Result<Vec<ModIssue>> {
        let base = self.require_game_pak()?;
        let overrides = self.animation_owners().iter().any(|o| o != id);
        let tmp = TempDir::new(self)?;
        let mut vdata = VdataChecker::new(&base);
        let mut camera = AttachmentHealer::new(&base);
        let mut compat = CompatibilityRepairer::new(&base)
            .map_err(|e| repair_err("Opening the game files", &e))?;
        let mut out = Vec::new();
        for staged in self.stage_roots(id, &tmp.0)? {
            out.extend(check_root(
                &staged,
                &mut vdata,
                &mut camera,
                &mut compat,
                overrides,
            ));
        }
        Ok(out)
    }
}

/// Runs the three checks on one staged root.
fn check_root(
    staged: &Staged,
    vdata: &mut VdataChecker,
    camera: &mut AttachmentHealer,
    compat: &mut CompatibilityRepairer,
    animation_overrides: bool,
) -> Vec<ModIssue> {
    let mut issues = Vec::new();
    let root = staged.rel_path.clone();
    let issue = |entry: &str, kind, repairable, detail: String| ModIssue {
        root: root.clone(),
        entry: entry.to_owned(),
        kind,
        repairable,
        detail,
    };
    match camera.check(&staged.path) {
        Ok(reports) => {
            for r in reports
                .into_iter()
                .filter(|r| r.status == AttachmentStatus::Stale)
            {
                issues.push(issue(
                    &r.entry,
                    IssueKind::Camera,
                    true,
                    format!(
                        "Camera and aim points built for an older game version ({}).",
                        sample(&r.stale, 4)
                    ),
                ));
            }
        }
        Err(e) => tracing::warn!(root = %staged.rel_path, error = %e, "camera check failed"),
    }
    // Compatibility findings by entry; "data" ones explain outdated vdata.
    let mut data_fix: BTreeMap<String, (bool, String)> = BTreeMap::new();
    match compat.check(&staged.path, animation_overrides) {
        Ok(reports) => {
            for r in reports {
                let repairable = matches!(r.status, RepairStatus::Repairable);
                let kind = match r.kind.as_str() {
                    "data" => {
                        data_fix.insert(r.entry.clone(), (repairable, r.detail.clone()));
                        continue;
                    }
                    "localization" => IssueKind::Localization,
                    "skeleton" => IssueKind::Skeleton,
                    "sounds" => IssueKind::Sounds,
                    _ => continue,
                };
                let detail = if repairable {
                    match kind {
                        IssueKind::Localization => {
                            format!("{} text string(s) the game added are missing.", r.changes)
                        }
                        IssueKind::Skeleton => {
                            "Uses an older skeleton layout than the game.".to_owned()
                        }
                        IssueKind::Sounds => "Model is missing its sound bindings.".to_owned(),
                        _ => String::new(),
                    }
                } else {
                    format!("Can't be fixed automatically: {}", r.detail)
                };
                issues.push(issue(&r.entry, kind, repairable, detail));
            }
        }
        Err(e) => tracing::warn!(root = %staged.rel_path, error = %e, "compatibility check failed"),
    }
    match vdata.check(&staged.path) {
        Ok(reports) => {
            for r in reports
                .into_iter()
                .filter(|r| r.status == VdataStatus::Outdated)
            {
                let (repairable, why) = data_fix.remove(&r.entry).unwrap_or((false, String::new()));
                let mut detail = format!(
                    "Older copy of a game file: {} entr{} the game added since would be removed ({}).",
                    r.missing.len(),
                    if r.missing.len() == 1 { "y" } else { "ies" },
                    sample(&r.missing, 3)
                );
                if !repairable && !why.is_empty() {
                    detail.push_str(&format!(" Can't be fixed automatically: {why}"));
                }
                issues.push(issue(&r.entry, IssueKind::OutdatedData, repairable, detail));
            }
        }
        Err(e) => tracing::warn!(root = %staged.rel_path, error = %e, "vdata check failed"),
    }
    // Data repairs the vdata check did not flag (fields only, no removals).
    for (entry, (repairable, why)) in data_fix {
        let detail = if repairable {
            "Missing fields the game added since this mod was made.".to_owned()
        } else {
            format!("Can't be fixed automatically: {why}")
        };
        issues.push(issue(&entry, IssueKind::OutdatedData, repairable, detail));
    }
    issues
}

fn sample(items: &[String], n: usize) -> String {
    let mut s = items.iter().take(n).cloned().collect::<Vec<_>>().join(", ");
    if items.len() > n {
        s.push_str(&format!(", +{} more", items.len() - n));
    }
    s
}

/// The repaired archive must hold exactly the original entries, and every
/// entry outside `changed` must be byte-identical.
fn verify_only_changed(original: &Path, repaired: &Path, changed: &BTreeSet<String>) -> Result<()> {
    let bad = |what: String| AppError::internal(format!("{what}; the mod was left untouched."));
    let open = |p: &Path| {
        VpkArchive::open(p).map_err(|e| bad(format!("Cannot verify the repaired archive: {e}")))
    };
    let (a, b) = (open(original)?, open(repaired)?);
    let index = |v: &VpkArchive| -> BTreeMap<String, String> {
        v.file_paths()
            .into_iter()
            .map(|p| (p.to_ascii_lowercase(), p.to_owned()))
            .collect()
    };
    let (before, after) = (index(&a), index(&b));
    if before.keys().ne(after.keys()) {
        return Err(bad("The repaired archive changed its list of files".into()));
    }
    let changed: BTreeSet<String> = changed.iter().map(|c| c.to_ascii_lowercase()).collect();
    for (key, path) in &before {
        if changed.contains(key) {
            continue;
        }
        let read = |v: &VpkArchive, p: &str| {
            v.read_file(p)
                .map_err(|e| bad(format!("Cannot verify {p}: {e}")))
        };
        if read(&a, path)? != read(&b, &after[key])? {
            return Err(bad(format!(
                "The repair changed an unrelated file ({path})"
            )));
        }
    }
    Ok(())
}
