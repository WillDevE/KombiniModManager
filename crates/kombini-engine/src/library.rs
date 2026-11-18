//! Library use cases: enable/disable, load order, selection, uninstall,
//! metadata, profiles, groups, conflicts and autoexec.

use crate::{ChangeReport, Engine, NoProgress, Progress, Result};
use kombini_core::conflicts::{ConflictPair, summarize_pairs};
use kombini_core::order::{move_in_order, set_enabled_order};
use kombini_core::{
    AppError, BlobHash, Conflict, ErrorCode, Group, GroupId, ModId, Profile, ProfileId,
    effective_order, find_conflicts,
};
use kombini_storage::LibraryState;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Metadata-only edits (no files move).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModPatch {
    pub title: Option<String>,
    pub favorite: Option<bool>,
    /// `Some(None)` clears.
    pub notes: Option<Option<String>>,
    /// `Some(None)` clears.
    pub hero_override: Option<Option<String>>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ConflictsView {
    pub conflicts: Vec<Conflict>,
    pub pairs: Vec<ConflictPair>,
    /// Enabled mods every one of whose files is overridden by a higher mod.
    pub fully_overridden: Vec<ModId>,
    /// Mods whose VPK could not be read (reported, not fatal).
    pub unreadable: Vec<ModId>,
}

fn require_mod(state: &LibraryState, id: &ModId) -> Result<()> {
    if state.mods.contains_key(id) {
        Ok(())
    } else {
        Err(AppError::mod_not_found(id))
    }
}

fn profile_index(state: &LibraryState, id: &ProfileId) -> Result<usize> {
    state
        .profiles
        .iter()
        .position(|p| &p.id == id)
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::ProfileNotFound,
                format!("Profile {id} was not found"),
            )
        })
}

fn group_mut<'a>(profile: &'a mut Profile, id: &GroupId) -> Result<&'a mut Group> {
    profile
        .groups
        .iter_mut()
        .find(|g| &g.id == id)
        .ok_or_else(|| AppError::invalid(format!("Group {id} was not found")))
}

fn title(state: &LibraryState, id: &ModId) -> String {
    state
        .mods
        .get(id)
        .map(|m| m.title.clone())
        .unwrap_or_else(|| id.to_string())
}

impl Engine {
    /// Effective load order of the active profile (index 0 wins).
    pub fn load_order(&self) -> Vec<ModId> {
        self.with_state(|s| {
            let existing: BTreeSet<ModId> = s.mods.keys().cloned().collect();
            effective_order(s.profile(), &existing)
        })
    }

    pub fn set_enabled(&self, ids: &[ModId], enabled: bool) -> Result<ChangeReport> {
        let ids = ids.to_vec();
        self.mutate(
            if enabled { "enable" } else { "disable" },
            &NoProgress,
            |state| {
                for id in &ids {
                    require_mod(state, id)?;
                }
                let profile = state.profile_mut();
                for id in &ids {
                    profile.ensure_ordered(id);
                    if enabled {
                        profile.enabled.insert(id.clone());
                    } else {
                        profile.enabled.remove(id);
                    }
                }
                Ok(match ids.as_slice() {
                    [one] => format!(
                        "{} {}",
                        if enabled { "Enabled" } else { "Disabled" },
                        title(state, one)
                    ),
                    many => format!(
                        "{} {} mods",
                        if enabled { "Enabled" } else { "Disabled" },
                        many.len()
                    ),
                })
            },
        )
    }

    /// Puts back the version at `index` of the mod's history (rollback); the
    /// installed version moves into the history so it can be restored too.
    pub fn restore_version(&self, id: &ModId, index: usize) -> Result<ChangeReport> {
        let id = id.clone();
        let now = kombini_storage::now();
        self.mutate("restore_version", &NoProgress, move |state| {
            let m = state
                .mods
                .get_mut(&id)
                .ok_or_else(|| AppError::mod_not_found(&id))?;
            if !m.restore_version(index, now) {
                return Err(AppError::invalid("That version is no longer kept"));
            }
            Ok(format!(
                "Restored {}{}",
                m.title,
                m.version
                    .as_deref()
                    .map(|v| format!(" v{}", v.trim_start_matches(['v', 'V'])))
                    .unwrap_or_default()
            ))
        })
    }

