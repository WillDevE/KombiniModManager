//! Moving a library over from Deadlock Mod Manager (DMM) or Grimoire.
//!
//! Each reader turns the other manager's on-disk data into a mod
//! interchange [`Document`] whose files are absolute paths, which
//! [`migrate`] then imports. The DMM reader follows Grimoire's own DMM
//! migration (MIT; `dmmReader.ts`, `dmmState.ts`, `dmmMigration.ts`):
//! `state.json` is a Tauri store whose `local-config` value is a
//! zustand-persist envelope (a JSON string or an object), mod ids are
//! `123`, `snd-123` or `local-<uuid>`, and `.dmm.json` v1-v3 manifests list
//! each mod's files per profile folder and shard. Nothing the readers find
//! is written to.

use crate::interchange::{
    self, Document, FileEntry, ImportReport, ModEntry, Origin, ProfileEntry, ProfileModRef,
};
use kombini_core::AppError;
use kombini_core::naming::{parse_chunk_name, parse_enabled_root, root_base};
use kombini_engine::{Engine, Progress};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const DMM_ID: &str = "dev.stormix.deadlock-mod-manager";
const DMM_MANIFEST: &str = ".dmm.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Manager {
    Dmm,
    Grimoire,
}

impl Manager {
    pub fn name(self) -> &'static str {
        match self {
            Manager::Dmm => "Deadlock Mod Manager",
            Manager::Grimoire => "Grimoire",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Manager::Dmm => "DMM",
            Manager::Grimoire => "Grimoire",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Manager::Dmm => "dmm",
            Manager::Grimoire => "grimoire",
        }
    }
}

/// Another manager's library, read and ready to import.
#[derive(Debug, Clone)]
pub struct Found {
    pub manager: Manager,
    /// Where its data was read from.
    pub location: String,
    pub doc: Document,
    /// The manager's own bookkeeping inside the game folder (DMM manifests),
    /// moved aside with the mod files.
    pub bookkeeping: Vec<PathBuf>,
}

impl Found {
    pub fn enabled_count(&self) -> usize {
        self.doc
            .profiles
            .iter()
            .find(|p| p.active)
            .map(|p| p.mods.iter().filter(|m| m.enabled).count())
            .unwrap_or(0)
    }

    /// Every file a migration would take, lowercased.
    pub fn file_set(&self) -> BTreeSet<String> {
        self.doc
            .mods
            .iter()
            .flat_map(|m| m.files.iter())
            .map(|f| path_key(Path::new(&f.path)))
            .collect()
    }
}

/// Lowercased, `/`-separated path for comparing file sets.
pub fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

/// Finds DMM and Grimoire data for this game (their default locations).
pub fn detect(citadel: &Path) -> Vec<Found> {
    let mut out = Vec::new();
    if let Some(found) = read_dmm(citadel, &dmm_data_dirs()) {
        out.push(found);
    }
    if let Some(found) = grimoire_data_dirs()
        .iter()
        .find_map(|dir| read_grimoire(citadel, dir))
    {
        out.push(found);
    }
    out.retain(|f| !f.doc.mods.is_empty());
    out
}

