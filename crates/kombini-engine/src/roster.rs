//! The game's hero roster, in hero-select order, with portraits.
//!
//! Read from the installed game so it follows updates: `scripts/heroes.vdata_c`
//! lists the heroes (picker shows those not disabled with development state
//! `Release`), the English localization gives names and the sort names the
//! picker orders by, and each hero's card image (`m_strIconHeroCard`) is
//! decoded to a PNG. Everything is cached per game build in
//! `<cache>/heroes/`.

use crate::Engine;
use kombini_core::heroes::HeroTable;
use kombini_core::{AppError, ErrorCode};

type Result<T> = std::result::Result<T, AppError>;
use kombini_source2::kv3::Value;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Bump when the cached layout or selection rules change.
const ROSTER_VERSION: u32 = 1;
const ROSTER_FILE: &str = "roster.json";
/// Localization files that carry hero names, most specific first.
const NAME_FILES: &[&str] = &["citadel_gc_hero_names", "citadel_gc", "citadel_main"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterHero {
    /// The vdata key without `hero_` (`bebop`, `atlas`).
    pub codename: String,
    /// The name Kombini uses for the hero (its hero table name when known).
    pub name: String,
    /// The English name the picker sorts by.
    pub sort: String,
    /// Portrait PNG, when it could be decoded.
    pub portrait: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Roster {
    pub version: u32,
    /// Game build this was read from.
    pub fingerprint: String,
    /// Hero-select order.
    pub heroes: Vec<RosterHero>,
}

fn invalid(e: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::InvalidVpk, e.to_string())
}

/// `"key:n" "value"` / `"key" "value"` lines of a Valve localization file.
pub fn parse_localization(text: &str, into: &mut HashMap<String, String>) {
    for line in text.lines() {
        let parts: Vec<&str> = line.split('"').collect();
        if parts.len() < 5 {
            continue;
        }
        let key = parts[1]
            .split(':')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        into.entry(key).or_insert_with(|| parts[3].to_owned());
    }
}

/// Heroes the picker shows, as (vdata key, card image reference).
pub fn picker_heroes(heroes: &Value) -> Vec<(String, Option<String>)> {
    let Some(entries) = heroes.as_object() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|(key, hero)| {
            let disabled = hero
                .get("m_bDisabled")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let released = hero.get("m_eHeroDevelopmentState").and_then(Value::as_str)
                == Some("EHeroDevState_Release");
            (key.starts_with("hero_") && !disabled && released).then(|| {
                let card = hero
                    .get("m_strIconHeroCard")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                (key.clone(), card)
            })
        })
        .collect()
}

/// Kombini's name for a hero: its hero table entry, else the game's name.
pub fn table_name(table: &HeroTable, codename: &str, display: Option<&str>, sort: &str) -> String {
    table
        .hero_from_token(codename)
        .or_else(|| table.hero_from_token(&format!("hero_{codename}")))
        .or_else(|| display.and_then(|d| table.hero_from_name(d)))
        .or_else(|| table.hero_from_name(sort))
        .map(str::to_owned)
        .or_else(|| display.map(str::to_owned))
        .unwrap_or_else(|| codename.to_owned())
}

fn write_png(path: &Path, image: &kombini_source2::texture::RgbaImage) -> std::io::Result<()> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, image.width, image.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
        writer
            .write_image_data(&image.pixels)
            .map_err(std::io::Error::other)?;
    }
    crate::fsops::write_atomic(path, &bytes).map_err(|e| std::io::Error::other(e.to_string()))
}