    /// Enables `enable` and disables `disable` in one transaction (skin
    /// swaps, shuffles). `summary` becomes the activity entry.
    pub fn set_enabled_many(
        &self,
        enable: &[ModId],
        disable: &[ModId],
        summary: &str,
    ) -> Result<ChangeReport> {
        let (enable, disable, summary) = (enable.to_vec(), disable.to_vec(), summary.to_owned());
        self.mutate("enable", &NoProgress, |state| {
            for id in enable.iter().chain(&disable) {
                require_mod(state, id)?;
            }
            let profile = state.profile_mut();
            for id in &disable {
                profile.enabled.remove(id);
            }
            for id in &enable {
                profile.ensure_ordered(id);
                profile.enabled.insert(id.clone());
            }
            Ok(summary)
        })
    }

    /// Replaces the order of the enabled mods (drag and drop of the enabled list).
    pub fn reorder_enabled(&self, enabled_order: &[ModId]) -> Result<ChangeReport> {
        let order = enabled_order.to_vec();
        self.mutate("reorder", &NoProgress, |state| {
            set_enabled_order(state.profile_mut(), &order);
            Ok("Load order updated".to_owned())
        })
    }

    /// Moves one mod to `index` in the full profile order.
    pub fn move_mod(&self, id: &ModId, index: usize) -> Result<ChangeReport> {
        let id = id.clone();
        self.mutate("reorder", &NoProgress, |state| {
            require_mod(state, &id)?;
            move_in_order(state.profile_mut(), &id, index);
            Ok(format!("Moved {}", title(state, &id)))
        })
    }

    pub fn set_pinned(&self, id: &ModId, pinned: bool) -> Result<ChangeReport> {
        let id = id.clone();
        self.mutate("pin", &NoProgress, |state| {
            require_mod(state, &id)?;
            let profile = state.profile_mut();
            profile.pinned.retain(|m| m != &id);
            if pinned {
                profile.pinned.push(id.clone());
            }
            Ok(format!(
                "{} {}",
                if pinned { "Pinned" } else { "Unpinned" },
                title(state, &id)
            ))
        })
    }

    /// Chooses which roots (variants) of a mod are mounted.
    pub fn set_selection(&self, id: &ModId, selected_roots: &[String]) -> Result<ChangeReport> {
        let id = id.clone();
        let wanted: BTreeSet<String> = selected_roots.iter().cloned().collect();
        self.mutate("select_variant", &NoProgress, |state| {
            let m = state
                .mods
                .get_mut(&id)
                .ok_or_else(|| AppError::mod_not_found(&id))?;
            if !m.roots().any(|f| wanted.contains(&f.rel_path)) {
                return Err(AppError::invalid("Select at least one file of this mod"));
            }
            for f in &mut m.files {
                f.selected = if f.is_chunk {
                    f.parent.as_ref().is_some_and(|p| wanted.contains(p))
                } else {
                    wanted.contains(&f.rel_path)
                };
            }
            Ok(format!("Updated files of {}", m.title))
        })
    }

    /// Removes mods and their files (blobs stay until GC; snapshots keep them).
    pub fn uninstall(&self, ids: &[ModId]) -> Result<ChangeReport> {
        let ids = ids.to_vec();
        if self.settings().auto_snapshot {
            let label = format!("Before uninstalling {} mod(s)", ids.len());
            if let Err(e) = self.create_snapshot(&label, crate::snapshots::SnapshotKind::Auto) {
                tracing::warn!(error = %e, "auto snapshot before uninstall failed");
            }
        }
        self.mutate("uninstall", &NoProgress, |state| {
            let mut names = Vec::new();
            for id in &ids {
                require_mod(state, id)?;
                names.push(title(state, id));
            }
            for id in &ids {
                state.mods.remove(id);
                for profile in &mut state.profiles {
                    profile.forget(id);
                }
            }
            Ok(match names.as_slice() {
                [one] => format!("Uninstalled {one}"),
                many => format!("Uninstalled {} mods", many.len()),
            })
        })
    }

