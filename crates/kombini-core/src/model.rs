//! Domain records: mods, their files, profiles, groups and intelligence.

use crate::ids::{BlobHash, GroupId, ModId, ProfileId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModSource {
    Gamebanana,
    Local,
    Merged,
}

/// Where a managed file currently is on disk.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Placement {
    /// Parked in shard 1 (`addons`) under a name the game ignores.
    Disabled { file_name: String },
    /// Mounted as `pakNN_dir.vpk` / `pakNN_XXX.vpk` in a shard (1..=10).
    Mounted { shard: u8, file_name: String },
    /// Not found on disk during the last reconciliation.
    Missing,
}

impl Placement {
    pub fn is_mounted(&self) -> bool {
        matches!(self, Self::Mounted { .. })
    }
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
    /// Shard and file name, if the file is on disk.
    pub fn location(&self) -> Option<(u8, &str)> {
        match self {
            Self::Disabled { file_name } => Some((1, file_name)),
            Self::Mounted { shard, file_name } => Some((*shard, file_name)),
            Self::Missing => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModFile {
    /// Unique key within the mod: '/'-separated path inside the source
    /// archive/folder (e.g. `Blue/pak01_dir.vpk`).
    pub rel_path: String,
    /// File name as shipped (`pak01_dir.vpk`, `skin_dir.vpk`, `skin_000.vpk`).
    pub original_name: String,
    pub blob: BlobHash,
    pub size: u64,
    pub is_chunk: bool,
    /// For chunks: the `rel_path` of the root this chunk belongs to.
    pub parent: Option<String>,
    pub chunk_index: Option<u16>,
    /// Roots only: whether the user wants this root mounted. Chunks mirror their root.
    pub selected: bool,
    pub placement: Placement,
}

impl ModFile {
    pub fn is_root(&self) -> bool {
        !self.is_chunk
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateState {
    pub available: bool,
    pub latest_version: Option<String>,
    pub file_id: Option<u64>,
    pub checked_at: Option<i64>,
    pub skipped_file_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requirement {
    pub name: String,
    pub url: Option<String>,
    pub submission_id: Option<u64>,
    pub author: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafetyCode {
    GameplayData,
    Skeleton,
    Executable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafetyFinding {
    pub level: Level,
    pub code: SafetyCode,
    pub message: String,
    /// Up to 20 example paths.
    pub samples: Vec<String>,
    pub count: usize,
}

/// Summary health of a mod's files on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilesState {
    Ok,
    /// Some files missing on disk (restorable from the blob store).
    Broken,
    /// All files missing on disk.
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mod {
    pub id: ModId,
    pub title: String,
    pub source: ModSource,
    pub remote_id: Option<u64>,
    pub author: Option<String>,
    pub version: Option<String>,
    pub category: Option<String>,
    pub page_url: Option<String>,
    /// Plain text or sanitized HTML.
    pub description: Option<String>,
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub gallery: Vec<String>,
    pub file_id: Option<u64>,
    pub file_name: Option<String>,
    pub md5: Option<String>,
    pub variant_desc: Option<String>,
    /// Unix seconds.
    pub installed_at: i64,
    pub updated_at: i64,
    /// Remote "last updated" timestamp of the installed file/submission.
    pub remote_updated_at: Option<i64>,
    pub files: Vec<ModFile>,
    #[serde(default)]
    pub update: UpdateState,
    #[serde(default)]
    pub heroes: Vec<String>,
    pub hero_override: Option<String>,
    #[serde(default)]
    pub requirements: Vec<Requirement>,
    #[serde(default)]
    pub safety: Vec<SafetyFinding>,
    #[serde(default)]
    pub favorite: bool,
    pub source_key: Option<String>,
    pub notes: Option<String>,
    /// Path of the kept download archive, for reinstall without re-downloading.
    pub archive_path: Option<String>,
    /// Earlier versions replaced by updates, newest first (at most
    /// [`VERSION_HISTORY`]). Their blobs stay referenced, so a rollback
    /// needs no download.
    #[serde(default)]
    pub history: Vec<ModVersion>,
}

/// How many replaced versions a mod keeps.
pub const VERSION_HISTORY: usize = 3;

/// A version of a mod's files that an update replaced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModVersion {
    pub version: Option<String>,
    pub file_id: Option<u64>,
    pub file_name: Option<String>,
    pub md5: Option<String>,
    pub variant_desc: Option<String>,
    pub remote_updated_at: Option<i64>,
    pub archive_path: Option<String>,
    #[serde(default)]
    pub heroes: Vec<String>,
    #[serde(default)]
    pub safety: Vec<SafetyFinding>,
    /// Files with `Placement::Missing`; a restore places them again.
    pub files: Vec<ModFile>,
    /// When this version stopped being the installed one.
    pub replaced_at: i64,
}

impl Mod {
    /// A new, empty local mod record (files filled by the installer).
    pub fn new(id: ModId, title: impl Into<String>, source: ModSource, now: i64) -> Self {
        Self {
            id,
            title: title.into(),
            source,
            remote_id: None,
            author: None,
            version: None,
            category: None,
            page_url: None,
            description: None,
            thumbnail_url: None,
            gallery: Vec::new(),
            file_id: None,
            file_name: None,
            md5: None,
            variant_desc: None,
            installed_at: now,
            updated_at: now,
            remote_updated_at: None,
            files: Vec::new(),
            update: UpdateState::default(),
            heroes: Vec::new(),
            hero_override: None,
            requirements: Vec::new(),
            safety: Vec::new(),
            favorite: false,
            source_key: None,
            notes: None,
            archive_path: None,
            history: Vec::new(),
        }
    }

    /// The installed version as a history entry (files unplaced).
    pub fn current_version(&self, replaced_at: i64) -> ModVersion {
        let mut files = self.files.clone();
        for f in &mut files {
            f.placement = Placement::Missing;
        }
        ModVersion {
            version: self.version.clone(),
            file_id: self.file_id,
            file_name: self.file_name.clone(),
            md5: self.md5.clone(),
            variant_desc: self.variant_desc.clone(),
            remote_updated_at: self.remote_updated_at,
            archive_path: self.archive_path.clone(),
            heroes: self.heroes.clone(),
            safety: self.safety.clone(),
            files,
            replaced_at,
        }
    }

    /// Records the installed version before an update replaces it.
    pub fn push_history(&mut self, now: i64) {
        let entry = self.current_version(now);
        self.history.insert(0, entry);
        self.history.truncate(VERSION_HISTORY);
    }

    /// Swaps the installed files with history entry `index`; the version
    /// that was installed takes its place in the history. Returns false
    /// when `index` is out of range.
    pub fn restore_version(&mut self, index: usize, now: i64) -> bool {
        if index >= self.history.len() {
            return false;
        }
        let old = self.history.remove(index);
        let current = self.current_version(now);
        self.history.insert(0, current);
        self.history.truncate(VERSION_HISTORY);
        self.version = old.version;
        self.file_id = old.file_id;
        self.file_name = old.file_name;
        self.md5 = old.md5;
        self.variant_desc = old.variant_desc;
        self.remote_updated_at = old.remote_updated_at;
        self.archive_path = old.archive_path;
        self.heroes = old.heroes;
        self.safety = old.safety;
        self.files = old.files;
        self.updated_at = now;
        true
    }

    pub fn roots(&self) -> impl Iterator<Item = &ModFile> {
        self.files.iter().filter(|f| f.is_root())
    }

    pub fn selected_roots(&self) -> impl Iterator<Item = &ModFile> {
        self.roots().filter(|f| f.selected)
    }

    /// Chunks of the root with this `rel_path`, ordered by chunk index.
    pub fn chunks_of<'a>(&'a self, root_rel_path: &'a str) -> Vec<&'a ModFile> {
        let mut chunks: Vec<&ModFile> = self
            .files
            .iter()
            .filter(|f| f.is_chunk && f.parent.as_deref() == Some(root_rel_path))
            .collect();
        chunks.sort_by_key(|f| f.chunk_index.unwrap_or(0));
        chunks
    }

    pub fn file(&self, rel_path: &str) -> Option<&ModFile> {
        self.files.iter().find(|f| f.rel_path == rel_path)
    }

    pub fn file_mut(&mut self, rel_path: &str) -> Option<&mut ModFile> {
        self.files.iter_mut().find(|f| f.rel_path == rel_path)
    }

    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    pub fn files_state(&self) -> FilesState {
        let missing = self
            .files
            .iter()
            .filter(|f| f.placement.is_missing())
            .count();
        if missing == 0 {
            FilesState::Ok
        } else if missing == self.files.len() {
            FilesState::Missing
        } else {
            FilesState::Broken
        }
    }

    /// Effective hero: the user's override, else the GameBanana category
    /// when it names a hero (skins are filed under their hero), else the
    /// first detected hero.
    pub fn primary_hero(&self) -> Option<&str> {
        self.hero_override
            .as_deref()
            .or_else(|| self.category_hero())
            .or_else(|| self.heroes.first().map(String::as_str))
    }

    /// The hero named by the GameBanana category, if any.
    pub fn category_hero(&self) -> Option<&'static str> {
        let category = self.category.as_deref()?;
        crate::heroes::HeroTable::builtin().hero_from_name(category)
    }

    /// A hero skin: filed under a hero on GameBanana, or a local/merged mod
    /// the user assigned to a hero. Other hero mods (sounds, HUD icons)
    /// are not skins and can be combined freely.
    pub fn is_skin(&self) -> bool {
        self.category_hero().is_some() || (self.hero_override.is_some() && self.category.is_none())
    }

    /// Mounted pak slots `(shard, NN)` of this mod's roots, in file order.
    pub fn mounted_slots(&self) -> Vec<(u8, u8)> {
        self.roots()
            .filter_map(|f| match &f.placement {
                Placement::Mounted { shard, file_name } => {
                    crate::naming::parse_enabled_root(file_name).map(|slot| (*shard, slot))
                }
                _ => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub id: GroupId,
    pub name: String,
    pub mod_ids: Vec<ModId>,
    #[serde(default)]
    pub collapsed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub id: ProfileId,
    pub name: String,
    /// Every mod this profile has ordered (enabled or not); a mod keeps its
    /// position when disabled so re-enabling restores it.
    pub order: Vec<ModId>,
    /// Authoritative on/off set.
    pub enabled: BTreeSet<ModId>,
    /// Forced to the front of the load order, in this relative order.
    pub pinned: Vec<ModId>,
    pub groups: Vec<Group>,
    /// Lowercased game paths whose conflict warning is hidden.
    pub ignored_conflicts: BTreeSet<String>,
    /// None = inherit the current autoexec.cfg.
    pub autoexec: Option<String>,
    /// None = app default.
    pub launch_options: Option<String>,
}

impl Profile {
    pub fn new(id: ProfileId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            order: Vec::new(),
            enabled: BTreeSet::new(),
            pinned: Vec::new(),
            groups: Vec::new(),
            ignored_conflicts: BTreeSet::new(),
            autoexec: None,
            launch_options: None,
        }
    }

    pub fn default_profile() -> Self {
        Self::new(ProfileId::default_profile(), "Default")
    }

    pub fn is_enabled(&self, id: &ModId) -> bool {
        self.enabled.contains(id)
    }

    pub fn is_pinned(&self, id: &ModId) -> bool {
        self.pinned.contains(id)
    }

    /// Ensures `id` is in `order` (appended at the end when new).
    pub fn ensure_ordered(&mut self, id: &ModId) {
        if !self.order.contains(id) {
            self.order.push(id.clone());
        }
    }

    /// Removes every reference to a mod (uninstall).
    pub fn forget(&mut self, id: &ModId) {
        self.order.retain(|m| m != id);
        self.enabled.remove(id);
        self.pinned.retain(|m| m != id);
        for group in &mut self.groups {
            group.mod_ids.retain(|m| m != id);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthIssue {
    pub level: Level,
    /// Stable machine code, e.g. `missing_files`, `gameinfo_incomplete`.
    pub code: String,
    pub message: String,
    pub mod_id: Option<ModId>,
    pub fix: Option<FixAction>,
}

/// One-click repairs the UI can offer next to a health issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FixAction {
    ReapplyGameinfo,
    RepairGameinfoMarkers,
    ClearReadOnly,
    RestoreFiles { mod_id: ModId },
    Reinstall { mod_id: ModId },
    RemoveMod { mod_id: ModId },
    OpenConflicts,
    OpenImport,
    CheckUpdates,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignFile {
    pub shard: u8,
    pub file_name: String,
    pub size: u64,
    /// Slot this file occupies if the game mounts it.
    pub slot: Option<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(rel: &str, chunk: Option<(&str, u16)>, placement: Placement) -> ModFile {
        ModFile {
            rel_path: rel.into(),
            original_name: rel.rsplit('/').next().unwrap().into(),
            blob: BlobHash::from("00"),
            size: 10,
            is_chunk: chunk.is_some(),
            parent: chunk.map(|(p, _)| p.to_owned()),
            chunk_index: chunk.map(|(_, i)| i),
            selected: true,
            placement,
        }
    }

    #[test]
    fn files_state_and_chunks() {
        let mut m = Mod::new(ModId::from("gb1"), "x", ModSource::Gamebanana, 0);
        m.files = vec![
            file("a_dir.vpk", None, Placement::Missing),
            file("a_001.vpk", Some(("a_dir.vpk", 1)), Placement::Missing),
            file("a_000.vpk", Some(("a_dir.vpk", 0)), Placement::Missing),
        ];
        assert_eq!(m.files_state(), FilesState::Missing);
        m.files[0].placement = Placement::Mounted {
            shard: 2,
            file_name: "pak07_dir.vpk".into(),
        };
        assert_eq!(m.files_state(), FilesState::Broken);
        let chunks = m.chunks_of("a_dir.vpk");
        assert_eq!(chunks[0].chunk_index, Some(0));
        assert_eq!(m.mounted_slots(), vec![(2, 7)]);
    }

    #[test]
    fn profile_forget() {
        let mut p = Profile::default_profile();
        let id = ModId::from("gb1");
        p.ensure_ordered(&id);
        p.ensure_ordered(&id);
        p.enabled.insert(id.clone());
        p.pinned.push(id.clone());
        assert_eq!(p.order.len(), 1);
        p.forget(&id);
        assert!(p.order.is_empty() && p.enabled.is_empty() && p.pinned.is_empty());
    }

    #[test]
    fn placement_serde() {
        let json = serde_json::to_string(&Placement::Mounted {
            shard: 1,
            file_name: "pak01_dir.vpk".into(),
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"kind":"mounted","shard":1,"file_name":"pak01_dir.vpk"}"#
        );
    }
}
