//! Deadlock addon file naming rules (ARCHITECTURE §3, §13.4, §13.5).

use crate::ids::ModId;
use std::collections::BTreeMap;

/// Shard folder name relative to `game/citadel`: `addons`, `addons2`..`addons10`.
pub fn shard_dir_name(shard: u8) -> String {
    if shard <= 1 {
        "addons".to_owned()
    } else {
        format!("addons{shard}")
    }
}

/// Parses a shard folder name back to its number (1..=10).
pub fn parse_shard_dir(name: &str) -> Option<u8> {
    let lower = name.to_ascii_lowercase();
    let rest = lower.strip_prefix("addons")?;
    if rest.is_empty() {
        return Some(1);
    }
    if rest.starts_with('0') {
        return None;
    }
    match rest.parse::<u8>() {
        Ok(n @ 2..=10) => Some(n),
        _ => None,
    }
}

/// `pak05_dir.vpk`
pub fn enabled_root_name(slot: u8) -> String {
    format!("pak{slot:02}_dir.vpk")
}

/// `pak05_003.vpk`
pub fn enabled_chunk_name(slot: u8, index: u16) -> String {
    format!("pak{slot:02}_{index:03}.vpk")
}

/// Slot of a mounted root name. Accepts any digit count (`pak1_dir.vpk`,
/// `pak001_dir.vpk`) because the game may treat them as the same slot; returns
/// `Some` only for values 1..=99. Case-insensitive.
pub fn parse_enabled_root(file_name: &str) -> Option<u8> {
    let lower = file_name.to_ascii_lowercase();
    let digits = lower.strip_prefix("pak")?.strip_suffix("_dir.vpk")?;
    if digits.is_empty() || digits.len() > 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match digits.parse::<u32>() {
        Ok(n @ 1..=99) => u8::try_from(n).ok(),
        _ => None,
    }
}

/// Exactly `pakNN_dir.vpk` with NN 01..=99 (what the manager writes).
pub fn is_canonical_enabled_root(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    lower.len() == "pak00_dir.vpk".len()
        && parse_enabled_root(&lower).is_some()
        && lower.as_bytes()[3].is_ascii_digit()
        && lower.as_bytes()[4].is_ascii_digit()
}

/// `(slot, chunk index)` of a mounted chunk name like `pak05_003.vpk`.
pub fn parse_enabled_chunk(file_name: &str) -> Option<(u8, u16)> {
    let (base, index) = parse_chunk_name(file_name)?;
    let lower = base.to_ascii_lowercase();
    let digits = lower.strip_prefix("pak")?;
    if digits.len() != 2 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match digits.parse::<u8>() {
        Ok(n @ 1..=99) => Some((n, index)),
        _ => None,
    }
}

/// `<base>_<NNN>.vpk` → (base, index). Case-insensitive extension.
pub fn parse_chunk_name(file_name: &str) -> Option<(String, u16)> {
    let lower = file_name.to_ascii_lowercase();
    if !lower.ends_with(".vpk") {
        return None;
    }
    let stem = &file_name[..file_name.len() - 4];
    let (base, idx) = stem.rsplit_once('_')?;
    if base.is_empty() || idx.len() != 3 || !idx.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    idx.parse().ok().map(|i| (base.to_owned(), i))
}

/// Root base: strip `.vpk` then a trailing `_dir` (both case-insensitive).
pub fn root_base(file_name: &str) -> String {
    let mut base = file_name;
    if base.to_ascii_lowercase().ends_with(".vpk") {
        base = &base[..base.len() - 4];
    }
    if base.to_ascii_lowercase().ends_with("_dir") {
        base = &base[..base.len() - 4];
    }
    base.to_owned()
}

/// Replaces characters outside `[A-Za-z0-9._-]` with `_`.
pub fn sanitize_component(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Parked (disabled) file name: `<mod_id>_<rel_path with '/' → '__', sanitized>`.
pub fn disabled_name(mod_id: &ModId, rel_path: &str) -> String {
    let flattened = rel_path.replace('/', "__");
    format!("{}_{}", mod_id.as_str(), sanitize_component(&flattened))
}

/// Disabled names for all of a mod's files, de-duplicated: when two rel paths
/// sanitize to the same name, later ones get `~2`, `~3`, ... before `.vpk`.
pub fn disabled_names<'a>(
    mod_id: &ModId,
    rel_paths: impl IntoIterator<Item = &'a str>,
) -> BTreeMap<String, String> {
    let mut used: BTreeMap<String, usize> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for rel in rel_paths {
        let base = disabled_name(mod_id, rel);
        let key = base.to_ascii_lowercase();
        let count = used.entry(key).or_insert(0);
        *count += 1;
        let name = if *count == 1 {
            base
        } else {
            let stem = base.strip_suffix(".vpk").unwrap_or(&base);
            format!("{stem}~{count}.vpk")
        };
        out.insert(rel.to_owned(), name);
    }
    out
}

/// If a file name looks like one of our parked files, returns the mod id prefix
/// candidate (text before the first `_` after `gb<digits>`/`local_<hex>`/`merged_<digits>`).
pub fn parked_mod_id(file_name: &str) -> Option<ModId> {
    if parse_enabled_root(file_name).is_some() || parse_enabled_chunk(file_name).is_some() {
        return None;
    }
    let lower = file_name.to_ascii_lowercase();
    if !lower.ends_with(".vpk") {
        return None;
    }
    let take_id = |prefix: &str, valid: fn(u8) -> bool| -> Option<ModId> {
        let rest = file_name.get(prefix.len()..)?;
        let end = rest.find('_')?;
        let body = &rest[..end];
        if body.is_empty() || !body.bytes().all(valid) {
            return None;
        }
        Some(ModId::new(format!("{prefix}{body}")))
    };
    if lower.starts_with("gb") {
        return take_id("gb", |b| b.is_ascii_digit());
    }
    if lower.starts_with("local_") {
        return take_id("local_", |b| b.is_ascii_hexdigit());
    }
    if lower.starts_with("merged_") {
        return take_id("merged_", |b| b.is_ascii_digit());
    }
    None
}