fn env_dir(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn home_join(parts: &[&str]) -> Option<PathBuf> {
    let mut p = env_dir("HOME").or_else(|| env_dir("USERPROFILE"))?;
    for part in parts {
        p.push(part);
    }
    Some(p)
}

/// DMM's app data folders (Tauri's store dir varies by version).
pub fn dmm_data_dirs() -> Vec<PathBuf> {
    let mut bases = vec![env_dir("APPDATA"), env_dir("LOCALAPPDATA")];
    if !cfg!(windows) {
        bases.push(env_dir("XDG_DATA_HOME").or_else(|| home_join(&[".local", "share"])));
        bases.push(env_dir("XDG_CONFIG_HOME").or_else(|| home_join(&[".config"])));
    }
    bases
        .into_iter()
        .flatten()
        .map(|b| b.join(DMM_ID))
        .collect()
}

/// Grimoire's Electron `userData` folders.
pub fn grimoire_data_dirs() -> Vec<PathBuf> {
    let mut bases = vec![env_dir("APPDATA")];
    if !cfg!(windows) {
        bases.push(env_dir("XDG_CONFIG_HOME").or_else(|| home_join(&[".config"])));
    }
    let mut out = Vec::new();
    for base in bases.into_iter().flatten() {
        out.push(base.join("Grimoire"));
        if !cfg!(windows) {
            out.push(base.join("grimoire"));
        }
    }
    out
}

fn read_json(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn str_of(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn u64_of(v: &Value, key: &str) -> Option<u64> {
    match v.get(key)? {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn strip_archive_ext(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    for ext in [".zip", ".7z", ".rar", ".tar", ".gz", ".vpk"] {
        if lower.ends_with(ext) {
            return name[..name.len() - ext.len()].to_owned();
        }
    }
    name.to_owned()
}

fn pretty(stem: &str) -> String {
    let words: Vec<String> = stem
        .split(['_', '-', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .collect();
    if words.is_empty() {
        stem.to_owned()
    } else {
        words.join(" ")
    }
}

/// A VPK root plus its `_NNN` chunk siblings in the same folder.
fn with_chunks(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![root.to_path_buf()];
    let Some(name) = root.file_name().map(|n| n.to_string_lossy().to_string()) else {
        return out;
    };
    if !name.to_ascii_lowercase().ends_with("_dir.vpk") {
        return out;
    }
    let base = root_base(&name).to_ascii_lowercase();
    let Some(dir) = root.parent() else { return out };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut chunks: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .filter(|e| {
            parse_chunk_name(&e.file_name().to_string_lossy())
                .is_some_and(|(b, _)| b.to_ascii_lowercase() == base)
        })
        .map(|e| e.path())
        .collect();
    chunks.sort();
    out.extend(chunks);
    out
}

/// Interchange file entries; slot names (`pak01_dir.vpk`) get the mod's
/// name so they read well in the library and keep their chunks grouped.
fn file_entries(paths: &[PathBuf], title: &str) -> Vec<FileEntry> {
    let rename = |base: &str| -> String {
        if parse_enabled_root(&format!("{base}_dir.vpk")).is_some() {
            format!("{}_{}", interchange::slug(title), base.to_ascii_lowercase())
        } else {
            base.to_owned()
        }
    };
    let mut taken = BTreeSet::new();
    let mut out = Vec::new();
    for path in paths {
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut name = if let Some((base, idx)) = parse_chunk_name(&file) {
            format!("{}_{idx:03}.vpk", rename(&base))
        } else if file.to_ascii_lowercase().ends_with("_dir.vpk") {
            format!("{}_dir.vpk", rename(&root_base(&file)))
        } else {
            file.clone()
        };
        let mut n = 2;
        while !taken.insert(name.to_ascii_lowercase()) {
            name = format!("{n}_{file}");
            n += 1;
        }
        out.push(FileEntry {
            name,
            path: path.to_string_lossy().to_string(),
            sha256: None,
            size: std::fs::metadata(path).ok().map(|m| m.len()),
            selected: Some(true),
        });
    }
    out
}

fn gamebanana_origin(
    kind: &str,
    id: u64,
    file_id: Option<u64>,
    file_name: Option<String>,
) -> Origin {
    Origin {
        provider: "gamebanana".into(),
        submission_type: Some(kind.into()),
        submission_id: Some(Value::String(id.to_string())),
        file_id,
        file_name,
        local_id: None,
    }
}

// ------------------------------------------------------------------ DMM

#[derive(Debug, Clone, PartialEq, Eq)]
enum DmmId {
    Mod(u64),
    Sound(u64),
    Local(String),
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

fn parse_dmm_id(key: &str) -> Option<DmmId> {
    let (sound, digits) = match key.strip_prefix("snd-") {
        Some(rest) => (true, rest),
        None => (false, key),
    };
    if !digits.is_empty() && !digits.starts_with('0') && digits.bytes().all(|b| b.is_ascii_digit())
    {
        let id = digits.parse().ok()?;
        return Some(if sound {
            DmmId::Sound(id)
        } else {
            DmmId::Mod(id)
        });
    }
    let uuid = key.strip_prefix("local-")?;
    is_uuid(uuid).then(|| DmmId::Local(uuid.to_ascii_lowercase()))
}

impl DmmId {
    fn key(&self) -> String {
        match self {
            DmmId::Mod(id) => format!("gamebanana:mod:{id}"),
            DmmId::Sound(id) => format!("gamebanana:sound:{id}"),
            DmmId::Local(u) => format!("local:{u}"),
        }
    }
}

/// The DMM id a parked `<dmmId>_<name>.vpk` file belongs to.
fn dmm_id_from_file(name: &str) -> Option<String> {
    if !name.to_ascii_lowercase().ends_with(".vpk") {
        return None;
    }
    let (prefix, _) = name.split_once('_')?;
    parse_dmm_id(prefix).map(|_| prefix.to_owned())
}

/// What DMM's `state.json` knows about one mod.
#[derive(Debug, Clone, Default)]
struct DmmInfo {
    name: Option<String>,
    author: Option<String>,
    description: Option<String>,
    category: Option<String>,
    thumbnail: Option<String>,
    file_id: Option<u64>,
    download_name: Option<String>,
    install_order: Option<i64>,
    installed_vpks: Vec<String>,
    selected_vpks: Vec<String>,
}

/// The GameBanana file id in a DMM download url
/// (`gamebanana-file://<mod>/<file>` or `https://gamebanana.com/dl/<file>`).
fn file_id_from_url(url: &str) -> Option<u64> {
    let digits = if let Some(rest) = url.strip_prefix("gamebanana-file://") {
        rest.rsplit('/').next()?
    } else {
        let lower = url.to_ascii_lowercase();
        let at = lower.find("gamebanana.com/dl/")? + "gamebanana.com/dl/".len();
        let tail = &url[at..];
        let end = tail
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(tail.len());
        &tail[..end]
    };
    digits.parse().ok().filter(|id| *id > 0)
}

fn dmm_info(raw: &Value) -> Option<(String, DmmInfo)> {
    let remote = str_of(raw, "remoteId")?;
    let downloads: Vec<&Value> = match raw.get("selectedDownloads") {
        Some(Value::Array(list)) => list.iter().filter(|d| d.is_object()).collect(),
        _ => raw
            .get("selectedDownload")
            .filter(|d| d.is_object())
            .into_iter()
            .collect(),
    };
    let with_id = downloads.iter().find_map(|d| {
        d.get("url")
            .and_then(Value::as_str)
            .and_then(file_id_from_url)
            .map(|id| (id, *d))
    });
    let label = with_id
        .map(|(_, d)| d)
        .or_else(|| downloads.first().copied());
    let strings = |key: &str| -> Vec<String> {
        raw.get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let selected_vpks = raw
        .get("installedFileTree")
        .and_then(|t| t.get("files"))
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .filter(|f| f.get("is_selected").and_then(Value::as_bool) != Some(false))
                .filter_map(|f| str_of(f, "name"))
                .collect()
        })
        .unwrap_or_default();
    let info = DmmInfo {
        name: str_of(raw, "name"),
        author: str_of(raw, "author"),
        description: str_of(raw, "description"),
        category: str_of(raw, "category"),
        thumbnail: raw
            .get("images")
            .and_then(Value::as_array)
            .and_then(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .find(|u| u.starts_with("http://") || u.starts_with("https://"))
            })
            .map(str::to_owned),
        file_id: with_id.map(|(id, _)| id),
        download_name: label.and_then(|d| str_of(d, "name")),
        install_order: raw.get("installOrder").and_then(Value::as_i64),
        installed_vpks: strings("installedVpks"),
        selected_vpks,
    };
    Some((remote, info))
}

#[derive(Debug, Clone, Default)]
struct DmmProfile {
    id: String,
    name: String,
    folder: Option<String>,
    is_default: bool,
    enabled: BTreeMap<String, bool>,
    mods: Vec<(String, DmmInfo)>,
}

#[derive(Debug, Clone, Default)]
struct DmmState {
    active: Option<String>,
    local: Vec<(String, DmmInfo)>,
    profiles: Vec<DmmProfile>,
}

/// Unwraps `state.json` → `local-config` (string or object) → `state`.
fn dmm_state_value(text: &str) -> Option<Value> {
    let outer: Value = serde_json::from_str(text).ok()?;
    let mut inner = outer.get("local-config").cloned().unwrap_or(outer);
    if let Value::String(s) = &inner {
        inner = serde_json::from_str(s).ok()?;
    }
    let state = match inner.get("state") {
        Some(s) if s.is_object() => s.clone(),
        _ => inner,
    };
    state.is_object().then_some(state)
}

fn parse_dmm_state(text: &str) -> Option<DmmState> {
    let state = dmm_state_value(text)?;
    let mods = |v: Option<&Value>| -> Vec<(String, DmmInfo)> {
        v.and_then(Value::as_array)
            .map(|a| a.iter().filter_map(dmm_info).collect())
            .unwrap_or_default()
    };
    let mut profiles = Vec::new();
    if let Some(map) = state.get("profiles").and_then(Value::as_object) {
        for (id, raw) in map {
            if !raw.is_object() {
                continue;
            }
            let enabled = raw
                .get("enabledMods")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| {
                            v.get("enabled")
                                .and_then(Value::as_bool)
                                .map(|b| (k.clone(), b))
                        })
                        .collect()
                })
                .unwrap_or_default();
            profiles.push(DmmProfile {
                id: id.clone(),
                name: str_of(raw, "name").unwrap_or_else(|| id.clone()),
                folder: str_of(raw, "folderName"),
                is_default: raw.get("isDefault").and_then(Value::as_bool) == Some(true),
                enabled,
                mods: mods(raw.get("mods")),
            });
        }
    }
    Some(DmmState {
        active: str_of(&state, "activeProfileId"),
        local: mods(state.get("localMods")),
        profiles,
    })
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct DmmEntry {
    enabled: bool,
    order: Option<i64>,
    shard: Option<u8>,
    current_vpks: Vec<String>,
    disabled_vpks: Vec<String>,
    original_vpk_names: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct DmmManifest {
    mods: BTreeMap<String, DmmEntry>,
}

fn load_manifest(dir: &Path) -> Option<(PathBuf, DmmManifest)> {
    for name in [DMM_MANIFEST.to_owned(), format!("{DMM_MANIFEST}.tmp")] {
        let path = dir.join(name);
        if let Some(v) = read_json(&path)
            && let Ok(m) = serde_json::from_value::<DmmManifest>(v)
        {
            return Some((path, m));
        }
    }
    None
}

/// A manifest built from a state profile when the folder has none.
fn manifest_from_profile(p: &DmmProfile) -> DmmManifest {
    let mut mods = BTreeMap::new();
    for (id, info) in &p.mods {
        if parse_dmm_id(id).is_none() {
            continue;
        }
        let enabled = p.enabled.get(id).copied().unwrap_or(false);
        mods.insert(
            id.clone(),
            DmmEntry {
                enabled,
                order: info.install_order,
                shard: None,
                current_vpks: if enabled {
                    info.installed_vpks.clone()
                } else {
                    Vec::new()
                },
                disabled_vpks: if enabled {
                    Vec::new()
                } else {
                    info.installed_vpks.clone()
                },
                original_vpk_names: info.download_name.clone().into_iter().collect(),
            },
        );
    }
    DmmManifest { mods }
}

/// Shard `shard` of a profile folder: `addons<N>/<same subpath>`.
fn dmm_shard_dir(citadel: &Path, base: &Path, shard: u8) -> PathBuf {
    if shard <= 1 {
        return base.to_path_buf();
    }
    let addons = citadel.join("addons");
    match base.strip_prefix(&addons) {
        Ok(rest) => citadel.join(format!("addons{shard}")).join(rest),
        Err(_) => base.to_path_buf(),
    }
}

fn vpks_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(d) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                pending.push(p);
            } else if e
                .file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(".vpk")
            {
                found.push(p);
            }
        }
    }
    found.sort();
    found
}

struct DmmSource {
    id: String,
    name: String,
    dir: PathBuf,
}

/// DMM names a profile folder `profile_<time>_<rand>_<name>`.
fn dmm_profile_name(folder: &str) -> String {
    let parts: Vec<&str> = folder.splitn(4, '_').collect();
    let rest = if parts.len() == 4
        && parts[0].eq_ignore_ascii_case("profile")
        && parts[1].bytes().all(|b| b.is_ascii_digit())
    {
        parts[3]
    } else {
        folder
    };
    let name = rest.replace(['-', '_'], " ").trim().to_owned();
    if name.is_empty() {
        folder.to_owned()
    } else {
        name
    }
}

/// Reads every DMM profile: the union of their mods is the library, the
/// active profile's state comes first and is the one activated.
pub fn read_dmm(citadel: &Path, data_dirs: &[PathBuf]) -> Option<Found> {
    let addons = citadel.join("addons");
    let state_path = data_dirs
        .iter()
        .map(|d| d.join("state.json"))
        .find(|p| p.is_file());
    let state = state_path
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| parse_dmm_state(&t));

    let mut sources: Vec<DmmSource> = Vec::new();
    let mut active = None;
    if let Some(st) = &state {
        for p in &st.profiles {
            sources.push(DmmSource {
                id: p.id.clone(),
                name: p.name.clone(),
                dir: p
                    .folder
                    .as_ref()
                    .map(|f| addons.join(f))
                    .unwrap_or_else(|| addons.clone()),
            });
        }
        active = st
            .active
            .clone()
            .filter(|a| st.profiles.iter().any(|p| &p.id == a))
            .or_else(|| {
                st.profiles
                    .iter()
                    .find(|p| p.is_default)
                    .map(|p| p.id.clone())
            })
            .or_else(|| st.profiles.first().map(|p| p.id.clone()));
    }
    if sources.is_empty() {
        if load_manifest(&addons).is_some() {
            sources.push(DmmSource {
                id: "default".into(),
                name: "Default Profile".into(),
                dir: addons.clone(),
            });
        }
        if let Ok(entries) = std::fs::read_dir(&addons) {
            let mut dirs: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .filter(|p| {
                    !p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with('.'))
                })
                .filter(|p| load_manifest(p).is_some())
                .collect();
            dirs.sort();
            for dir in dirs {
                let folder = dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                sources.push(DmmSource {
                    id: format!("folder:{folder}"),
                    name: dmm_profile_name(&folder),
                    dir,
                });
            }
        }
        active = sources.first().map(|s| s.id.clone());
    }
    if sources.is_empty() {
        return None;
    }
    sources.sort_by_key(|s| Some(&s.id) != active.as_ref());

    let stores: Vec<PathBuf> = data_dirs.iter().map(|d| d.join("mods")).collect();
    let mut doc = Document {
        format: interchange::FORMAT.into(),
        version: 1,
        source: serde_json::json!({ "manager": "deadlock-mod-manager" }),
        ..Document::default()
    };
    let mut bookkeeping = Vec::new();
    let mut library: BTreeSet<String> = BTreeSet::new();
    let mut claimed: BTreeSet<String> = BTreeSet::new();
    for source in &sources {
        let is_active = Some(&source.id) == active.as_ref();
        let profile = state
            .as_ref()
            .and_then(|st| st.profiles.iter().find(|p| p.id == source.id));
        let manifest = match load_manifest(&source.dir) {
            Some((path, m)) => {
                bookkeeping.push(path);
                m
            }
            None => match profile {
                Some(p) => manifest_from_profile(p),
                None => continue,
            },
        };
        let info_of = |id: &str| -> Option<DmmInfo> {
            let st = state.as_ref()?;
            let pick = |list: &[(String, DmmInfo)]| {
                list.iter().find(|(k, _)| k == id).map(|(_, i)| i.clone())
            };
            profile
                .and_then(|p| pick(&p.mods))
                .or_else(|| pick(&st.local))
        };
        let mut refs = Vec::new();
        let mut fallback_order = 100_000i64;
        for (dmm_key, entry) in &manifest.mods {
            let Some(id) = parse_dmm_id(dmm_key) else {
                doc.warnings
                    .push(format!("Skipped an unrecognised DMM mod id: {dmm_key}"));
                continue;
            };
            let key = id.key();
            let info = info_of(dmm_key).unwrap_or_default();
            let order = info.install_order.or(entry.order).unwrap_or_else(|| {
                fallback_order += 1;
                fallback_order
            });
            refs.push(ProfileModRef {
                mod_key: key.clone(),
                enabled: entry.enabled,
                order,
            });
            if library.contains(&key) {
                continue;
            }
            let title = info
                .name
                .clone()
                .or_else(|| info.download_name.as_deref().map(strip_archive_ext))
                .or_else(|| {
                    entry
                        .original_vpk_names
                        .first()
                        .map(|n| strip_archive_ext(n))
                })
                .unwrap_or_else(|| format!("DMM mod {dmm_key}"));
            let files = dmm_files(citadel, &source.dir, dmm_key, entry, &info, &stores);
            let files: Vec<PathBuf> = files
                .into_iter()
                .filter(|p| claimed.insert(path_key(p)))
                .collect();
            if files.is_empty() {
                doc.warnings
                    .push(format!("Skipped {title}: its VPK file(s) are gone"));
                refs.pop();
                continue;
            }
            library.insert(key.clone());
            let (origin, link) = match &id {
                DmmId::Mod(n) => (
                    gamebanana_origin("mod", *n, info.file_id, info.download_name.clone()),
                    Some(format!("https://gamebanana.com/mods/{n}")),
                ),
                DmmId::Sound(n) => (
                    gamebanana_origin("sound", *n, info.file_id, info.download_name.clone()),
                    Some(format!("https://gamebanana.com/sounds/{n}")),
                ),
                DmmId::Local(u) => (
                    Origin {
                        provider: "local".into(),
                        local_id: Some(u.clone()),
                        ..Origin::default()
                    },
                    None,
                ),
            };
            doc.mods.push(ModEntry {
                key,
                name: title.clone(),
                enabled: Some(is_active && entry.enabled),
                order: Some(order),
                origin,
                author: info.author.clone(),
                description: info.description.clone(),
                category: info.category.clone(),
                hero: None,
                thumbnail_url: info.thumbnail.clone(),
                link,
                requirements: Value::Null,
                files: file_entries(&files, &title),
                extensions: Value::Null,
            });
        }
        doc.profiles.push(ProfileEntry {
            key: format!("profile:{}", source.id),
            name: source.name.clone(),
            active: is_active,
            mods: refs,
            ..ProfileEntry::default()
        });
    }
    doc.mods.sort_by_key(|m| m.order.unwrap_or(i64::MAX));
    let location = state_path
        .and_then(|p| p.parent().map(|d| d.display().to_string()))
        .unwrap_or_else(|| addons.display().to_string());
    Some(Found {
        manager: Manager::Dmm,
        location,
        doc,
        bookkeeping,
    })
}

