//! Pak-slot assignment across addon shards (ARCHITECTURE §4.2).
//!
//! Pure and deterministic: given every installed mod, the ordered list of
//! enabled mods, and the slots reserved by foreign files, compute where every
//! managed file must live. The engine diffs this against reality and applies
//! it transactionally.

use crate::ids::ModId;
use crate::model::{Mod, Placement};
use crate::naming::{disabled_names, enabled_chunk_name, enabled_root_name};
use crate::{MAX_SHARDS, SLOTS_PER_SHARD};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    #[error(
        "{needed} mod files need a load-order slot but only {capacity} are available across all \
         addon folders; disable some mods"
    )]
    TooManyRoots { needed: usize, capacity: usize },
    #[error("mod {0} is enabled but has no selected files")]
    NothingSelected(ModId),
    #[error("mod {mod_id}: chunk {rel_path} has no chunk index or root")]
    BadChunk { mod_id: ModId, rel_path: String },
}

impl From<LayoutError> for crate::AppError {
    fn from(err: LayoutError) -> Self {
        let code = match err {
            LayoutError::TooManyRoots { .. } => crate::ErrorCode::LayoutTooManyRoots,
            LayoutError::NothingSelected(_) => crate::ErrorCode::InvalidInput,
            LayoutError::BadChunk { .. } => crate::ErrorCode::Internal,
        };
        crate::AppError::new(code, err.to_string())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout {
    /// (mod id, rel_path) → target placement (never `Missing`).
    pub placements: BTreeMap<(ModId, String), Placement>,
    /// Highest shard used by a mounted root (1 when nothing is mounted).
    pub max_shard: u8,
    pub mounted_roots: usize,
    /// Mounted slot per (mod, root rel_path), in load order.
    pub slots: Vec<(ModId, String, u8, u8)>,
}

impl Layout {
    pub fn placement(&self, mod_id: &ModId, rel_path: &str) -> Option<&Placement> {
        self.placements.get(&(mod_id.clone(), rel_path.to_owned()))
    }
}

struct Cursor<'a> {
    shard: u8,
    slot: u8,
    foreign: &'a BTreeSet<(u8, u8)>,
}

impl Cursor<'_> {
    /// Returns the next free (shard, slot) and advances past it.
    fn take(&mut self) -> Option<(u8, u8)> {
        loop {
            if self.shard > MAX_SHARDS {
                return None;
            }
            if self.slot > SLOTS_PER_SHARD {
                self.shard += 1;
                self.slot = 1;
                continue;
            }
            let here = (self.shard, self.slot);
            self.slot += 1;
            if !self.foreign.contains(&here) {
                return Some(here);
            }
        }
    }
}

