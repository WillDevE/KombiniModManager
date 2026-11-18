//! Deadlock Mod Interchange v1 (`mod-interchange.json` + `files/`).

use kombini_core::{AppError, ErrorCode, ModId, ModSource, Profile, ProfileId, Requirement};
use kombini_engine::install::InstallMeta;
use kombini_engine::{Engine, Progress};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

pub const FORMAT: &str = "deadlock-mod-interchange";
pub const MANIFEST: &str = "mod-interchange.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Origin {
    pub provider: String,
    pub submission_type: Option<String>,
    /// GameBanana writes it as a string; accept both.
    pub submission_id: Option<serde_json::Value>,
    pub file_id: Option<u64>,
    pub file_name: Option<String>,
    pub local_id: Option<String>,
}

impl Origin {
    pub fn submission(&self) -> Option<u64> {
        match self.submission_id.as_ref()? {
            serde_json::Value::Number(n) => n.as_u64(),
            serde_json::Value::String(s) => s.parse().ok(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FileEntry {
    pub name: String,
    /// Relative to the bundle root, `/`-separated.
    pub path: String,
    pub sha256: Option<String>,
    pub size: Option<u64>,
    pub selected: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModEntry {
    pub key: String,
    pub name: String,
    pub enabled: Option<bool>,
    pub order: Option<i64>,
    pub origin: Origin,
    pub author: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub hero: Option<String>,
    pub thumbnail_url: Option<String>,
    pub link: Option<String>,
    pub requirements: serde_json::Value,
    pub files: Vec<FileEntry>,
    pub extensions: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProfileModRef {
    pub mod_key: String,
    pub enabled: bool,
    pub order: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProfileEntry {
    pub key: String,
    pub name: String,
    pub active: bool,
    pub description: Option<String>,
    pub mods: Vec<ProfileModRef>,
    pub autoexec: Option<Vec<String>>,
    pub extensions: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Document {
    pub format: String,
    pub version: u32,
    pub created_at: Option<String>,
    pub source: serde_json::Value,
    pub contents: Vec<String>,
    pub mods: Vec<ModEntry>,
    pub profiles: Vec<ProfileEntry>,
    pub warnings: Vec<String>,
}

fn invalid(msg: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::InvalidInput, msg)
}

/// Opens a bundle given its folder or its manifest file.
pub fn read(path: &Path) -> Result<(PathBuf, Document), AppError> {
    let (base, manifest) = if path.is_dir() {
        (path.to_path_buf(), path.join(MANIFEST))
    } else {
        (
            path.parent().map(Path::to_path_buf).unwrap_or_default(),
            path.to_path_buf(),
        )
    };
    let bytes = std::fs::read(&manifest)
        .map_err(|e| AppError::from_io("Reading the interchange manifest", &manifest, &e))?;
    let doc: Document = serde_json::from_slice(&bytes)
        .map_err(|e| invalid(format!("mod-interchange.json is not valid: {e}")))?;
    if doc.format != FORMAT {
        return Err(invalid("This is not a Deadlock mod interchange file"));
    }
    if doc.version != 1 {
        return Err(invalid(format!(
            "Unsupported interchange version {} (expected 1)",
            doc.version
        )));
    }
    Ok((base, doc))
}

/// Resolves a bundle-relative path, refusing anything that escapes the bundle.
fn bundle_file(base: &Path, rel: &str) -> Result<PathBuf, AppError> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute()
        || rel_path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(invalid(format!("Unsafe file path in the bundle: {rel}")));
    }
    Ok(base.join(rel_path))
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportReport {
    pub imported: Vec<ModId>,
    pub skipped: Vec<String>,
    pub profiles: Vec<ProfileId>,
    pub warnings: Vec<String>,
}

/// Relative install path for a file: strips the exporter's `files/<slug>/`.
fn install_rel(entry: &FileEntry) -> String {
    let parts: Vec<&str> = entry.path.split('/').collect();
    if parts.len() > 2 && parts[0] == "files" {
        parts[2..].join("/")
    } else {
        entry.name.clone()
    }
}

/// Imports mods (optionally only `selected` keys) and then profiles.
pub fn import(
    engine: &Engine,
    path: &Path,
    selected: Option<&BTreeSet<String>>,
    progress: &dyn Progress,
) -> Result<ImportReport, AppError> {
    let (base, doc) = read(path)?;
    let mut report = ImportReport::default();
    let key_to_id = import_mods(engine, Some(&base), &doc, selected, &mut report, progress)?;
    import_profiles(engine, &doc, &key_to_id, &mut report, progress)?;
    Ok(report)
}

/// Where a document's file lives: inside the bundle at `base`, or (for
/// documents read straight from another manager, `base` = None) an
/// absolute path.
fn source_file(base: Option<&Path>, rel: &str) -> Result<PathBuf, AppError> {
    match base {
        Some(base) => bundle_file(base, rel),
        None if Path::new(rel).is_absolute() => Ok(PathBuf::from(rel)),
        None => Err(invalid(format!("Expected an absolute file path: {rel}"))),
    }
}

/// Installs the document's mods (disabled) and returns mod key → library id,
/// including mods that were already installed.
pub fn import_mods(
    engine: &Engine,
    base: Option<&Path>,
    doc: &Document,
    selected: Option<&BTreeSet<String>>,
    report: &mut ImportReport,
    progress: &dyn Progress,
) -> Result<BTreeMap<String, ModId>, AppError> {
    report.warnings.extend(doc.warnings.iter().cloned());
    let (existing_keys, mut remote_files, mut taken_ids) = engine.with_state(|s| {
        let keys: BTreeMap<String, ModId> = s
            .mods
            .values()
            .filter_map(|m| m.source_key.clone().map(|k| (k, m.id.clone())))
            .collect();
        // Installed files per GameBanana submission: one page can ship several
        // files (skin, voice lines, icons), each its own library entry.
        let mut remote: BTreeMap<u64, Vec<FileRef>> = BTreeMap::new();
        for m in s.mods.values() {
            if let Some(r) = m.remote_id {
                remote.entry(r).or_default().push(FileRef {
                    file_id: m.file_id,
                    file_name: m.file_name.clone(),
                    id: m.id.clone(),
                });
            }
        }
        let ids: BTreeSet<ModId> = s.mods.keys().cloned().collect();
        (keys, remote, ids)
    });
    let mut key_to_id: BTreeMap<String, ModId> = BTreeMap::new();
    let total = doc.mods.len();
    for (i, m) in doc.mods.iter().enumerate() {
        if selected.is_some_and(|s| !s.contains(&m.key)) {
            continue;
        }
        if progress.cancelled() {
            return Err(AppError::cancelled());
        }
        progress.stage(&format!("Importing {} ({}/{total})", m.name, i + 1));
        // GameBanana sounds live in another section than mods (our remote id).
        let is_mod = m
            .origin
            .submission_type
            .as_deref()
            .is_none_or(|t| t.eq_ignore_ascii_case("mod"));
        let remote = if m.origin.provider.eq_ignore_ascii_case("gamebanana") && is_mod {
            m.origin.submission()
        } else {
            None
        };
        let same_file = remote.and_then(|r| {
            remote_files.get(&r).and_then(|files| {
                files
                    .iter()
                    .find(|f| f.same_as(m.origin.file_id, m.origin.file_name.as_deref()))
                    .map(|f| f.id.clone())
            })
        });
        if let Some(id) = existing_keys.get(&m.key).cloned().or(same_file) {
            key_to_id.insert(m.key.clone(), id.clone());
            report
                .skipped
                .push(format!("{} (already installed)", m.name));
            continue;
        }
        let staging = engine
            .dirs()
            .extract_tmp()
            .join(format!("interchange-{}", kombini_storage::random_hex(6)));
        let result = (|| -> Result<ModId, AppError> {
            let mut selection = Vec::new();
            for f in &m.files {
                let src = source_file(base, &f.path)?;
                if let Some(expected) = f.sha256.as_deref() {
                    let actual = sha256_file(&src)
                        .map_err(|e| AppError::from_io("Checking a bundled file", &src, &e))?;
                    if !actual.eq_ignore_ascii_case(expected) {
                        return Err(AppError::new(
                            ErrorCode::HashMismatch,
                            format!("{} is corrupt in the bundle", f.name),
                        ));
                    }
                }
                let rel = install_rel(f);
                let dest = bundle_file(&staging, &rel)?;
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| AppError::from_io("Preparing import", parent, &e))?;
                }
                std::fs::copy(&src, &dest)
                    .map_err(|e| AppError::from_io("Copying a bundled file", &src, &e))?;
                if f.selected.unwrap_or(true) && f.name.to_ascii_lowercase().ends_with(".vpk") {
                    selection.push(rel);
                }
            }
            let requirements: Vec<Requirement> =
                serde_json::from_value(m.requirements.clone()).unwrap_or_default();
            // The page's first file is `gb<id>`; its other files get their own id.
            let id = remote
                .filter(|r| remote_files.contains_key(r))
                .map(|r| sibling_id(r, m.origin.file_id, &m.key, &taken_ids));
            let meta = InstallMeta {
                id,
                title: Some(m.name.clone()).filter(|n| !n.is_empty()),
                source: Some(if remote.is_some() {
                    ModSource::Gamebanana
                } else {
                    ModSource::Local
                }),
                remote_id: remote,
                author: m.author.clone(),
                category: m.category.clone(),
                page_url: m.link.clone(),
                description: m.description.clone(),
                thumbnail_url: m.thumbnail_url.clone(),
                file_id: m.origin.file_id,
                file_name: m.origin.file_name.clone(),
                requirements,
                source_key: Some(m.key.clone()),
                enable: Some(false),
                selection: (!selection.is_empty()).then_some(selection),
                ..InstallMeta::default()
            };
            Ok(engine.install_dir(&staging, meta, progress)?.mod_id)
        })();
        let _ = std::fs::remove_dir_all(&staging);
        match result {
            Ok(id) => {
                if let Some(r) = remote {
                    remote_files.entry(r).or_default().push(FileRef {
                        file_id: m.origin.file_id,
                        file_name: m.origin.file_name.clone(),
                        id: id.clone(),
                    });
                }
                taken_ids.insert(id.clone());
                key_to_id.insert(m.key.clone(), id.clone());
                report.imported.push(id);
            }
            Err(e) => report.warnings.push(format!("{}: {e}", m.name)),
        }
    }
    Ok(key_to_id)
}

/// Creates or updates the document's profiles from `key_to_id`.
pub fn import_profiles(
    engine: &Engine,
    doc: &Document,
    key_to_id: &BTreeMap<String, ModId>,
    report: &mut ImportReport,
    progress: &dyn Progress,
) -> Result<(), AppError> {
    let existing_profiles = engine.with_state(|s| s.profiles.clone());
    for p in &doc.profiles {
        let mut refs = p.mods.clone();
        refs.sort_by_key(|r| r.order);
        let target = existing_profiles
            .iter()
            .find(|e| e.name == p.name)
            .cloned()
            .unwrap_or_else(|| {
                Profile::new(
                    ProfileId::new(format!("p{}", kombini_storage::random_hex(4))),
                    if p.name.is_empty() {
                        "Imported profile"
                    } else {
                        &p.name
                    },
                )
            });
        let mut profile = target;
        let mut order = Vec::new();
        for r in &refs {
            let Some(id) = key_to_id.get(&r.mod_key) else {
                continue;
            };
            order.push(id.clone());
            if r.enabled {
                profile.enabled.insert(id.clone());
            } else {
                profile.enabled.remove(id);
            }
        }
        profile.order.retain(|m| !order.contains(m));
        order.append(&mut profile.order);
        profile.order = order;
        if let Some(lines) = &p.autoexec {
            profile.autoexec = Some(lines.join("\n"));
        }
        let ext = p.extensions.get("kombini");
        if let Some(ext) = ext {
            let ids = |key: &str| -> Vec<ModId> {
                ext.get(key)
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str())
                            .map(|s| key_to_id.get(s).cloned().unwrap_or_else(|| ModId::new(s)))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let pinned = ids("pinned");
            if !pinned.is_empty() {
                profile.pinned = pinned;
            }
            if let Some(lo) = ext.get("launchOptions").and_then(|v| v.as_str()) {
                profile.launch_options = Some(lo.to_owned());
            }
            if let Some(list) = ext.get("ignoredConflicts").and_then(|v| v.as_array()) {
                profile.ignored_conflicts = list
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_lowercase))
                    .collect();
            }
        }
        let id = profile.id.clone();
        engine.put_profile(profile, p.active, progress)?;
        report.profiles.push(id);
    }
    Ok(())
}

/// An installed file of a GameBanana submission.
struct FileRef {
    file_id: Option<u64>,
    file_name: Option<String>,
    id: ModId,
}

impl FileRef {
    /// Whether an incoming file is this one. Files that can't be told apart
    /// (no file id or name on either side) count as the same.
    fn same_as(&self, file_id: Option<u64>, file_name: Option<&str>) -> bool {
        if let (Some(a), Some(b)) = (self.file_id, file_id) {
            return a == b;
        }
        match (self.file_name.as_deref(), file_name) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
            _ => true,
        }
    }
}

/// Id for another file of GameBanana submission `remote`, unique in `taken`.
fn sibling_id(remote: u64, file_id: Option<u64>, key: &str, taken: &BTreeSet<ModId>) -> ModId {
    let tail = file_id.map_or_else(|| slug(key), |f| f.to_string());
    let base: String = format!("gb{remote}_{tail}").chars().take(56).collect();
    let mut id = ModId::new(base.clone());
    let mut n = 2;
    while taken.contains(&id) {
        id = ModId::new(format!("{base}_{n}"));
        n += 1;
    }
    id
}

pub(crate) fn slug(text: &str) -> String {
    let s: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let s = s
        .split('_')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    let s: String = s.chars().take(40).collect();
    if s.is_empty() { "mod".into() } else { s }
}

/// Writes a bundle folder: `mod-interchange.json` and `files/<slug>/...`.
pub fn export(
    engine: &Engine,
    dest: &Path,
    include_profiles: bool,
    progress: &dyn Progress,
) -> Result<PathBuf, AppError> {
    let state = engine.state();
    if dest.join(MANIFEST).exists() {
        return Err(invalid(
            "That folder already contains an interchange export",
        ));
    }
    std::fs::create_dir_all(dest)
        .map_err(|e| AppError::from_io("Creating the export", dest, &e))?;
    let enabled = state.profile().enabled.clone();
    let order = engine.load_order();
    let mut mods = Vec::new();
    let mut used = BTreeSet::new();
    let total: u64 = state.mods.values().map(|m| m.total_size()).sum();
    let mut done = 0u64;
    for m in state.mods.values() {
        if progress.cancelled() {
            return Err(AppError::cancelled());
        }
        let mut s = slug(&m.title);
        let mut n = 2;
        while !used.insert(s.clone()) {
            s = format!("{}_{n}", slug(&m.title));
            n += 1;
        }
        let mut files = Vec::new();
        for f in &m.files {
            let rel = format!("files/{s}/{}", f.rel_path);
            let target = bundle_file(dest, &rel)?;
            engine
                .blobs()
                .copy_to(&f.blob, &target)
                .map_err(AppError::from)?;
            done += f.size;
            progress.progress(done, total);
            files.push(FileEntry {
                name: f.original_name.clone(),
                path: rel,
                sha256: Some(f.blob.as_str().to_owned()),
                size: Some(f.size),
                selected: Some(f.selected),
            });
        }
        let key = m.source_key.clone().unwrap_or_else(|| match m.remote_id {
            Some(id) => format!("gamebanana:mod:{id}"),
            None => format!("local:{}", m.id),
        });
        mods.push(ModEntry {
            key,
            name: m.title.clone(),
            enabled: Some(enabled.contains(&m.id)),
            order: order.iter().position(|o| o == &m.id).map(|p| p as i64),
            origin: match m.remote_id {
                Some(id) => Origin {
                    provider: "gamebanana".into(),
                    submission_type: Some("mod".into()),
                    submission_id: Some(serde_json::Value::String(id.to_string())),
                    file_id: m.file_id,
                    file_name: m.file_name.clone(),
                    local_id: None,
                },
                None => Origin {
                    provider: "local".into(),
                    local_id: Some(m.id.to_string()),
                    ..Origin::default()
                },
            },
            author: m.author.clone(),
            description: m.description.clone(),
            category: m.category.clone(),
            hero: m.primary_hero().map(str::to_owned),
            thumbnail_url: m.thumbnail_url.clone(),
            link: m.page_url.clone(),
            requirements: serde_json::to_value(&m.requirements).unwrap_or_default(),
            files,
            extensions: serde_json::json!({ "kombini": { "modId": m.id, "heroes": m.heroes } }),
        });
    }
    let key_of = |id: &ModId| -> Option<String> {
        let m = state.mods.get(id)?;
        Some(m.source_key.clone().unwrap_or_else(|| match m.remote_id {
            Some(r) => format!("gamebanana:mod:{r}"),
            None => format!("local:{}", m.id),
        }))
    };
    let profiles = if include_profiles {
        state
            .profiles
            .iter()
            .map(|p| ProfileEntry {
                key: format!("profile:{}", p.id),
                name: p.name.clone(),
                active: p.id == state.active_profile,
                description: None,
                mods: p
                    .order
                    .iter()
                    .enumerate()
                    .filter_map(|(i, id)| {
                        Some(ProfileModRef {
                            mod_key: key_of(id)?,
                            enabled: p.enabled.contains(id),
                            order: i as i64,
                        })
                    })
                    .collect(),
                autoexec: p
                    .autoexec
                    .as_ref()
                    .map(|a| a.lines().map(str::to_owned).collect()),
                extensions: serde_json::json!({
                    "kombini": {
                        "pinned": p.pinned.iter().filter_map(key_of).collect::<Vec<_>>(),
                        "launchOptions": p.launch_options,
                        "ignoredConflicts": p.ignored_conflicts,
                    }
                }),
            })
            .collect()
    } else {
        Vec::new()
    };
    let doc = Document {
        format: FORMAT.into(),
        version: 1,
        created_at: Some(format!("{}", kombini_storage::now())),
        source: serde_json::json!({ "manager": "kombini-rust", "managerVersion": env!("CARGO_PKG_VERSION") }),
        contents: if include_profiles {
            vec!["mods".into(), "profiles".into()]
        } else {
            vec!["mods".into()]
        },
        mods,
        profiles,
        warnings: Vec::new(),
    };
    let json = serde_json::to_vec_pretty(&doc).map_err(|e| AppError::internal(e.to_string()))?;
    let manifest = dest.join(MANIFEST);
    std::fs::write(&manifest, json)
        .map_err(|e| AppError::from_io("Writing the manifest", &manifest, &e))?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_confined() {
        let base = Path::new("/b");
        assert!(bundle_file(base, "files/x/pak01_dir.vpk").is_ok());
        assert!(bundle_file(base, "../x").is_err());
        assert!(bundle_file(base, "/etc/passwd").is_err());
        assert!(bundle_file(base, "files/../../x").is_err());
        assert_eq!(slug("Cool Mod: V2!"), "cool_mod_v2");
        assert_eq!(
            install_rel(&FileEntry {
                name: "pak01_dir.vpk".into(),
                path: "files/cool/Blue/pak01_dir.vpk".into(),
                ..FileEntry::default()
            }),
            "Blue/pak01_dir.vpk"
        );
    }
}