    /// Metadata edits; no files move, so the game may be running.
    pub fn update_mod(&self, id: &ModId, patch: ModPatch) -> Result<()> {
        let _writer = self.writer.lock();
        let mut state = self.state.write();
        let m = state
            .mods
            .get_mut(id)
            .ok_or_else(|| AppError::mod_not_found(id))?;
        if let Some(t) = patch.title {
            let t = t.trim().to_owned();
            if t.is_empty() {
                return Err(AppError::invalid("The title cannot be empty"));
            }
            m.title = t;
        }
        if let Some(f) = patch.favorite {
            m.favorite = f;
        }
        if let Some(n) = patch.notes {
            m.notes = n.filter(|s| !s.trim().is_empty());
        }
        if let Some(h) = patch.hero_override {
            m.hero_override = h.filter(|s| !s.trim().is_empty());
        }
        self.db.lock().save_mod(self.game_id.as_str(), m)?;
        Ok(())
    }

    /// Restores missing files from the blob store (a plain re-apply).
    pub fn restore_files(&self, progress: &dyn Progress) -> Result<ChangeReport> {
        self.mutate("restore_files", progress, |_| {
            Ok("Restored missing files".to_owned())
        })
    }

    // ------------------------------------------------------------ profiles

    pub fn create_profile(&self, name: &str, copy_from: Option<&ProfileId>) -> Result<ProfileId> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::invalid("Profile name cannot be empty"));
        }
        let id = ProfileId::new(format!("p{}", kombini_storage::random_hex(4)));
        let _writer = self.writer.lock();
        let mut state = self.state.read().clone();
        let mut profile = match copy_from {
            Some(src) => state.profiles[profile_index(&state, src)?].clone(),
            None => Profile::new(id.clone(), name),
        };
        profile.id = id.clone();
        profile.name = name.to_owned();
        if copy_from.is_none() {
            profile.order = state.mods.keys().cloned().collect();
        }
        state.profiles.push(profile);
        self.db
            .lock()
            .save_library(self.game_id.as_str(), &state, None)?;
        *self.state.write() = state;
        self.log("profile_create", &format!("Created profile {name}"), None);
        Ok(id)
    }

    pub fn rename_profile(&self, id: &ProfileId, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::invalid("Profile name cannot be empty"));
        }
        let _writer = self.writer.lock();
        let mut state = self.state.read().clone();
        let i = profile_index(&state, id)?;
        state.profiles[i].name = name.to_owned();
        self.db
            .lock()
            .save_library(self.game_id.as_str(), &state, None)?;
        *self.state.write() = state;
        Ok(())
    }

    /// Switches profiles: applies its layout and its autoexec.
    pub fn activate_profile(
        &self,
        id: &ProfileId,
        progress: &dyn Progress,
    ) -> Result<ChangeReport> {
        let id = id.clone();
        let report = self.mutate("profile_switch", progress, |state| {
            let i = profile_index(state, &id)?;
            state.active_profile = id.clone();
            Ok(format!("Switched to {}", state.profiles[i].name))
        })?;
        let autoexec = self.with_state(|s| s.profile().autoexec.clone());
        if let Some(text) = autoexec {
            self.write_autoexec(&text)?;
        }
        Ok(report)
    }

    pub fn delete_profile(&self, id: &ProfileId) -> Result<ChangeReport> {
        let id = id.clone();
        self.mutate("profile_delete", &NoProgress, |state| {
            if state.profiles.len() <= 1 {
                return Err(AppError::new(
                    ErrorCode::LastProfile,
                    "You cannot delete the only profile",
                ));
            }
            let i = profile_index(state, &id)?;
            let removed = state.profiles.remove(i);
            if state.active_profile == id {
                state.active_profile = state.profiles[0].id.clone();
            }
            Ok(format!("Deleted profile {}", removed.name))
        })
    }

    /// Inserts or replaces a whole profile (imports). Applies the layout
    /// when it is (or becomes) the active profile.
    pub fn put_profile(
        &self,
        profile: Profile,
        activate: bool,
        progress: &dyn Progress,
    ) -> Result<ChangeReport> {
        let autoexec = profile.autoexec.clone();
        let report = self.mutate("profile_import", progress, move |state| {
            let name = profile.name.clone();
            let id = profile.id.clone();
            match state.profiles.iter_mut().find(|p| p.id == id) {
                Some(slot) => *slot = profile,
                None => state.profiles.push(profile),
            }
            let existing: Vec<ModId> = state.mods.keys().cloned().collect();
            let p = state
                .profiles
                .iter_mut()
                .find(|p| p.id == id)
                .ok_or_else(|| AppError::internal("profile vanished"))?;
            p.enabled.retain(|m| existing.contains(m));
            p.pinned.retain(|m| existing.contains(m));
            for m in &existing {
                p.ensure_ordered(m);
            }
            if activate {
                state.active_profile = id;
            }
            Ok(format!("Imported profile {name}"))
        })?;
        if activate && let Some(text) = autoexec {
            self.write_autoexec(&text)?;
        }
        Ok(report)
    }

    pub fn set_profile_launch_options(&self, options: Option<String>) -> Result<()> {
        self.edit_profile_meta(|p| p.launch_options = options.filter(|o| !o.trim().is_empty()))
    }

    /// Stores the autoexec text on the active profile and writes the file.
    pub fn set_profile_autoexec(&self, text: Option<String>) -> Result<()> {
        if let Some(t) = &text {
            self.write_autoexec(t)?;
        }
        self.edit_profile_meta(|p| p.autoexec = text)
    }

    pub fn set_ignored_conflict(&self, path: &str, ignored: bool) -> Result<()> {
        let key = path.replace('\\', "/").to_lowercase();
        self.edit_profile_meta(|p| {
            if ignored {
                p.ignored_conflicts.insert(key);
            } else {
                p.ignored_conflicts.remove(&key);
            }
        })
    }

    fn edit_profile_meta(&self, f: impl FnOnce(&mut Profile)) -> Result<()> {
        let _writer = self.writer.lock();
        let mut state = self.state.read().clone();
        f(state.profile_mut());
        self.db
            .lock()
            .save_library(self.game_id.as_str(), &state, None)?;
        *self.state.write() = state;
        Ok(())
    }

    // ------------------------------------------------------------- autoexec

    pub fn read_autoexec(&self) -> Option<String> {
        std::fs::read(self.paths.autoexec())
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    pub fn write_autoexec(&self, text: &str) -> Result<()> {
        let path = self.paths.autoexec();
        self.paths.check_scope(&path)?;
        crate::fsops::write_atomic(&path, text.as_bytes())
    }

    // --------------------------------------------------------------- groups

    pub fn create_group(&self, name: &str, members: &[ModId]) -> Result<GroupId> {
        let id = GroupId::new(format!("g{}", kombini_storage::random_hex(4)));
        let name = name.trim().to_owned();
        if name.is_empty() {
            return Err(AppError::invalid("Group name cannot be empty"));
        }
        let members = members.to_vec();
        let gid = id.clone();
        self.edit_profile_meta(move |p| {
            for g in &mut p.groups {
                g.mod_ids.retain(|m| !members.contains(m));
            }
            p.groups.push(Group {
                id: gid,
                name,
                mod_ids: members,
                collapsed: false,
            });
        })?;
        Ok(id)
    }

    pub fn update_group(
        &self,
        id: &GroupId,
        name: Option<String>,
        members: Option<Vec<ModId>>,
        collapsed: Option<bool>,
    ) -> Result<()> {
        let _writer = self.writer.lock();
        let mut state = self.state.read().clone();
        {
            let profile = state.profile_mut();
            if let Some(members) = &members {
                for g in profile.groups.iter_mut().filter(|g| &g.id != id) {
                    g.mod_ids.retain(|m| !members.contains(m));
                }
            }
            let g = group_mut(profile, id)?;
            if let Some(n) = name {
                g.name = n;
            }
            if let Some(m) = members {
                g.mod_ids = m;
            }
            if let Some(c) = collapsed {
                g.collapsed = c;
            }
        }
        self.db
            .lock()
            .save_library(self.game_id.as_str(), &state, None)?;
        *self.state.write() = state;
        Ok(())
    }

    pub fn delete_group(&self, id: &GroupId) -> Result<()> {
        let id = id.clone();
        self.edit_profile_meta(move |p| p.groups.retain(|g| g.id != id))
    }

    pub fn set_group_enabled(&self, id: &GroupId, enabled: bool) -> Result<ChangeReport> {
        let members = self.with_state(|s| {
            s.profile()
                .groups
                .iter()
                .find(|g| &g.id == id)
                .map(|g| g.mod_ids.clone())
        });
        let members = members.ok_or_else(|| AppError::invalid("Group not found"))?;
        self.set_enabled(&members, enabled)
    }

    // ------------------------------------------------------------ conflicts

    /// Game paths inside a VPK blob (cached).
    pub(crate) fn listing(&self, blob: &BlobHash) -> Option<Arc<Vec<String>>> {
        if let Some(hit) = self.listing_cache.lock().get(blob) {
            return Some(hit.clone());
        }
        let archive = kombini_vpk::VpkArchive::open(&self.blobs.path_for(blob)).ok()?;
        let list: Arc<Vec<String>> = Arc::new(
            archive
                .file_paths()
                .into_iter()
                .map(|p| p.to_lowercase())
                .collect(),
        );
        self.listing_cache.lock().insert(blob.clone(), list.clone());
        Some(list)
    }

    pub fn conflicts(&self) -> ConflictsView {
        let (ordered_roots, ignored) = self.with_state(|s| {
            let existing: BTreeSet<ModId> = s.mods.keys().cloned().collect();
            let order = effective_order(s.profile(), &existing);
            let roots: Vec<(ModId, Vec<BlobHash>)> = order
                .iter()
                .filter_map(|id| s.mods.get(id))
                .map(|m| {
                    (
                        m.id.clone(),
                        m.selected_roots().map(|f| f.blob.clone()).collect(),
                    )
                })
                .collect();
            (roots, s.profile().ignored_conflicts.clone())
        });
        let mut view = ConflictsView::default();
        let mut ordered: Vec<(ModId, Vec<String>)> = Vec::new();
        for (id, blobs) in ordered_roots {
            let mut paths = Vec::new();
            for blob in &blobs {
                match self.listing(blob) {
                    Some(list) => paths.extend(list.iter().cloned()),
                    None => {
                        if !view.unreadable.contains(&id) {
                            view.unreadable.push(id.clone());
                        }
                    }
                }
            }
            ordered.push((id, paths));
        }
        view.conflicts = find_conflicts(&ordered, &ignored);
        view.pairs = summarize_pairs(&view.conflicts);
        let mut lost: BTreeMap<&ModId, usize> = BTreeMap::new();
        for c in &view.conflicts {
            for loser in c.owners.iter().skip(1) {
                *lost.entry(loser).or_default() += 1;
            }
        }
        for (id, paths) in &ordered {
            let unique: BTreeSet<&String> = paths.iter().collect();
            if !unique.is_empty() && lost.get(id).copied().unwrap_or(0) >= unique.len() {
                view.fully_overridden.push(id.clone());
            }
        }
        view
    }
}