/// Computes the target layout.
///
/// * `mods` — every installed mod (order irrelevant).
/// * `enabled_order` — effective load order (index 0 wins); ids not in `mods` are ignored.
/// * `foreign_slots` — `(shard, slot)` pairs occupied by files we do not manage.
pub fn assign_layout(
    mods: &[&Mod],
    enabled_order: &[ModId],
    foreign_slots: &BTreeSet<(u8, u8)>,
) -> Result<Layout, LayoutError> {
    let by_id: BTreeMap<&ModId, &Mod> = mods.iter().map(|m| (&m.id, *m)).collect();
    let mut layout = Layout {
        max_shard: 1,
        ..Layout::default()
    };
    let capacity = usize::from(MAX_SHARDS) * usize::from(SLOTS_PER_SHARD) - foreign_slots.len();
    let needed: usize = enabled_order
        .iter()
        .filter_map(|id| by_id.get(id))
        .map(|m| m.selected_roots().count())
        .sum();
    if needed > capacity {
        return Err(LayoutError::TooManyRoots { needed, capacity });
    }

    let mut cursor = Cursor {
        shard: 1,
        slot: 1,
        foreign: foreign_slots,
    };
    let mut mounted: BTreeSet<(ModId, String)> = BTreeSet::new();
    let mut seen_enabled: BTreeSet<&ModId> = BTreeSet::new();

    for id in enabled_order {
        let Some(m) = by_id.get(id) else { continue };
        if !seen_enabled.insert(id) {
            continue;
        }
        let roots: Vec<_> = m.selected_roots().collect();
        if roots.is_empty() {
            return Err(LayoutError::NothingSelected(id.clone()));
        }
        for root in roots {
            let (shard, slot) = cursor
                .take()
                .ok_or(LayoutError::TooManyRoots { needed, capacity })?;
            layout.placements.insert(
                (id.clone(), root.rel_path.clone()),
                Placement::Mounted {
                    shard,
                    file_name: enabled_root_name(slot),
                },
            );
            mounted.insert((id.clone(), root.rel_path.clone()));
            for chunk in m.chunks_of(&root.rel_path) {
                let index = chunk.chunk_index.ok_or_else(|| LayoutError::BadChunk {
                    mod_id: id.clone(),
                    rel_path: chunk.rel_path.clone(),
                })?;
                layout.placements.insert(
                    (id.clone(), chunk.rel_path.clone()),
                    Placement::Mounted {
                        shard,
                        file_name: enabled_chunk_name(slot, index),
                    },
                );
                mounted.insert((id.clone(), chunk.rel_path.clone()));
            }
            layout.max_shard = layout.max_shard.max(shard);
            layout.mounted_roots += 1;
            layout
                .slots
                .push((id.clone(), root.rel_path.clone(), shard, slot));
        }
    }

    // Everything else is parked under its disabled name in shard 1.
    for m in mods {
        let names = disabled_names(&m.id, m.files.iter().map(|f| f.rel_path.as_str()));
        for file in &m.files {
            let key = (m.id.clone(), file.rel_path.clone());
            if mounted.contains(&key) {
                continue;
            }
            if file.is_chunk && file.parent.is_none() {
                return Err(LayoutError::BadChunk {
                    mod_id: m.id.clone(),
                    rel_path: file.rel_path.clone(),
                });
            }
            let file_name = names
                .get(&file.rel_path)
                .cloned()
                .unwrap_or_else(|| crate::naming::disabled_name(&m.id, &file.rel_path));
            layout
                .placements
                .insert(key, Placement::Disabled { file_name });
        }
    }
    Ok(layout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::BlobHash;
    use crate::model::{ModFile, ModSource};

    fn root(rel: &str, selected: bool) -> ModFile {
        ModFile {
            rel_path: rel.into(),
            original_name: rel.rsplit('/').next().unwrap_or(rel).into(),
            blob: BlobHash::from("00"),
            size: 1,
            is_chunk: false,
            parent: None,
            chunk_index: None,
            selected,
            placement: Placement::Missing,
        }
    }

    fn chunk(rel: &str, parent: &str, index: u16) -> ModFile {
        ModFile {
            is_chunk: true,
            parent: Some(parent.into()),
            chunk_index: Some(index),
            ..root(rel, true)
        }
    }

    fn simple(id: &str) -> Mod {
        let mut m = Mod::new(ModId::from(id), id, ModSource::Local, 0);
        m.files = vec![root("pak01_dir.vpk", true)];
        m
    }

    fn mounted(layout: &Layout, id: &str, rel: &str) -> (u8, String) {
        match layout.placement(&ModId::from(id), rel).unwrap() {
            Placement::Mounted { shard, file_name } => (*shard, file_name.clone()),
            other => panic!("not mounted: {other:?}"),
        }
    }

    #[test]
    fn single_mod() {
        let a = simple("gb1");
        let layout = assign_layout(&[&a], &[ModId::from("gb1")], &BTreeSet::new()).unwrap();
        assert_eq!(
            mounted(&layout, "gb1", "pak01_dir.vpk"),
            (1, "pak01_dir.vpk".into())
        );
        assert_eq!(layout.max_shard, 1);
    }

    #[test]
    fn disabled_mods_are_parked() {
        let a = simple("gb1");
        let b = simple("gb2");
        let layout = assign_layout(&[&a, &b], &[ModId::from("gb2")], &BTreeSet::new()).unwrap();
        assert_eq!(
            mounted(&layout, "gb2", "pak01_dir.vpk"),
            (1, "pak01_dir.vpk".into())
        );
        assert_eq!(
            layout.placement(&ModId::from("gb1"), "pak01_dir.vpk"),
            Some(&Placement::Disabled {
                file_name: "gb1_pak01_dir.vpk".into()
            })
        );
    }

    #[test]
    fn chunks_follow_root_and_unselected_roots_park() {
        let mut m = Mod::new(ModId::from("gb9"), "x", ModSource::Local, 0);
        m.files = vec![
            root("Blue/pak01_dir.vpk", true),
            chunk("Blue/pak01_000.vpk", "Blue/pak01_dir.vpk", 0),
            chunk("Blue/pak01_001.vpk", "Blue/pak01_dir.vpk", 1),
            root("Red/pak01_dir.vpk", false),
            chunk("Red/pak01_000.vpk", "Red/pak01_dir.vpk", 0),
        ];
        let other = simple("gb1");
        let order = [ModId::from("gb1"), ModId::from("gb9")];
        let layout = assign_layout(&[&m, &other], &order, &BTreeSet::new()).unwrap();
        assert_eq!(
            mounted(&layout, "gb9", "Blue/pak01_dir.vpk"),
            (1, "pak02_dir.vpk".into())
        );
        assert_eq!(
            mounted(&layout, "gb9", "Blue/pak01_001.vpk"),
            (1, "pak02_001.vpk".into())
        );
        assert_eq!(
            layout.placement(&ModId::from("gb9"), "Red/pak01_000.vpk"),
            Some(&Placement::Disabled {
                file_name: "gb9_Red__pak01_000.vpk".into()
            })
        );
    }

    #[test]
    fn foreign_slots_are_skipped() {
        let a = simple("gb1");
        let b = simple("gb2");
        let foreign: BTreeSet<(u8, u8)> = [(1, 1), (1, 3)].into_iter().collect();
        let order = [ModId::from("gb1"), ModId::from("gb2")];
        let layout = assign_layout(&[&a, &b], &order, &foreign).unwrap();
        assert_eq!(mounted(&layout, "gb1", "pak01_dir.vpk").1, "pak02_dir.vpk");
        assert_eq!(mounted(&layout, "gb2", "pak01_dir.vpk").1, "pak04_dir.vpk");
    }

    fn many(n: usize) -> Vec<Mod> {
        (0..n).map(|i| simple(&format!("gb{i}"))).collect()
    }

    #[test]
    fn boundaries_99_100_990() {
        for (n, last_shard, last_slot) in [(99usize, 1u8, 99u8), (100, 2, 1), (990, 10, 99)] {
            let mods = many(n);
            let refs: Vec<&Mod> = mods.iter().collect();
            let order: Vec<ModId> = mods.iter().map(|m| m.id.clone()).collect();
            let layout = assign_layout(&refs, &order, &BTreeSet::new()).unwrap();
            let last = layout.slots.last().unwrap();
            assert_eq!((last.2, last.3), (last_shard, last_slot), "n={n}");
            assert_eq!(layout.max_shard, last_shard);
            assert_eq!(layout.mounted_roots, n);
        }
    }

    #[test]
    fn overflow_is_an_error() {
        let mods = many(991);
        let refs: Vec<&Mod> = mods.iter().collect();
        let order: Vec<ModId> = mods.iter().map(|m| m.id.clone()).collect();
        let err = assign_layout(&refs, &order, &BTreeSet::new()).unwrap_err();
        assert_eq!(
            err,
            LayoutError::TooManyRoots {
                needed: 991,
                capacity: 990
            }
        );
    }

    #[test]
    fn enabled_with_nothing_selected_is_an_error() {
        let mut m = simple("gb1");
        m.files[0].selected = false;
        let err = assign_layout(&[&m], &[ModId::from("gb1")], &BTreeSet::new()).unwrap_err();
        assert_eq!(err, LayoutError::NothingSelected(ModId::from("gb1")));
    }

    #[test]
    fn deterministic_and_no_duplicate_targets() {
        let mods = many(250);
        let refs: Vec<&Mod> = mods.iter().collect();
        let order: Vec<ModId> = mods.iter().rev().map(|m| m.id.clone()).collect();
        let a = assign_layout(&refs, &order, &BTreeSet::new()).unwrap();
        let b = assign_layout(&refs, &order, &BTreeSet::new()).unwrap();
        assert_eq!(a, b);
        let mut targets = BTreeSet::new();
        for placement in a.placements.values() {
            let (shard, name) = placement.location().unwrap();
            assert!(targets.insert((shard, name.to_ascii_lowercase())));
        }
    }
}