/// Locates one DMM mod's files: the manifest's list for its state, then the
/// other list, then parked `<id>_*.vpk` files, then DMM's download cache.
fn dmm_files(
    citadel: &Path,
    base: &Path,
    dmm_key: &str,
    entry: &DmmEntry,
    info: &DmmInfo,
    stores: &[PathBuf],
) -> Vec<PathBuf> {
    let shard_dir = dmm_shard_dir(citadel, base, entry.shard.unwrap_or(1));
    let (first, second) = if entry.enabled {
        (&entry.current_vpks, &entry.disabled_vpks)
    } else {
        (&entry.disabled_vpks, &entry.current_vpks)
    };
    let names = if first.is_empty() { second } else { first };
    let mut found: Vec<PathBuf> = Vec::new();
    for name in names {
        let candidates: Vec<PathBuf> = if Path::new(name).is_absolute() {
            vec![PathBuf::from(name)]
        } else if parse_enabled_root(name).is_some()
            || parse_chunk_name(name)
                .is_some_and(|(b, _)| parse_enabled_root(&format!("{b}_dir.vpk")).is_some())
        {
            // Slot names only mean something in the folder DMM recorded.
            vec![if entry.enabled {
                shard_dir.join(name)
            } else {
                base.join(name)
            }]
        } else {
            vec![shard_dir.join(name), base.join(name)]
        };
        if let Some(p) = candidates.into_iter().find(|p| p.is_file()) {
            found.push(p);
        }
    }
    if found.is_empty() {
        for dir in [&shard_dir, &base.to_path_buf()] {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            let mut parked: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| e.path().is_file())
                .filter(|e| {
                    dmm_id_from_file(&e.file_name().to_string_lossy()).as_deref() == Some(dmm_key)
                })
                .map(|e| e.path())
                .collect();
            parked.sort();
            found.extend(parked);
            if !found.is_empty() {
                break;
            }
        }
    }
    if found.is_empty() {
        let wanted: BTreeSet<String> = info
            .selected_vpks
            .iter()
            .chain(names.iter())
            .map(|n| n.to_ascii_lowercase())
            .collect();
        for store in stores {
            let files = vpks_under(&store.join(dmm_key).join("files"));
            if files.is_empty() {
                continue;
            }
            let picked: Vec<PathBuf> = files
                .iter()
                .filter(|f| {
                    f.file_name()
                        .is_some_and(|n| wanted.contains(&n.to_string_lossy().to_ascii_lowercase()))
                })
                .cloned()
                .collect();
            return if picked.is_empty() { files } else { picked };
        }
        return found;
    }
    // Chunks travel with their root.
    let mut out: Vec<PathBuf> = Vec::new();
    for p in found {
        for f in with_chunks(&p) {
            if !out.contains(&f) {
                out.push(f);
            }
        }
    }
    out
}

