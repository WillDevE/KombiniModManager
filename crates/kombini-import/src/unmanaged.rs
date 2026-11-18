//! Adopting unmanaged addon files and DMM's `.dmm.json`.

use kombini_core::naming::{parse_chunk_name, parse_enabled_root, root_base};
use kombini_core::{AppError, ForeignFile, ModId};
use kombini_engine::install::InstallMeta;
use kombini_engine::{Engine, Progress};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A group of foreign files that form one mod.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub key: String,
    pub title: String,
    /// Mounted by the game right now.
    pub enabled: bool,
    pub shard: u8,
    /// Pak slot for mounted roots (load priority; lower wins).
    pub slot: Option<u8>,
    pub files: Vec<ForeignFile>,
    pub remote_id: Option<u64>,
    pub size: u64,
    /// "addons", "dmm".
    pub origin: String,
}

/// Prefix id: `12345_name` or `gb12345_name`.
fn remote_prefix(name: &str) -> Option<(u64, String)> {
    let (prefix, rest) = name.split_once('_')?;
    let digits = prefix
        .strip_prefix("gb")
        .or_else(|| prefix.strip_prefix("GB"))
        .unwrap_or(prefix);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((digits.parse().ok()?, rest.to_owned()))
}

fn pretty(base: &str) -> String {
    let words: Vec<String> = base
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
        base.to_owned()
    } else {
        words.join(" ")
    }
}