/// Default root selection for a newly installed mod (§13.4.6): when several
/// roots share the same file name in different directories they are variants
/// and only the first (by rel_path, case-insensitive) is selected; all other
/// roots are selected. Returns rel_path → selected.
pub fn default_root_selection<'a>(
    roots: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> BTreeMap<String, bool> {
    let mut by_name: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for (rel_path, original_name) in roots {
        by_name
            .entry(original_name.to_ascii_lowercase())
            .or_default()
            .push(rel_path);
    }
    let mut out = BTreeMap::new();
    for (_, mut rels) in by_name {
        rels.sort_by_key(|r| r.to_ascii_lowercase());
        for (i, rel) in rels.iter().enumerate() {
            out.insert((*rel).to_owned(), i == 0);
        }
    }
    out
}

/// Variant group label for a root: its parent directory (`Blue` for
/// `Blue/pak01_dir.vpk`), or None at the top level.
pub fn variant_label(rel_path: &str) -> Option<&str> {
    rel_path.rsplit_once('/').map(|(dir, _)| dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shard_names() {
        assert_eq!(shard_dir_name(1), "addons");
        assert_eq!(shard_dir_name(10), "addons10");
        assert_eq!(parse_shard_dir("addons"), Some(1));
        assert_eq!(parse_shard_dir("Addons7"), Some(7));
        assert_eq!(parse_shard_dir("addons1"), None);
        assert_eq!(parse_shard_dir("addons11"), None);
        assert_eq!(parse_shard_dir("addons02"), None);
        assert_eq!(parse_shard_dir("addons_backup"), None);
    }

    #[test]
    fn enabled_roots() {
        assert_eq!(parse_enabled_root("pak01_dir.vpk"), Some(1));
        assert_eq!(parse_enabled_root("PAK07_DIR.VPK"), Some(7));
        assert_eq!(parse_enabled_root("pak1_dir.vpk"), Some(1));
        assert_eq!(parse_enabled_root("pak001_dir.vpk"), Some(1));
        assert_eq!(parse_enabled_root("pak00_dir.vpk"), None);
        assert_eq!(parse_enabled_root("pak100_dir.vpk"), None);
        assert_eq!(parse_enabled_root("cool_dir.vpk"), None);
        assert!(is_canonical_enabled_root("pak99_dir.vpk"));
        assert!(!is_canonical_enabled_root("pak1_dir.vpk"));
        assert_eq!(enabled_root_name(3), "pak03_dir.vpk");
        assert_eq!(enabled_chunk_name(3, 12), "pak03_012.vpk");
        assert_eq!(parse_enabled_chunk("pak03_012.vpk"), Some((3, 12)));
        assert_eq!(parse_enabled_chunk("skin_000.vpk"), None);
    }

    #[test]
    fn chunks_and_bases() {
        assert_eq!(parse_chunk_name("skin_000.vpk"), Some(("skin".into(), 0)));
        assert_eq!(
            parse_chunk_name("my_skin_012.VPK"),
            Some(("my_skin".into(), 12))
        );
        assert_eq!(parse_chunk_name("skin_dir.vpk"), None);
        assert_eq!(parse_chunk_name("skin_0000.vpk"), None);
        assert_eq!(root_base("Skin_DIR.vpk"), "Skin");
        assert_eq!(root_base("cool.vpk"), "cool");
    }

    #[test]
    fn disabled_naming() {
        let id = ModId::from("gb123");
        assert_eq!(disabled_name(&id, "pak01_dir.vpk"), "gb123_pak01_dir.vpk");
        assert_eq!(
            disabled_name(&id, "Blue Skin/pak01_dir.vpk"),
            "gb123_Blue_Skin__pak01_dir.vpk"
        );
        let names = disabled_names(&id, ["a b/x.vpk", "a_b/x.vpk"]);
        assert_eq!(names["a b/x.vpk"], "gb123_a_b__x.vpk");
        assert_eq!(names["a_b/x.vpk"], "gb123_a_b__x~2.vpk");
    }

    #[test]
    fn parked_ids() {
        assert_eq!(
            parked_mod_id("gb123_pak01_dir.vpk"),
            Some(ModId::from("gb123"))
        );
        assert_eq!(
            parked_mod_id("local_0a1b2c_skin.vpk"),
            Some(ModId::from("local_0a1b2c"))
        );
        assert_eq!(parked_mod_id("pak01_dir.vpk"), None);
        assert_eq!(parked_mod_id("gbx_skin.vpk"), None);
        assert_eq!(parked_mod_id("cool_skin.vpk"), None);
    }

    #[test]
    fn variant_selection() {
        let sel = default_root_selection([
            ("Red/pak01_dir.vpk", "pak01_dir.vpk"),
            ("Blue/pak01_dir.vpk", "pak01_dir.vpk"),
            ("extras_dir.vpk", "extras_dir.vpk"),
        ]);
        assert!(sel["Blue/pak01_dir.vpk"]);
        assert!(!sel["Red/pak01_dir.vpk"]);
        assert!(sel["extras_dir.vpk"]);
        assert_eq!(variant_label("Blue/pak01_dir.vpk"), Some("Blue"));
        assert_eq!(variant_label("x.vpk"), None);
    }
}