// ------------------------------------------------------------- Grimoire

/// Reads Grimoire's library: every VPK in its folders (with the names and
/// GameBanana ids from `mod-metadata.json`), the current state as the active
/// profile, and its saved profiles from `profiles.json`.
pub fn read_grimoire(citadel: &Path, data_dir: &Path) -> Option<Found> {
    let metadata_path = data_dir.join("mod-metadata.json");
    let profiles_path = data_dir.join("profiles.json");
    if !metadata_path.is_file() && !profiles_path.is_file() {
        return None;
    }
    let metadata = read_json(&metadata_path).unwrap_or(Value::Null);
    let addons = citadel.join("addons");

    // (folder, metadata key prefix, enabled)
    let mut roots: Vec<(PathBuf, Option<String>, bool)> = vec![
        (citadel.join("grimoire"), Some("grimoire".into()), true),
        (addons.clone(), None, true),
    ];
    let mut overflow: Vec<(u32, PathBuf)> = std::fs::read_dir(citadel)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    let n: u32 = name
                        .to_ascii_lowercase()
                        .strip_prefix("addons")?
                        .parse()
                        .ok()?;
                    Some((n, e.path()))
                })
                .collect()
        })
        .unwrap_or_default();
    overflow.sort();
    for (_, path) in overflow {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string());
        roots.push((path, name, true));
    }
    roots.push((addons.join(".disabled"), None, false));

    struct Installed {
        key: String,
        file_name: String,
        enabled: bool,
        rank: i64,
        meta: Value,
    }
    let mut installed: Vec<Installed> = Vec::new();
    let mut doc = Document {
        format: interchange::FORMAT.into(),
        version: 1,
        source: serde_json::json!({ "manager": "grimoire" }),
        ..Document::default()
    };
    for (root_index, (dir, prefix, enabled)) in roots.iter().enumerate() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|e| e.path().is_file())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.to_ascii_lowercase().ends_with("_dir.vpk"))
            .collect();
        names.sort_by_key(|n| n.to_ascii_lowercase());
        for file_name in names {
            let slot = parse_enabled_root(&file_name);
            if prefix.as_deref() == Some("grimoire") && slot.is_some_and(|s| s < 5) {
                continue; // Grimoire's own Locker files
            }
            let meta_key = match prefix {
                Some(p) => format!("{p}/{file_name}"),
                None => file_name.clone(),
            };
            let meta = metadata.get(&meta_key).cloned().unwrap_or(Value::Null);
            let path = dir.join(&file_name);
            let stem = root_base(&file_name);
            let mut title = str_of(&meta, "modName").unwrap_or_else(|| pretty(&stem));
            if let Some(label) = str_of(&meta, "variantLabel") {
                title = format!("{title} ({label})");
            }
            let gb = u64_of(&meta, "gameBananaId").filter(|id| *id > 0);
            let section = str_of(&meta, "sourceSection")
                .unwrap_or_default()
                .to_ascii_lowercase();
            let (origin, link) = match gb {
                Some(id) if section.starts_with("sound") => (
                    gamebanana_origin(
                        "sound",
                        id,
                        u64_of(&meta, "gameBananaFileId"),
                        str_of(&meta, "sourceFileName"),
                    ),
                    Some(format!("https://gamebanana.com/sounds/{id}")),
                ),
                Some(id) => (
                    gamebanana_origin(
                        "mod",
                        id,
                        u64_of(&meta, "gameBananaFileId"),
                        str_of(&meta, "sourceFileName"),
                    ),
                    Some(format!("https://gamebanana.com/mods/{id}")),
                ),
                None => (
                    Origin {
                        provider: "local".into(),
                        ..Origin::default()
                    },
                    None,
                ),
            };
            let rank = root_index as i64 * 1000 + slot.map(i64::from).unwrap_or(999);
            let key = format!("grimoire:{}", meta_key.to_lowercase());
            doc.mods.push(ModEntry {
                key: key.clone(),
                name: title.clone(),
                enabled: Some(*enabled),
                order: Some(rank),
                origin,
                author: str_of(&meta, "author"),
                category: str_of(&meta, "categoryName"),
                thumbnail_url: str_of(&meta, "thumbnailUrl"),
                link,
                requirements: Value::Null,
                files: file_entries(&with_chunks(&path), &title),
                extensions: Value::Null,
                ..ModEntry::default()
            });
            installed.push(Installed {
                key,
                file_name,
                enabled: *enabled,
                rank,
                meta,
            });
        }
    }
    if installed.is_empty() {
        return None;
    }
    doc.profiles.push(ProfileEntry {
        key: "profile:grimoire-current".into(),
        name: "Grimoire".into(),
        active: true,
        mods: installed
            .iter()
            .map(|m| ProfileModRef {
                mod_key: m.key.clone(),
                enabled: m.enabled,
                order: m.rank,
            })
            .collect(),
        ..ProfileEntry::default()
    });

    // Saved profiles: match each entry by content hash, then GameBanana id,
    // then file name.
    if let Some(Value::Array(profiles)) = read_json(&profiles_path) {
        for p in &profiles {
            let Some(name) = str_of(p, "name") else {
                continue;
            };
            let mut used: BTreeSet<String> = BTreeSet::new();
            let mut refs = Vec::new();
            for pm in p
                .get("mods")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let sha = str_of(pm, "sha256").map(|s| s.to_ascii_lowercase());
                let gb = u64_of(pm, "gameBananaId");
                let file = str_of(pm, "fileName").map(|s| s.to_ascii_lowercase());
                let free = |m: &&Installed| !used.contains(&m.key);
                let hit = installed
                    .iter()
                    .filter(free)
                    .find(|m| {
                        sha.is_some()
                            && str_of(&m.meta, "sha256").map(|s| s.to_ascii_lowercase()) == sha
                    })
                    .or_else(|| {
                        installed
                            .iter()
                            .filter(free)
                            .find(|m| gb.is_some() && u64_of(&m.meta, "gameBananaId") == gb)
                    })
                    .or_else(|| {
                        installed.iter().filter(free).find(|m| {
                            file.as_deref() == Some(m.file_name.to_ascii_lowercase().as_str())
                        })
                    });
                let Some(hit) = hit else { continue };
                used.insert(hit.key.clone());
                refs.push(ProfileModRef {
                    mod_key: hit.key.clone(),
                    enabled: pm.get("enabled").and_then(Value::as_bool).unwrap_or(false),
                    order: pm.get("priority").and_then(Value::as_i64).unwrap_or(0),
                });
            }
            doc.profiles.push(ProfileEntry {
                key: format!(
                    "profile:{}",
                    str_of(p, "id").unwrap_or_else(|| name.clone())
                ),
                name: format!("Grimoire: {name}"),
                active: false,
                mods: refs,
                ..ProfileEntry::default()
            });
        }
    }
    Some(Found {
        manager: Manager::Grimoire,
        location: data_dir.display().to_string(),
        doc,
        bookkeeping: Vec::new(),
    })
}