impl Engine {
    /// The hero-select roster for the installed build (cached per build).
    pub fn hero_roster(&self) -> Result<Roster> {
        let dir = self.dirs().cache.join("heroes");
        let fingerprint = kombini_steam::read_build_info(&self.paths().root).fingerprint;
        if let Some(cached) = std::fs::read(dir.join(ROSTER_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice::<Roster>(&b).ok())
            .filter(|r| {
                r.version == ROSTER_VERSION
                    && r.fingerprint == fingerprint
                    && r.heroes
                        .iter()
                        .all(|h| h.portrait.as_ref().is_none_or(|p| p.is_file()))
            })
        {
            return Ok(cached);
        }
        let pak = kombini_vpk::VpkArchive::open(&self.paths().citadel.join("pak01_dir.vpk"))
            .map_err(invalid)?;
        let vdata = pak.read_file("scripts/heroes.vdata_c").map_err(invalid)?;
        let heroes = kombini_source2::decode_kv3_resource(&vdata).map_err(invalid)?;
        let mut names = HashMap::new();
        let loc = self.paths().citadel.join("resource").join("localization");
        for file in NAME_FILES {
            let path = loc.join(file).join(format!("{file}_english.txt"));
            if let Ok(bytes) = std::fs::read(&path) {
                parse_localization(&String::from_utf8_lossy(&bytes), &mut names);
            }
        }
        std::fs::create_dir_all(&dir)
            .map_err(|e| AppError::from_io("Creating the hero cache", &dir, &e))?;
        let table = HeroTable::builtin();
        let mut out: Vec<RosterHero> = picker_heroes(&heroes)
            .into_iter()
            .map(|(key, card)| {
                let codename = key.trim_start_matches("hero_").to_owned();
                let display = names.get(&key).cloned();
                let sort = names
                    .get(&format!("{key}_sort"))
                    .cloned()
                    .or_else(|| display.clone())
                    .unwrap_or_else(|| codename.clone());
                let portrait = card
                    .as_deref()
                    .and_then(kombini_source2::texture::panorama_image_path)
                    .and_then(|path| pak.read_file(&path).ok())
                    .and_then(|bytes| {
                        kombini_source2::texture::decode_texture(&bytes)
                            .map_err(|e| tracing::warn!(codename, error = %e, "hero portrait not decoded"))
                            .ok()
                    })
                    .and_then(|image| {
                        let path = dir.join(format!("{codename}.png"));
                        write_png(&path, &image)
                            .map_err(|e| tracing::warn!(codename, error = %e, "hero portrait not saved"))
                            .ok()
                            .map(|_| path)
                    });
                RosterHero {
                    name: table_name(table, &codename, display.as_deref(), &sort),
                    codename,
                    sort,
                    portrait,
                }
            })
            .collect();
        out.sort_by_key(|h| h.sort.to_lowercase());
        let roster = Roster {
            version: ROSTER_VERSION,
            fingerprint,
            heroes: out,
        };
        if let Ok(json) = serde_json::to_vec_pretty(&roster)
            && let Err(e) = crate::fsops::write_atomic(&dir.join(ROSTER_FILE), &json)
        {
            tracing::warn!(error = %e, "could not cache the hero roster");
        }
        Ok(roster)
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_localization, table_name};
    use kombini_core::heroes::HeroTable;
    use std::collections::HashMap;

    #[test]
    fn localization_lines() {
        let mut m = HashMap::new();
        parse_localization(
            "\"lang\"\n{\n\t\"Tokens\"\n\t{\n\t\t\"hero_doorman:n\" \"The Doorman\"\n\t\t\"hero_doorman_sort\"\t\"Doorman\"\n",
            &mut m,
        );
        assert_eq!(
            m.get("hero_doorman").map(String::as_str),
            Some("The Doorman")
        );
        assert_eq!(
            m.get("hero_doorman_sort").map(String::as_str),
            Some("Doorman")
        );
    }

    #[test]
    fn maps_codenames_to_table_names() {
        let t = HeroTable::builtin();
        assert_eq!(table_name(t, "atlas", Some("Abrams"), "Abrams"), "Abrams");
        assert_eq!(
            table_name(t, "doorman", Some("The Doorman"), "Doorman"),
            "The Doorman"
        );
        assert_eq!(
            table_name(t, "newguy", Some("New Guy"), "New Guy"),
            "New Guy"
        );
    }

    /// Reads the installed game; runs only with DEADLOCK_PAK set.
    #[test]
    fn installed_game_roster() {
        let Some(pak) = std::env::var_os("DEADLOCK_PAK") else {
            return;
        };
        let archive = kombini_vpk::VpkArchive::open(std::path::Path::new(&pak)).unwrap();
        let vdata = archive.read_file("scripts/heroes.vdata_c").unwrap();
        let heroes = kombini_source2::decode_kv3_resource(&vdata).unwrap();
        let list = super::picker_heroes(&heroes);
        assert!(list.len() >= 30, "{}", list.len());
        assert!(list.iter().any(|(k, _)| k == "hero_bebop"));
        assert!(!list.iter().any(|(k, _)| k == "hero_targetdummy"));
        let card = list
            .iter()
            .find(|(k, _)| k == "hero_bebop")
            .and_then(|(_, c)| c.as_deref())
            .and_then(kombini_source2::texture::panorama_image_path)
            .unwrap();
        let image =
            kombini_source2::texture::decode_texture(&archive.read_file(&card).unwrap()).unwrap();
        assert_eq!((image.width, image.height), (280, 380));
        // Every picker hero has a name Kombini's skin detection uses.
        let citadel = std::path::Path::new(&pak).parent().unwrap();
        let mut names = HashMap::new();
        for file in super::NAME_FILES {
            let path = citadel
                .join("resource/localization")
                .join(file)
                .join(format!("{file}_english.txt"));
            if let Ok(b) = std::fs::read(path) {
                parse_localization(&String::from_utf8_lossy(&b), &mut names);
            }
        }
        let t = HeroTable::builtin();
        let unknown: Vec<String> = list
            .iter()
            .map(|(k, _)| {
                let code = k.trim_start_matches("hero_");
                let display = names.get(k).cloned();
                let sort = names
                    .get(&format!("{k}_sort"))
                    .cloned()
                    .or(display.clone())
                    .unwrap_or_default();
                table_name(t, code, display.as_deref(), &sort)
            })
            .filter(|n| !t.names().contains(n))
            .collect();
        assert!(unknown.is_empty(), "not in the hero table: {unknown:?}");
    }
}