/// Groups the engine's foreign files into adoptable candidates.
pub fn scan(engine: &Engine) -> Vec<Candidate> {
    let foreign = engine.foreign_files();
    // (shard, lowercase base) → files
    let mut groups: BTreeMap<(u8, String), Vec<ForeignFile>> = BTreeMap::new();
    for f in &foreign {
        let base = match parse_chunk_name(&f.file_name) {
            Some((base, _)) => base,
            None => root_base(&f.file_name),
        };
        groups
            .entry((f.shard, base.to_ascii_lowercase()))
            .or_default()
            .push(f.clone());
    }
    let mut out = Vec::new();
    for ((shard, _), files) in groups {
        let Some(root) = files
            .iter()
            .find(|f| parse_chunk_name(&f.file_name).is_none())
        else {
            continue; // orphan chunks are not adoptable
        };
        let slot = parse_enabled_root(&root.file_name);
        let base = root_base(&root.file_name);
        let (remote_id, title) = match (slot, remote_prefix(&base)) {
            (Some(slot), _) => (None, format!("Addon pak{slot:02} (folder {shard})")),
            (None, Some((id, rest))) => (Some(id), pretty(&rest)),
            (None, None) => (None, pretty(&base)),
        };
        let key = match remote_id {
            Some(id) => format!("gamebanana:mod:{id}"),
            None => format!("local:shard{shard}:{}", base.to_ascii_lowercase()),
        };
        out.push(Candidate {
            key,
            title,
            enabled: slot.is_some(),
            shard,
            slot,
            size: files.iter().map(|f| f.size).sum(),
            files,
            remote_id,
            origin: "addons".into(),
        });
    }
    // Mounted ones first, in load order.
    out.sort_by_key(|c| {
        (
            !c.enabled,
            c.shard,
            c.slot.unwrap_or(u8::MAX),
            c.title.clone(),
        )
    });
    out
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AdoptReport {
    pub adopted: Vec<ModId>,
    pub failed: Vec<(String, String)>,
}

/// Copies each candidate into the library, then swaps the originals out in
/// one transaction. Mounted candidates stay enabled, ahead of other mods, in
/// their original load order.
pub fn adopt(
    engine: &Engine,
    candidates: &[Candidate],
    progress: &dyn Progress,
) -> Result<AdoptReport, AppError> {
    let mut report = AdoptReport::default();
    let mut originals: Vec<ForeignFile> = Vec::new();
    let mut enabled: Vec<ModId> = Vec::new();
    for (i, c) in candidates.iter().enumerate() {
        progress.stage(&format!(
            "Importing {} ({}/{})",
            c.title,
            i + 1,
            candidates.len()
        ));
        let staging = engine
            .dirs()
            .extract_tmp()
            .join(format!("adopt-{}", kombini_storage::random_hex(6)));
        let result = (|| -> Result<ModId, AppError> {
            std::fs::create_dir_all(&staging)
                .map_err(|e| AppError::from_io("Preparing import", &staging, &e))?;
            for f in &c.files {
                let src = engine.paths().shard(f.shard).join(&f.file_name);
                // Name the copies like a normal mod so chunks group with their root.
                let name = match parse_chunk_name(&f.file_name) {
                    Some((_, idx)) => format!("{}_{idx:03}.vpk", root_name_base(c)),
                    None => format!("{}_dir.vpk", root_name_base(c)),
                };
                std::fs::copy(&src, staging.join(&name))
                    .map_err(|e| AppError::from_io("Copying for import", &src, &e))?;
            }
            let meta = InstallMeta {
                title: Some(c.title.clone()),
                remote_id: c.remote_id,
                page_url: c
                    .remote_id
                    .map(|id| format!("https://gamebanana.com/mods/{id}")),
                source_key: Some(c.key.clone()),
                enable: Some(false),
                ..InstallMeta::default()
            };
            Ok(engine.install_dir(&staging, meta, progress)?.mod_id)
        })();
        let _ = std::fs::remove_dir_all(&staging);
        match result {
            Ok(id) => {
                originals.extend(c.files.iter().cloned());
                if c.enabled {
                    enabled.push(id.clone());
                }
                report.adopted.push(id);
            }
            Err(e) => report.failed.push((c.title.clone(), e.to_string())),
        }
    }
    if report.adopted.is_empty() {
        return Ok(report);
    }
    let count = report.adopted.len();
    engine.adopt_foreign("import", progress, &originals, move |state| {
        let p = state.profile_mut();
        for id in &enabled {
            p.enabled.insert(id.clone());
        }
        p.order.retain(|m| !enabled.contains(m));
        for (i, id) in enabled.iter().enumerate() {
            p.order.insert(i, id.clone());
        }
        Ok(format!("Imported {count} existing mod(s)"))
    })?;
    Ok(report)
}

fn root_name_base(c: &Candidate) -> String {
    match c.slot {
        Some(slot) => format!("pak{slot:02}"),
        None => c
            .files
            .iter()
            .find(|f| parse_chunk_name(&f.file_name).is_none())
            .map(|f| root_base(&f.file_name))
            .unwrap_or_else(|| "mod".into()),
    }
}

// ------------------------------------------------------------------ DMM

/// `.dmm.json` mod entry (DMM's native manifest; read-only).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct DmmMod {
    enabled: bool,
    shard: Option<u8>,
    #[serde(rename = "currentVpks")]
    current_vpks: Vec<String>,
    #[serde(rename = "disabledVpks")]
    disabled_vpks: Vec<String>,
    name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct DmmManifest {
    mods: BTreeMap<String, DmmMod>,
}

/// Finds DMM manifests in `addons/` and its profile subfolders.
pub fn find_dmm_manifests(citadel: &Path) -> Vec<PathBuf> {
    let addons = citadel.join("addons");
    let mut out = Vec::new();
    let top = addons.join(".dmm.json");
    if top.is_file() {
        out.push(top);
    }
    if let Ok(entries) = std::fs::read_dir(&addons) {
        for e in entries.flatten() {
            let p = e.path().join(".dmm.json");
            if e.path().is_dir() && p.is_file() {
                out.push(p);
            }
        }
    }
    out
}

/// Titles, GameBanana ids and order from DMM's manifest, applied to
/// candidates whose files it lists (files in DMM profile subfolders are
/// not mounted by the game and are reported only).
pub fn enrich_from_dmm(citadel: &Path, candidates: &mut [Candidate]) -> Vec<String> {
    let mut notes = Vec::new();
    for manifest_path in find_dmm_manifests(citadel) {
        let Ok(bytes) = std::fs::read(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_slice::<DmmManifest>(&bytes) else {
            notes.push(format!("Could not read {}", manifest_path.display()));
            continue;
        };
        let in_profile_folder = manifest_path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|n| !n.eq_ignore_ascii_case("addons"));
        if in_profile_folder {
            notes.push(format!(
                "DMM profile folder {} has {} mod(s); enable that profile in DMM or move its files to import them.",
                manifest_path.parent().map(|p| p.display().to_string()).unwrap_or_default(),
                manifest.mods.len()
            ));
            continue;
        }
        for (id, m) in &manifest.mods {
            let names: Vec<String> = m
                .current_vpks
                .iter()
                .chain(m.disabled_vpks.iter())
                .map(|n| n.to_ascii_lowercase())
                .collect();
            for c in candidates.iter_mut() {
                let hit = c.files.iter().any(|f| {
                    names.contains(&f.file_name.to_ascii_lowercase())
                        && (m.shard.unwrap_or(1) == f.shard || !m.enabled)
                });
                if !hit {
                    continue;
                }
                c.origin = "dmm".into();
                if let Some(name) = m.name.as_ref().filter(|n| !n.trim().is_empty()) {
                    c.title = name.clone();
                } else if c.slot.is_some() {
                    c.title = format!("DMM mod {id}");
                }
                if let Ok(remote) = id.parse::<u64>() {
                    c.remote_id = Some(remote);
                    c.key = format!("gamebanana:mod:{remote}");
                }
            }
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_and_titles() {
        assert_eq!(
            remote_prefix("12345_cool_skin"),
            Some((12345, "cool_skin".into()))
        );
        assert_eq!(remote_prefix("gb77_x"), Some((77, "x".into())));
        assert_eq!(remote_prefix("pak01"), None);
        assert_eq!(remote_prefix("abc_x"), None);
        assert_eq!(pretty("cool_skin-v2"), "Cool Skin V2");
    }
}
