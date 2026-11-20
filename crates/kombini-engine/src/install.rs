//! Installing mods from archives, folders and loose VPK files (§6.4).

use crate::{ChangeReport, Engine, Progress, Result};
use kombini_archive::{ArchiveError, ExtractOptions, Limits};
use kombini_core::heroes::HeroTable;
use kombini_core::naming::default_root_selection;
use kombini_core::{
    AppError, ErrorCode, Mod, ModFile, ModId, ModSource, Placement, Requirement, SafetyFinding,
};
use kombini_vpk::VpkArchive;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Metadata supplied by the caller (GameBanana, import, or the file picker).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallMeta {
    /// Explicit id (imports, merges, updates). Otherwise see
    /// [`resolve_install_id`].
    pub id: Option<ModId>,
    pub title: Option<String>,
    pub source: Option<ModSource>,
    pub remote_id: Option<u64>,
    pub author: Option<String>,
    pub version: Option<String>,
    pub category: Option<String>,
    pub page_url: Option<String>,
    pub description: Option<String>,
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub gallery: Vec<String>,
    pub file_id: Option<u64>,
    pub file_name: Option<String>,
    pub md5: Option<String>,
    pub variant_desc: Option<String>,
    pub remote_updated_at: Option<i64>,
    #[serde(default)]
    pub requirements: Vec<Requirement>,
    pub archive_path: Option<String>,
    pub source_key: Option<String>,
    /// Enable right after install (default: keep previous state / disabled).
    pub enable: Option<bool>,
    /// rel_paths of roots to select; None = previous selection or default.
    pub selection: Option<Vec<String>>,
    /// With `enable: Some(true)`: turn off the other files of the same
    /// GameBanana page (switching variants instead of stacking them).
    #[serde(default)]
    pub disable_siblings: bool,
    /// File ids of one batch install from a page, highest load priority
    /// first, so files that finish downloading in any order still end up in
    /// this order. Empty: a new file goes above the page's other files.
    #[serde(default)]
    pub order_hint: Vec<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallReport {
    pub mod_id: ModId,
    pub title: String,
    pub replaced: bool,
    pub roots: usize,
    pub files: usize,
    pub bytes: u64,
    pub heroes: Vec<String>,
    pub safety: Vec<SafetyFinding>,
    pub warnings: Vec<String>,
    /// Titles of the page's other files that were turned off.
    pub disabled_siblings: Vec<String>,
    pub change: ChangeReport,
}

/// Validated content of an extracted folder, before blobs are stored.
struct Staged {
    files: Vec<StagedFile>,
    entry_paths: Vec<String>,
    warnings: Vec<String>,
}

struct StagedFile {
    path: PathBuf,
    rel_path: String,
    original_name: String,
    is_chunk: bool,
    parent: Option<String>,
    chunk_index: Option<u16>,
}

pub(crate) fn archive_error(err: ArchiveError) -> AppError {
    let message = err.to_string();
    let code = match &err {
        ArchiveError::Unsupported(_) => ErrorCode::UnsupportedArchive,
        ArchiveError::UnsafePath(_) | ArchiveError::Link(_) => ErrorCode::ArchiveUnsafePath,
        ArchiveError::TooLarge { .. } | ArchiveError::TooManyEntries { .. } => {
            ErrorCode::ArchiveTooLarge
        }
        ArchiveError::Encrypted => ErrorCode::ArchiveEncrypted,
        ArchiveError::Cancelled => ErrorCode::Cancelled,
        ArchiveError::Io { path, source } => {
            return AppError::from_io("Extracting archive", path, source);
        }
        ArchiveError::MissingVolume(_)
        | ArchiveError::Failed { .. }
        | ArchiveError::DestinationNotEmpty(_) => ErrorCode::ExtractFailed,
    };
    AppError::new(code, message)
}

fn rel_of(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Groups and validates every VPK under `dir`.
fn stage_dir(dir: &Path) -> Result<Staged> {
    let discovered = kombini_archive::discover_vpks(dir);
    let paths: Vec<PathBuf> = discovered
        .iter()
        .map(|d| {
            let mut p = dir.to_path_buf();
            for part in d.rel_path.split('/') {
                p.push(part);
            }
            p
        })
        .collect();
    let (bundles, orphans) = kombini_vpk::group_bundles(&paths);
    if bundles.is_empty() {
        return Err(AppError::new(
            ErrorCode::NoVpkInArchive,
            "No Deadlock mod files (.vpk) were found. The download may be a source/tool package \
             or use a format this manager cannot install.",
        ));
    }
    let mut warnings: Vec<String> = orphans
        .iter()
        .map(|p| {
            format!(
                "Ignored {} (a VPK chunk without its _dir.vpk)",
                rel_of(dir, p)
            )
        })
        .collect();
    let mut files = Vec::new();
    let mut entry_paths = Vec::new();
    for bundle in bundles {
        let root_rel = rel_of(dir, &bundle.root);
        let archive = VpkArchive::open(&bundle.root).map_err(|e| {
            AppError::new(
                ErrorCode::InvalidVpk,
                format!("{root_rel} is not a valid VPK file"),
            )
            .with_detail(e.to_string())
        })?;
        let present: BTreeSet<u16> = bundle.chunks.iter().map(|(i, _)| *i).collect();
        let missing: Vec<u16> = archive
            .chunk_indices()
            .iter()
            .copied()
            .filter(|i| !present.contains(i))
            .collect();
        if !missing.is_empty() {
            return Err(AppError::new(
                ErrorCode::InvalidVpk,
                format!(
                    "{root_rel} needs {} data chunk file(s) that are not in the download",
                    missing.len()
                ),
            )
            .with_detail(format!("missing chunk indices: {missing:?}")));
        }
        entry_paths.extend(archive.file_paths().into_iter().map(str::to_owned));
        for (index, chunk) in &bundle.chunks {
            if !archive.chunk_indices().contains(index) {
                warnings.push(format!("Ignored unused chunk {}", rel_of(dir, chunk)));
                continue;
            }
            files.push(StagedFile {
                path: chunk.clone(),
                rel_path: rel_of(dir, chunk),
                original_name: name_of(chunk),
                is_chunk: true,
                parent: Some(root_rel.clone()),
                chunk_index: Some(*index),
            });
        }
        files.push(StagedFile {
            original_name: name_of(&bundle.root),
            path: bundle.root,
            rel_path: root_rel,
            is_chunk: false,
            parent: None,
            chunk_index: None,
        });
    }
    if discovered
        .iter()
        .any(|d| !d.valid_signature && !d.rel_path.ends_with("_000.vpk"))
    {
        tracing::debug!("some .vpk files lacked a signature (chunks are expected to)");
    }
    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(Staged {
        files,
        entry_paths,
        warnings,
    })
}

/// A scratch directory under the cache that is removed on drop.
pub(crate) struct TempDir(pub(crate) PathBuf);

impl TempDir {
    pub(crate) fn new(engine: &Engine) -> Result<Self> {
        let path = engine
            .dirs
            .extract_tmp()
            .join(kombini_storage::random_hex(8));
        std::fs::create_dir_all(&path)
            .map_err(|e| AppError::from_io("Creating a temporary folder", &path, &e))?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Engine {
    /// Removes leftovers from crashed installs.
    pub fn clean_extract_tmp(&self) {
        let _ = std::fs::remove_dir_all(self.dirs.extract_tmp());
    }

    /// Installs from an archive (zip/7z/rar) or a single `.vpk`.
    pub fn install_archive(
        &self,
        archive: &Path,
        meta: InstallMeta,
        progress: &dyn Progress,
    ) -> Result<InstallReport> {
        let size = std::fs::metadata(archive)
            .map_err(|e| AppError::from_io("Reading the download", archive, &e))?
            .len();
        let tmp = TempDir::new(self)?;
        progress.stage("Extracting");
        let opts = ExtractOptions {
            limits: Limits::for_archive_size(size),
            cancel: &|| progress.cancelled(),
            progress: &|done, total| progress.progress(done, total.unwrap_or(0)),
        };
        let extracted = kombini_archive::extract(archive, &tmp.0, &opts).map_err(archive_error)?;
        let mut meta = meta;
        if meta.title.is_none() {
            meta.title = archive.file_stem().map(|s| s.to_string_lossy().to_string());
        }
        let mut report = self.install_dir(&tmp.0, meta, progress)?;
        report.warnings.extend(extracted.warnings);
        Ok(report)
    }

    /// Installs loose files or one folder (drag and drop).
    pub fn install_paths(
        &self,
        paths: &[PathBuf],
        meta: InstallMeta,
        progress: &dyn Progress,
    ) -> Result<InstallReport> {
        match paths {
            [] => Err(AppError::invalid("Nothing to install")),
            [single] if single.is_dir() => {
                let mut meta = meta;
                if meta.title.is_none() {
                    meta.title = Some(name_of(single));
                }
                self.install_dir(single, meta, progress)
            }
            [single] => self.install_archive(single, meta, progress),
            many => {
                let tmp = TempDir::new(self)?;
                for path in many {
                    if path.is_dir() {
                        return Err(AppError::invalid(
                            "Drop either one folder or a set of files, not both",
                        ));
                    }
                    let name = name_of(path);
                    if !name.to_ascii_lowercase().ends_with(".vpk") {
                        return Err(AppError::invalid(format!(
                            "{name} is not a .vpk file; install archives one at a time"
                        )));
                    }
                    crate::fsops::copy(path, &tmp.0.join(&name))?;
                }
                let mut meta = meta;
                if meta.title.is_none() {
                    meta.title = many
                        .iter()
                        .map(|p| name_of(p))
                        .find(|n| n.to_ascii_lowercase().ends_with("_dir.vpk"))
                        .or_else(|| many.first().map(|p| name_of(p)));
                }
                self.install_dir(&tmp.0, meta, progress)
            }
        }
    }

    /// Installs a voice pack of WAV files: each one named after a game clip
    /// is converted and packed into a VPK, which is installed as usual.
    fn install_sound_pack(
        &self,
        dir: &Path,
        meta: InstallMeta,
        progress: &dyn Progress,
    ) -> Result<InstallReport> {
        progress.stage("Converting sounds");
        let tmp = TempDir::new(self)?;
        let pack = crate::sound_pack::build_sound_pack(
            dir,
            &self.game_pak(),
            &tmp.0.join("pak01_dir.vpk"),
            &|| progress.cancelled(),
        )?;
        let mut report = self.install_dir(&tmp.0, meta, progress)?;
        report.warnings.push(format!(
            "Converted {} WAV file{} into game sounds.",
            pack.converted,
            if pack.converted == 1 { "" } else { "s" }
        ));
        if !pack.skipped.is_empty() {
            let shown: Vec<&str> = pack.skipped.iter().take(5).map(String::as_str).collect();
            let more = pack.skipped.len().saturating_sub(shown.len());
            report.warnings.push(format!(
                "Skipped {} WAV file{} not named after a game sound: {}{}",
                pack.skipped.len(),
                if pack.skipped.len() == 1 { "" } else { "s" },
                shown.join(", "),
                if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                }
            ));
        }
        Ok(report)
    }

    /// Installs every VPK bundle under `dir` as one mod.
    pub fn install_dir(
        &self,
        dir: &Path,
        meta: InstallMeta,
        progress: &dyn Progress,
    ) -> Result<InstallReport> {
        if crate::sound_pack::is_sound_pack(dir) {
            return self.install_sound_pack(dir, meta, progress);
        }
        progress.stage("Checking files");
        let staged = stage_dir(dir)?;
        let total: u64 = staged
            .files
            .iter()
            .filter_map(|f| std::fs::metadata(&f.path).ok())
            .map(|m| m.len())
            .sum();
        progress.stage("Storing");
        let mut files = Vec::with_capacity(staged.files.len());
        let mut done_before = 0u64;
        for f in &staged.files {
            if progress.cancelled() {
                return Err(AppError::cancelled());
            }
            let base = done_before;
            let (blob, size) = self
                .blobs
                .insert_file(&f.path, &|| progress.cancelled(), &|n| {
                    progress.progress(base + n, total)
                })?;
            done_before += size;
            let _ = self.db.lock().record_blob(blob.as_str(), size);
            files.push(ModFile {
                rel_path: f.rel_path.clone(),
                original_name: f.original_name.clone(),
                blob,
                size,
                is_chunk: f.is_chunk,
                parent: f.parent.clone(),
                chunk_index: f.chunk_index,
                selected: true,
                placement: Placement::Missing,
            });
        }

        let first_root = files
            .iter()
            .find(|f| !f.is_chunk)
            .map(|f| f.blob.as_str().to_owned())
            .unwrap_or_default();
        let local_id = ModId::new(format!("local_{}", &first_root[..first_root.len().min(12)]));
        // Resolved inside the transaction so two files of one page finishing
        // together cannot claim the same id.
        let mut id = local_id.clone();

        let heroes = HeroTable::builtin().detect(staged.entry_paths.iter().map(String::as_str));
        let safety =
            kombini_core::safety::scan_paths(staged.entry_paths.iter().map(String::as_str));
        let now = kombini_storage::now();
        let roots: Vec<(String, String)> = files
            .iter()
            .filter(|f| !f.is_chunk)
            .map(|f| (f.rel_path.clone(), f.original_name.clone()))
            .collect();
        let defaults = default_root_selection(roots.iter().map(|(r, n)| (r.as_str(), n.as_str())));
        let bytes: u64 = files.iter().map(|f| f.size).sum();
        let warnings = staged.warnings;
        let mut disabled_siblings = Vec::new();

        let mut replaced = false;
        let mut final_title = String::new();
        let mut installed_roots = 0;
        let change = self.mutate("install", progress, |state| {
            id = resolve_install_id(&state.mods, &meta, &local_id);
            let title_fallback = meta.title.clone().unwrap_or_else(|| id.to_string());
            let previous = state.mods.get(&id).cloned();
            replaced = previous.is_some();
            let mut m = previous.clone().unwrap_or_else(|| {
                Mod::new(
                    id.clone(),
                    title_fallback.clone(),
                    meta.source.unwrap_or(if meta.remote_id.is_some() {
                        ModSource::Gamebanana
                    } else {
                        ModSource::Local
                    }),
                    now,
                )
            });
            let mut new_files = files.clone();
            for f in new_files.iter_mut().filter(|f| !f.is_chunk) {
                f.selected = match &meta.selection {
                    Some(sel) => sel.contains(&f.rel_path),
                    None => previous
                        .as_ref()
                        .and_then(|p| {
                            p.file(&f.rel_path).map(|o| o.selected).or_else(|| {
                                p.roots()
                                    .find(|o| o.original_name.eq_ignore_ascii_case(&f.original_name))
                                    .map(|o| o.selected)
                            })
                        })
                        .unwrap_or_else(|| defaults.get(&f.rel_path).copied().unwrap_or(true)),
                };
            }
            if !new_files.iter().any(|f| !f.is_chunk && f.selected)
                && let Some(first) = new_files.iter_mut().find(|f| !f.is_chunk) {
                    first.selected = true;
                }
            // Chunks mirror their root's selection.
            let selected: BTreeSet<String> = new_files
                .iter()
                .filter(|f| !f.is_chunk && f.selected)
                .map(|f| f.rel_path.clone())
                .collect();
            for f in new_files.iter_mut().filter(|f| f.is_chunk) {
                f.selected = f.parent.as_ref().is_some_and(|p| selected.contains(p));
            }
            installed_roots = new_files.iter().filter(|f| !f.is_chunk).count();
            // An update with different content keeps the old files for rollback.
            let changed = previous.as_ref().is_some_and(|p| {
                let old: BTreeSet<_> = p.files.iter().map(|f| &f.blob).collect();
                let new: BTreeSet<_> = new_files.iter().map(|f| &f.blob).collect();
                old != new
            });
            if changed {
                m.push_history(now);
            }
            m.files = new_files;
            m.updated_at = now;
            if let Some(t) = &meta.title {
                m.title = t.clone();
            }
            macro_rules! take {
                ($($field:ident),*) => { $( if meta.$field.is_some() { m.$field = meta.$field.clone(); } )* };
            }
            take!(
                remote_id, author, version, category, page_url, description, thumbnail_url,
                file_id, file_name, md5, variant_desc, remote_updated_at, archive_path, source_key
            );
            if !meta.gallery.is_empty() {
                m.gallery = meta.gallery.clone();
            }
            if !meta.requirements.is_empty() {
                m.requirements = meta.requirements.clone();
            }
            m.heroes = heroes.clone();
            m.safety = safety.clone();
            if m.update.file_id.is_some() && m.update.file_id == meta.file_id {
                m.update.available = false;
            }
            final_title = m.title.clone();
            state.mods.insert(id.clone(), m);
            let siblings = siblings_of(&state.mods, &id);
            for profile in &mut state.profiles {
                if replaced || siblings.is_empty() {
                    profile.ensure_ordered(&id);
                } else {
                    place_with_siblings(profile, &id, &siblings, &state.mods, &meta.order_hint);
                }
            }
            if let Some(enable) = meta.enable {
                // Files installed in the same batch stay on together.
                let titles: Vec<(ModId, String)> = siblings
                    .iter()
                    .filter_map(|s| state.mods.get(s))
                    .filter(|m| !m.file_id.is_some_and(|f| meta.order_hint.contains(&f)))
                    .map(|m| (m.id.clone(), m.title.clone()))
                    .collect();
                let profile = state.profile_mut();
                if enable {
                    profile.enabled.insert(id.clone());
                    if meta.disable_siblings {
                        for (s, title) in titles {
                            if profile.enabled.remove(&s) {
                                disabled_siblings.push(title);
                            }
                        }
                    }
                } else {
                    profile.enabled.remove(&id);
                }
            }
            Ok(format!(
                "{} {}",
                if replaced { "Updated" } else { "Installed" },
                title_fallback
            ))
        })?;

        Ok(InstallReport {
            mod_id: id,
            title: final_title,
            replaced,
            roots: installed_roots,
            files: files.len(),
            bytes,
            heroes,
            safety,
            warnings,
            disabled_siblings,
            change,
        })
    }
}

/// Which mod an install goes to when the caller gave no id:
/// - GameBanana: the mod already holding this file (a reinstall), else
///   `gb<page>` for the page's first file, else a new sibling
///   `gb<page>_<file>`, so other files of a page never replace each other.
///   Without a file id (old metadata) the page's mod is replaced as before.
/// - Local: `local_<hash>` of the first root.
pub fn resolve_install_id(
    mods: &std::collections::BTreeMap<ModId, Mod>,
    meta: &InstallMeta,
    local_id: &ModId,
) -> ModId {
    if let Some(id) = &meta.id {
        return id.clone();
    }
    let Some(remote) = meta.remote_id else {
        return local_id.clone();
    };
    let base = ModId::gamebanana(remote);
    let mut same_page = mods.values().filter(|m| m.remote_id == Some(remote));
    let Some(file_id) = meta.file_id else {
        return if mods.contains_key(&base) {
            base
        } else {
            same_page.next().map_or(base, |m| m.id.clone())
        };
    };
    if let Some(m) = mods
        .values()
        .find(|m| m.remote_id == Some(remote) && m.file_id == Some(file_id))
    {
        return m.id.clone();
    }
    if same_page.next().is_none() && !mods.contains_key(&base) {
        return base;
    }
    ModId::gamebanana_sibling(remote, &file_id.to_string(), |c| mods.contains_key(c))
}

/// Other installed files of the same GameBanana page as `id`.
pub fn siblings_of(mods: &std::collections::BTreeMap<ModId, Mod>, id: &ModId) -> Vec<ModId> {
    let Some(remote) = mods.get(id).and_then(|m| m.remote_id) else {
        return Vec::new();
    };
    mods.values()
        .filter(|m| m.remote_id == Some(remote) && &m.id != id)
        .map(|m| m.id.clone())
        .collect()
}

/// Puts a newly installed page file next to its siblings in the load order
/// (index 0 wins). Siblings named in `hint` keep the hint's order; otherwise
/// the new file goes directly above the page's other files.
pub fn place_with_siblings(
    profile: &mut kombini_core::Profile,
    id: &ModId,
    siblings: &[ModId],
    mods: &std::collections::BTreeMap<ModId, Mod>,
    hint: &[u64],
) {
    profile.order.retain(|o| o != id);
    let rank = |m: &ModId| {
        mods.get(m)
            .and_then(|m| m.file_id)
            .and_then(|f| hint.iter().position(|h| *h == f))
    };
    let mine = rank(id);
    let positions: Vec<(usize, Option<usize>)> = profile
        .order
        .iter()
        .enumerate()
        .filter(|(_, o)| siblings.contains(o))
        .map(|(i, o)| (i, rank(o)))
        .collect();
    let Some(&(first, _)) = positions.first() else {
        profile.order.push(id.clone());
        return;
    };
    let at = match mine {
        Some(me) => {
            // Above the first hinted sibling that ranks lower than this one,
            // else below the last one that ranks higher.
            if let Some(&(i, _)) = positions.iter().find(|(_, r)| r.is_some_and(|r| r > me)) {
                i
            } else if let Some(&(i, _)) = positions
                .iter()
                .rev()
                .find(|(_, r)| r.is_some_and(|r| r < me))
            {
                i + 1
            } else {
                first
            }
        }
        None => first,
    };
    profile.order.insert(at, id.clone());
}