// -------------------------------------------------------------- migrate

#[derive(Debug, Clone, Default, Serialize)]
pub struct MigrateReport {
    pub imported: usize,
    /// Mods the library already had.
    pub already: usize,
    pub profiles: usize,
    /// Files moved out of the game folder.
    pub moved: usize,
    pub backup: Option<PathBuf>,
    pub warnings: Vec<String>,
}

fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    // Another drive: copy, then remove.
    std::fs::copy(from, to)?;
    std::fs::remove_file(from)
}

/// Imports another manager's library: its mods go into the library, the
/// files it had in the game folder move to a backup (so nothing loads
/// twice), and its profiles are recreated with the active one switched to.
pub fn migrate(
    engine: &Engine,
    found: &Found,
    progress: &dyn Progress,
) -> Result<MigrateReport, AppError> {
    if engine.game_running() {
        return Err(AppError::invalid("Close Deadlock before importing."));
    }
    let doc = &found.doc;
    let mut report = ImportReport::default();
    let key_to_id = interchange::import_mods(engine, None, doc, None, &mut report, progress)?;

    let citadel = engine.paths().citadel.clone();
    let backup = engine.dirs().data.join("imports").join(format!(
        "{}-{}",
        found.manager.id(),
        kombini_storage::now()
    ));
    let mut moved = 0;
    let mut touched_dirs: BTreeSet<PathBuf> = BTreeSet::new();
    let mut take = |from: &Path, warnings: &mut Vec<String>| {
        let Ok(rel) = from.strip_prefix(&citadel) else {
            return;
        };
        if !from.is_file() {
            return;
        }
        match move_file(from, &backup.join(rel)) {
            Ok(()) => {
                moved += 1;
                if let Some(dir) = from.parent() {
                    touched_dirs.insert(dir.to_path_buf());
                }
            }
            Err(e) => warnings.push(format!("Could not move {}: {e}", from.display())),
        }
    };
    progress.stage("Moving the old files aside");
    for m in &doc.mods {
        if !key_to_id.contains_key(&m.key) {
            continue;
        }
        for f in &m.files {
            take(Path::new(&f.path), &mut report.warnings);
        }
    }
    if !key_to_id.is_empty() {
        for path in &found.bookkeeping {
            take(path, &mut report.warnings);
        }
    }
    // Drop folders the other manager is left with nothing in (never the
    // game's own addons folder).
    for dir in touched_dirs.iter().rev() {
        if *dir != citadel.join("addons") {
            let _ = std::fs::remove_dir(dir);
        }
    }
    engine.rescan()?;
    interchange::import_profiles(engine, doc, &key_to_id, &mut report, progress)?;

    let already = report.skipped.len();
    Ok(MigrateReport {
        imported: report.imported.len(),
        already,
        profiles: report.profiles.len(),
        moved,
        backup: (moved > 0).then_some(backup),
        warnings: report.warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dmm_ids() {
        assert_eq!(parse_dmm_id("123"), Some(DmmId::Mod(123)));
        assert_eq!(parse_dmm_id("snd-7"), Some(DmmId::Sound(7)));
        assert_eq!(
            parse_dmm_id("local-0f8fad5b-d9cb-469f-a165-70867728950e"),
            Some(DmmId::Local("0f8fad5b-d9cb-469f-a165-70867728950e".into()))
        );
        assert_eq!(parse_dmm_id("012"), None);
        assert_eq!(parse_dmm_id("local-nope"), None);
        assert_eq!(dmm_id_from_file("123_cool.vpk").as_deref(), Some("123"));
        assert_eq!(dmm_id_from_file("pak01_dir.vpk"), None);
    }

    #[test]
    fn file_ids_from_urls() {
        assert_eq!(
            file_id_from_url("https://gamebanana.com/dl/1234"),
            Some(1234)
        );
        assert_eq!(file_id_from_url("gamebanana-file://55/987"), Some(987));
        assert_eq!(file_id_from_url("https://example.com/x/dl/5"), None);
    }

    #[test]
    fn state_envelope_as_string_or_object() {
        let inner = r#"{"state":{"activeProfileId":"p1","localMods":[{"remoteId":"42","name":"Cool","selectedDownloads":[{"url":"https://gamebanana.com/dl/9","name":"cool.zip"}]}],"profiles":{"p1":{"name":"Main","folderName":null,"isDefault":true,"enabledMods":{"42":{"enabled":true}},"mods":[]}}},"version":3}"#;
        let as_string = serde_json::json!({ "local-config": inner }).to_string();
        let as_object = format!(r#"{{"local-config":{inner}}}"#);
        for text in [as_string, as_object] {
            let st = parse_dmm_state(&text).unwrap();
            assert_eq!(st.active.as_deref(), Some("p1"));
            assert_eq!(st.local[0].0, "42");
            assert_eq!(st.local[0].1.file_id, Some(9));
            assert_eq!(st.local[0].1.download_name.as_deref(), Some("cool.zip"));
            assert_eq!(st.profiles[0].enabled.get("42"), Some(&true));
        }
    }

    #[test]
    fn profile_folder_names() {
        assert_eq!(dmm_profile_name("profile_1712_ab12cd_My-Comp"), "My Comp");
        assert_eq!(dmm_profile_name("custom"), "custom");
    }

    #[test]
    fn slot_files_take_the_mod_name() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("pak03_dir.vpk");
        std::fs::write(&root, b"x").unwrap();
        std::fs::write(dir.path().join("pak03_000.vpk"), b"x").unwrap();
        std::fs::write(dir.path().join("pak04_000.vpk"), b"x").unwrap();
        let files = file_entries(&with_chunks(&root), "Cool Skin!");
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            ["cool_skin_pak03_dir.vpk", "cool_skin_pak03_000.vpk"]
        );
    }
}
