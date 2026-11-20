//! Health checks with one-click fixes.

use crate::{ChangeReport, Engine, NoProgress, Progress, Result};
use kombini_core::{FilesState, FixAction, HealthIssue, Level, ModId};
use kombini_gameinfo::IssueCode;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct HealthReport {
    pub issues: Vec<HealthIssue>,
    pub worst: Option<Level>,
}

fn issue(
    level: Level,
    code: &str,
    message: String,
    mod_id: Option<ModId>,
    fix: Option<FixAction>,
) -> HealthIssue {
    HealthIssue {
        level,
        code: code.to_owned(),
        message,
        mod_id,
        fix,
    }
}

impl Engine {
    /// Runs every check. Cheap enough to call after each change (conflicts
    /// use the listing cache).
    pub fn health(&self) -> HealthReport {
        let mut issues = Vec::new();
        let any_enabled = self.mods_active()
            && self.with_state(|s| {
                s.mods
                    .values()
                    .any(|m| m.files.iter().any(|f| f.placement.is_mounted()))
            });

        let gi = self.gameinfo_status();
        for gi_issue in &gi.issues {
            let (level, fix) = match gi_issue.code {
                IssueCode::Missing | IssueCode::Unreadable | IssueCode::MissingSearchPaths => {
                    (Level::Error, None)
                }
                IssueCode::AddonsNotMounted
                | IssueCode::AddonsBelowBase
                | IssueCode::MissingModWrite => {
                    if !any_enabled {
                        continue;
                    }
                    (Level::Error, Some(FixAction::ReapplyGameinfo))
                }
                IssueCode::MarkersCorrupt => (Level::Error, Some(FixAction::RepairGameinfoMarkers)),
                IssueCode::ReadOnly => (Level::Warning, Some(FixAction::ClearReadOnly)),
                IssueCode::OtherManager | IssueCode::DuplicateAddonMounts => (Level::Warning, None),
                IssueCode::ObsoleteGameLanguage => (Level::Info, None),
            };
            issues.push(issue(
                level,
                &format!(
                    "gameinfo_{}",
                    serde_json::to_value(gi_issue.code)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_default()
                ),
                gi_issue.message.clone(),
                None,
                fix,
            ));
        }
        if any_enabled && gi.patched {
            let needed = self.needed_shards();
            let mounted = gi
                .analysis
                .as_ref()
                .map(|a| a.addon_paths.len())
                .unwrap_or(0);
            if mounted < usize::from(needed) {
                issues.push(issue(
                    Level::Error,
                    "gameinfo_incomplete",
                    format!(
                        "gameinfo.gi mounts {mounted} addon folder(s) but {needed} are in use; some mods will not load."
                    ),
                    None,
                    Some(FixAction::ReapplyGameinfo),
                ));
            }
        }

        let mods = self.with_state(|s| {
            let enabled = s.profile().enabled.clone();
            s.mods
                .values()
                .map(|m| {
                    (
                        m.id.clone(),
                        m.title.clone(),
                        m.files_state(),
                        m.files.iter().all(|f| self.blobs.contains(&f.blob)),
                        enabled.contains(&m.id),
                        m.safety.clone(),
                    )
                })
                .collect::<Vec<_>>()
        });
        for (id, title, state, restorable, enabled, safety) in mods {
            if state != FilesState::Ok {
                let fix = if restorable {
                    FixAction::RestoreFiles { mod_id: id.clone() }
                } else {
                    FixAction::Reinstall { mod_id: id.clone() }
                };
                issues.push(issue(
                    if enabled {
                        Level::Error
                    } else {
                        Level::Warning
                    },
                    "missing_files",
                    format!(
                        "{title}: {} on disk.",
                        if state == FilesState::Missing {
                            "all files are missing"
                        } else {
                            "some files are missing"
                        }
                    ),
                    Some(id.clone()),
                    Some(fix),
                ));
            }
            if enabled {
                for f in safety.iter().filter(|f| f.level == Level::Error) {
                    issues.push(issue(
                        Level::Warning,
                        "unsafe_content",
                        format!("{title}: {}", f.message),
                        Some(id.clone()),
                        None,
                    ));
                }
            }
        }

        for signal in self.other_managers() {
            issues.push(issue(Level::Warning, "other_manager", format!("{signal}. Using two mod managers at once can make mods load in the wrong order."), None, None));
        }
        let foreign = self.foreign_files();
        if !foreign.is_empty() {
            issues.push(issue(
                Level::Info,
                "unmanaged_files",
                format!("{} mod file(s) in the addons folders are not managed. Import them to control their order.", foreign.len()),
                None,
                Some(FixAction::OpenImport),
            ));
        }
        let conflicts = self.conflicts();
        if !conflicts.fully_overridden.is_empty() {
            let names = self.with_state(|s| {
                conflicts
                    .fully_overridden
                    .iter()
                    .map(|id| {
                        s.mods
                            .get(id)
                            .map(|m| m.title.clone())
                            .unwrap_or_else(|| id.to_string())
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            });
            issues.push(issue(
                Level::Warning,
                "fully_overridden",
                format!("These mods are completely overridden by mods above them: {names}"),
                None,
                Some(FixAction::OpenConflicts),
            ));
        } else if !conflicts.conflicts.is_empty() {
            issues.push(issue(
                Level::Info,
                "conflicts",
                format!(
                    "{} file(s) are provided by more than one enabled mod; the higher mod wins.",
                    conflicts.conflicts.len()
                ),
                None,
                Some(FixAction::OpenConflicts),
            ));
        }
        let worst = issues.iter().map(|i| i.level).min();
        HealthReport { issues, worst }
    }

    /// True when enabled mods are not fully in the game: files missing from
    /// the addons folders (and restorable), or gameinfo.gi not mounting them.
    /// Always false while mods are switched off.
    pub fn needs_repair(&self) -> bool {
        self.mods_active()
            && self.health().issues.iter().any(|i| match &i.fix {
                Some(FixAction::ReapplyGameinfo) => true,
                Some(FixAction::RestoreFiles { .. }) => i.level == Level::Error,
                _ => false,
            })
    }

    /// Keeps the game modded without a manual "reapply": re-reads the addons
    /// folders and, if something is off ([`Engine::needs_repair`]), puts it
    /// back. Does nothing while mods are off, the game runs or Steam updates
    /// it. Returns what was fixed, or `None` when all was in place.
    pub fn repair_if_needed(&self, progress: &dyn Progress) -> Result<Option<ChangeReport>> {
        if !self.mods_active()
            || self.process.is_running()
            || kombini_steam::read_build_info(&self.paths.root).update_in_progress
        {
            return Ok(None);
        }
        self.rescan()?;
        if !self.needs_repair() {
            return Ok(None);
        }
        let mut report = self.restore_files(progress)?;
        // Restoring places files and patches gameinfo when anything mounts;
        // make sure gameinfo is right even when no file had to move.
        if self.ensure_gameinfo()?.changed {
            report.gameinfo_changed = true;
        }
        report.summary = match (report.placed + report.moved, report.gameinfo_changed) {
            (0, true) => "Repaired gameinfo.gi so your mods load".to_owned(),
            (n, true) => format!("Put {n} mod file(s) back and repaired gameinfo.gi"),
            (n, false) => format!("Put {n} missing mod file(s) back"),
        };
        self.log("repair", &report.summary, None);
        Ok(Some(report))
    }

    /// Executes a fix suggested by [`Engine::health`].
    pub fn apply_fix(&self, fix: &FixAction) -> Result<String> {
        match fix {
            FixAction::ReapplyGameinfo => {
                self.ensure_gameinfo()?;
                Ok("gameinfo.gi updated".into())
            }
            FixAction::RepairGameinfoMarkers => {
                self.repair_gameinfo_markers()?;
                Ok("gameinfo.gi markers repaired".into())
            }
            FixAction::ClearReadOnly => {
                crate::fsops::clear_read_only(&self.paths.gameinfo()).map_err(|e| {
                    kombini_core::AppError::from_io(
                        "Clearing read-only",
                        &self.paths.gameinfo(),
                        &e,
                    )
                })?;
                Ok("gameinfo.gi is writable again".into())
            }
            FixAction::RestoreFiles { .. } => {
                let r = self.restore_files(&NoProgress)?;
                Ok(format!("Restored {} file(s)", r.placed))
            }
            FixAction::RemoveMod { mod_id } => {
                self.uninstall(std::slice::from_ref(mod_id))?;
                Ok("Mod removed".into())
            }
            FixAction::Reinstall { .. }
            | FixAction::OpenConflicts
            | FixAction::OpenImport
            | FixAction::CheckUpdates => Err(kombini_core::AppError::invalid(
                "This fix is handled by the app (it opens a view or downloads)",
            )),
        }
    }
}
