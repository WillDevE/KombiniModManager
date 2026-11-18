//! Effective load order (ARCHITECTURE §4.1).

use crate::ids::ModId;
use crate::model::Profile;
use std::collections::BTreeSet;

/// Ordered enabled mods: pinned (in pin order), then the profile order, then
/// any enabled mods missing from the order, sorted by id. Only ids in
/// `existing` are returned; duplicates are dropped. Index 0 wins conflicts.
pub fn effective_order(profile: &Profile, existing: &BTreeSet<ModId>) -> Vec<ModId> {
    let mut out: Vec<ModId> = Vec::new();
    let mut seen: BTreeSet<&ModId> = BTreeSet::new();
    let usable = |id: &ModId| profile.enabled.contains(id) && existing.contains(id);
    for id in &profile.pinned {
        if usable(id) && seen.insert(id) {
            out.push(id.clone());
        }
    }
    for id in &profile.order {
        if usable(id) && seen.insert(id) {
            out.push(id.clone());
        }
    }
    for id in &profile.enabled {
        if existing.contains(id) && seen.insert(id) {
            out.push(id.clone());
        }
    }
    out
}

/// Moves `id` to `new_index` within `profile.order` (clamped). Used for drag
/// and drop; `new_index` counts positions in the full order list.
pub fn move_in_order(profile: &mut Profile, id: &ModId, new_index: usize) {
    profile.order.retain(|m| m != id);
    let index = new_index.min(profile.order.len());
    profile.order.insert(index, id.clone());
}

/// Replaces the order of the enabled mods with `enabled_order`, keeping the
/// relative positions of disabled mods. Unknown ids are ignored; enabled ids
/// absent from `enabled_order` keep their relative order after the given ones.
pub fn set_enabled_order(profile: &mut Profile, enabled_order: &[ModId]) {
    let enabled: BTreeSet<&ModId> = profile.enabled.iter().collect();
    let mut queue: Vec<ModId> = Vec::new();
    let mut seen = BTreeSet::new();
    for id in enabled_order {
        if enabled.contains(id) && seen.insert(id.clone()) {
            queue.push(id.clone());
        }
    }
    for id in &profile.order {
        if enabled.contains(id) && seen.insert(id.clone()) {
            queue.push(id.clone());
        }
    }
    let mut queue = queue.into_iter();
    let mut new_order = Vec::with_capacity(profile.order.len());
    for id in &profile.order {
        if enabled.contains(id) {
            if let Some(next) = queue.next() {
                new_order.push(next);
            }
        } else {
            new_order.push(id.clone());
        }
    }
    new_order.extend(queue);
    profile.order = new_order;
    // Pins follow the requested order too.
    let position = |id: &ModId| enabled_order.iter().position(|m| m == id);
    profile
        .pinned
        .sort_by_key(|id| position(id).unwrap_or(usize::MAX));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<ModId> {
        v.iter().map(|s| ModId::from(*s)).collect()
    }

    fn profile(order: &[&str], enabled: &[&str], pinned: &[&str]) -> Profile {
        let mut p = Profile::default_profile();
        p.order = ids(order);
        p.enabled = ids(enabled).into_iter().collect();
        p.pinned = ids(pinned);
        p
    }

    #[test]
    fn pinned_first_then_order_then_missing() {
        let p = profile(&["a", "b", "c", "d"], &["a", "c", "d", "z", "y"], &["d"]);
        let existing: BTreeSet<ModId> = ids(&["a", "b", "c", "d", "y"]).into_iter().collect();
        assert_eq!(effective_order(&p, &existing), ids(&["d", "a", "c", "y"]));
    }

    #[test]
    fn disabled_pins_are_skipped() {
        let p = profile(&["a", "b"], &["a"], &["b"]);
        let existing: BTreeSet<ModId> = ids(&["a", "b"]).into_iter().collect();
        assert_eq!(effective_order(&p, &existing), ids(&["a"]));
    }

    #[test]
    fn set_enabled_order_keeps_disabled_positions() {
        let mut p = profile(&["a", "x", "b", "c"], &["a", "b", "c"], &[]);
        set_enabled_order(&mut p, &ids(&["c", "a", "b"]));
        assert_eq!(p.order, ids(&["c", "x", "a", "b"]));
    }

    #[test]
    fn move_clamps() {
        let mut p = profile(&["a", "b", "c"], &[], &[]);
        move_in_order(&mut p, &ModId::from("a"), 99);
        assert_eq!(p.order, ids(&["b", "c", "a"]));
        move_in_order(&mut p, &ModId::from("a"), 0);
        assert_eq!(p.order, ids(&["a", "b", "c"]));
    }
}
