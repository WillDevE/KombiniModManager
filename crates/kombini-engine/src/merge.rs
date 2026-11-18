//! Merging several mods into one new mod (one VPK, one pak slot).
//!
//! Inputs are the selected roots of each mod, ordered so the mod that wins
//! today (higher in the load order) also wins inside the merged VPK. Blobs
//! are linked (or copied) into a scratch folder under their original
//! `*_dir.vpk` / `*_NNN.vpk` names so chunked VPKs resolve.

use crate::install::{InstallMeta, InstallReport, TempDir};
use crate::{Engine, Progress, Result};
use kombini_core::order::effective_order;
use kombini_core::{AppError, ErrorCode, ModId, ModSource};
use kombini_vpk::{CollisionPolicy, MergeOptions, merge_vpks};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize)]
pub struct MergeResult {
    pub install: InstallReport,
    pub entries: usize,
    pub overridden_paths: usize,
    pub disabled_sources: Vec<ModId>,
}

pub(crate) fn link_or_copy(from: &Path, to: &Path) -> Result<()> {
    if std::fs::hard_link(from, to).is_ok() {
        return Ok(());
    }
    crate::fsops::copy(from, to).map(|_| ())
}

impl Engine {
    /// Merges `ids` into a new mod called `title`, installs and enables it,
    /// and disables the sources when `disable_sources` is set.
    pub fn merge_mods(
        &self,
        ids: &[ModId],
        title: &str,
        disable_sources: bool,
        progress: &dyn Progress,
    ) -> Result<MergeResult> {
        if ids.len() < 2 {
            return Err(AppError::invalid("Select at least two mods to merge"));
        }
        let title = title.trim();
        if title.is_empty() {
            return Err(AppError::invalid("Give the merged mod a name"));
        }
        // Priority: effective order (enabled, index 0 wins), then the rest as given.
        let ordered: Vec<ModId> = self.with_state(|s| {
            let existing: BTreeSet<ModId> = s.mods.keys().cloned().collect();
            let wanted: BTreeSet<&ModId> = ids.iter().collect();
            let mut out: Vec<ModId> = effective_order(s.profile(), &existing)
                .into_iter()
                .filter(|id| wanted.contains(id))
                .collect();
            for id in ids {
                if !out.contains(id) {
                    out.push(id.clone());
                }
            }
            out
        });
        let tmp = TempDir::new(self)?;
        let mut inputs: Vec<PathBuf> = Vec::new();
        progress.stage("Collecting files");
        self.with_state(|s| -> Result<()> {
            for id in &ordered {
                let m = s.mods.get(id).ok_or_else(|| AppError::mod_not_found(id))?;
                for (r, root) in m.selected_roots().enumerate() {
                    let base = format!("in{:03}_{r:02}", inputs.len());
                    let dir = tmp.0.join(format!("{base}_dir.vpk"));
                    link_or_copy(&self.blobs().path_for(&root.blob), &dir)?;
                    for chunk in m.chunks_of(&root.rel_path) {
                        let idx = chunk.chunk_index.unwrap_or(0);
                        link_or_copy(
                            &self.blobs().path_for(&chunk.blob),
                            &tmp.0.join(format!("{base}_{idx:03}.vpk")),
                        )?;
                    }
                    inputs.push(dir);
                }
            }
            Ok(())
        })?;
        if inputs.is_empty() {
            return Err(AppError::invalid(
                "The selected mods have no selected files",
            ));
        }
        progress.stage("Merging VPKs");
        let out_dir = tmp.0.join("merged");
        std::fs::create_dir_all(&out_dir)
            .map_err(|e| AppError::from_io("Creating a temporary folder", &out_dir, &e))?;
        let output = out_dir.join("pak01_dir.vpk");
        // Lowest priority first + "last wins" = the top mod wins, and every
        // collision is reported in `overridden_paths`.
        inputs.reverse();
        let report = merge_vpks(
            &inputs,
            &output,
            &MergeOptions {
                policy: CollisionPolicy::Last,
                overrides: HashMap::new(),
                extra_files: Vec::new(),
            },
            &|| progress.cancelled(),
        )
        .map_err(|e| AppError::new(ErrorCode::InvalidVpk, format!("Merge failed: {e}")))?;
        if progress.cancelled() {
            return Err(AppError::cancelled());
        }
        let install = self.install_paths(
            &[output],
            InstallMeta {
                title: Some(title.to_owned()),
                source: Some(ModSource::Merged),
                description: Some(format!(
                    "Merged from: {}",
                    self.with_state(|s| ordered
                        .iter()
                        .filter_map(|id| s.mods.get(id).map(|m| m.title.clone()))
                        .collect::<Vec<_>>()
                        .join(", "))
                )),
                enable: Some(true),
                ..InstallMeta::default()
            },
            progress,
        )?;
        let disabled_sources = if disable_sources {
            self.set_enabled(&ordered, false)?;
            ordered
        } else {
            Vec::new()
        };
        Ok(MergeResult {
            install,
            entries: report.total_entries,
            overridden_paths: report.overridden_paths.len(),
            disabled_sources,
        })
    }
}
