//! Hero detection from VPK paths and GameBanana metadata.
//!
//! The codename table is data (`data/heroes.json`) so it can be updated
//! without touching code; `HeroTable::from_json` accepts a newer table.

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

const BUILTIN: &str = include_str!("../data/heroes.json");

const PATH_HINTS: &[&str] = &[
    "models/heroes",
    "materials/models/heroes",
    "panorama/images/heroes/",
    "panorama/images/hud/abilities/",
    "particles/abilities/",
    "soundevents/hero/",
    "sounds/hero",
    "sounds/vo/",
    "animgraphs/animgraph2/hero/",
];

const STRIP_EXTENSIONS: &[&str] = &[
    ".vmdl_c",
    ".vnmskel_c",
    ".vtex_c",
    ".vsndevts_c",
    ".vpcf_c",
    ".vmat_c",
    ".vmdl",
    ".vnmskel",
    ".vtex",
    ".vsndevts",
    ".vpcf",
    ".vmat",
];

#[derive(Debug, Deserialize)]
struct RawTable {
    heroes: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct HeroTable {
    names: Vec<String>,
    token_to_hero: BTreeMap<String, String>,
}

impl HeroTable {
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let raw: RawTable = serde_json::from_str(json)?;
        let mut token_to_hero = BTreeMap::new();
        for (display, aliases) in &raw.heroes {
            let lower = display.to_lowercase();
            token_to_hero.insert(lower.replace(' ', "_"), display.clone());
            token_to_hero.insert(lower.clone(), display.clone());
            token_to_hero.insert(slug(&lower), display.clone());
            for alias in aliases {
                token_to_hero.insert(alias.to_lowercase(), display.clone());
            }
        }
        let mut names: Vec<String> = raw.heroes.keys().cloned().collect();
        names.sort_by_key(|n| n.to_lowercase());
        Ok(Self {
            names,
            token_to_hero,
        })
    }

    /// The table compiled into the app.
    pub fn builtin() -> &'static HeroTable {
        static TABLE: OnceLock<HeroTable> = OnceLock::new();
        TABLE.get_or_init(|| {
            // The embedded JSON is validated by the `builtin_table_parses` test.
            HeroTable::from_json(BUILTIN).unwrap_or(HeroTable {
                names: Vec::new(),
                token_to_hero: BTreeMap::new(),
            })
        })
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn hero_from_token(&self, token: &str) -> Option<&str> {
        let token = normalize_token(token);
        if token.is_empty() {
            return None;
        }
        if let Some(hero) = self.token_to_hero.get(&token) {
            return Some(hero);
        }
        let base = strip_version_suffix(&token);
        self.token_to_hero.get(base).map(String::as_str)
    }

    /// Hero from a GameBanana category/subcategory name ("Lady Geist", "Mo & Krill").
    pub fn hero_from_name(&self, value: &str) -> Option<&str> {
        let token = slug(&value.to_lowercase());
        self.token_to_hero.get(&token).map(String::as_str)
    }

    /// Hero display names referenced by a mod's VPK paths, sorted.
    pub fn detect<'a>(&self, paths: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        let mut found: BTreeSet<String> = BTreeSet::new();
        for raw in paths {
            let path = raw.replace('\\', "/");
            let lowered = path.to_lowercase();
            if !PATH_HINTS.iter().any(|hint| lowered.contains(hint)) {
                continue;
            }
            if let Some(dir) = staging_dir(&lowered)
                && let Some(hero) = self.hero_from_token(dir)
            {
                found.insert(hero.to_owned());
                continue;
            }
            for part in path.split('/').filter(|p| !p.is_empty()) {
                if let Some(hero) = self.hero_from_token(part) {
                    found.insert(hero.to_owned());
                    break;
                }
            }
        }
        let mut out: Vec<String> = found.into_iter().collect();
        out.sort_by_key(|n| n.to_lowercase());
        out
    }
}

/// `heroes_wip/<x>` or `heroes_staging/<x>` → `<x>`.
fn staging_dir(lowered: &str) -> Option<&str> {
    for marker in ["heroes_wip/", "heroes_staging/"] {
        if let Some(pos) = lowered.find(marker) {
            let rest = &lowered[pos + marker.len()..];
            let end = rest.find('/').unwrap_or(rest.len());
            if end > 0 {
                return Some(&rest[..end]);
            }
        }
    }
    None
}

fn slug(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut last_underscore = true;
    for c in value.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_underscore = false;
        } else if !last_underscore {
            out.push('_');
            last_underscore = true;
        }
    }
    out.trim_matches('_').to_owned()
}

fn strip_version_suffix(token: &str) -> &str {
    if let Some((base, suffix)) = token.rsplit_once("_v")
        && !suffix.is_empty()
        && suffix.bytes().all(|b| b.is_ascii_digit())
    {
        return base;
    }
    token
}

fn normalize_token(value: &str) -> String {
    let mut v = value.to_lowercase();
    for ext in STRIP_EXTENSIONS {
        if let Some(stripped) = v.strip_suffix(ext) {
            v = stripped.to_owned();
            break;
        }
    }
    for prefix in ["hero_", "citadel_"] {
        if let Some(stripped) = v.strip_prefix(prefix) {
            v = stripped.to_owned();
            break;
        }
    }
    // Repeatedly strip trailing _card/_psd/_alt/_c/_vN.
    loop {
        let before = v.len();
        for suffix in ["_card", "_psd", "_alt", "_c"] {
            if let Some(stripped) = v.strip_suffix(suffix) {
                v = stripped.to_owned();
            }
        }
        let base = strip_version_suffix(&v).to_owned();
        v = base;
        if v.len() == before {
            break;
        }
    }
    v.trim_matches('_').to_owned()
}

/// "Unassigned", "Haze", or "Haze +2".
pub fn hero_label(heroes: &[String]) -> String {
    match heroes {
        [] => "Unassigned".to_owned(),
        [one] => one.clone(),
        [first, rest @ ..] => format!("{first} +{}", rest.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_table_parses() {
        let t = HeroTable::from_json(BUILTIN).unwrap();
        assert_eq!(t.names().len(), 44);
    }

    #[test]
    fn tokens() {
        let t = HeroTable::builtin();
        assert_eq!(t.hero_from_token("punkgoat"), Some("Billy"));
        assert_eq!(t.hero_from_token("hornet_v3"), Some("Vindicta"));
        assert_eq!(t.hero_from_token("hero_tengu"), Some("Ivy"));
        assert_eq!(t.hero_from_token("magician_v7"), Some("Sinclair"));
        assert_eq!(t.hero_from_token("haze_card_psd"), Some("Haze"));
        assert_eq!(t.hero_from_token("nothing"), None);
        assert_eq!(t.hero_from_name("Lady Geist"), Some("Lady Geist"));
        assert_eq!(t.hero_from_name("Mo & Krill"), Some("Mo & Krill"));
    }

    #[test]
    fn detect_from_paths() {
        let t = HeroTable::builtin();
        let heroes = t.detect([
            "models/heroes_staging/punkgoat/punkgoat.vmdl_c",
            "panorama/images/heroes/haze_card_psd.vtex_c",
            "materials/generic/bull.vmat_c",
            "sounds/hero/hornet/attack.vsnd_c",
            "sounds/vo/nano/nano_desperation_power1_01.vsnd_c",
        ]);
        assert_eq!(heroes, vec!["Billy", "Calico", "Haze", "Vindicta"]);
    }

    #[test]
    fn labels() {
        assert_eq!(hero_label(&[]), "Unassigned");
        assert_eq!(hero_label(&["Haze".into(), "Ivy".into()]), "Haze +1");
    }
}
