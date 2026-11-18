//! The installed game's version and the list of game updates, for the
//! "made for an older game" estimate (`kombini_core::freshness`).
//!
//! The version comes from the game's own `steam.inf`/appmanifest (offline).
//! Update dates come from Valve's Steam news feed, cached for
//! [`REFRESH_SECS`], merged with the built-in list of major updates.

use crate::Engine;
use kombini_core::freshness::{self, Freshness, GameUpdate};
use kombini_core::model::{Mod, ModSource};
use kombini_steam::BuildInfo;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Steam news is refetched after this long.
pub const REFRESH_SECS: i64 = 6 * 3600;
const CACHE_FILE: &str = "game_updates.json";
/// The installed build counts as its own (minor) update when no listed
/// update was announced within this window before it.
const BUILD_MATCH_SECS: i64 = 2 * 86_400;

#[derive(Debug, Clone, Default)]
pub struct GameTimeline {
    pub build: BuildInfo,
    /// When the installed build was made (steam.inf), unix seconds.
    pub build_time: Option<i64>,
    /// Newest first; includes the installed build when it is newer than
    /// every announced update.
    pub updates: Vec<GameUpdate>,
    /// When Steam news was last fetched successfully.
    pub fetched_at: Option<i64>,
}

impl GameTimeline {
    pub fn latest_major(&self) -> Option<&GameUpdate> {
        freshness::latest_major(&self.updates)
    }

    pub fn latest(&self) -> Option<&GameUpdate> {
        self.updates.first()
    }

    /// Estimate for an installed mod; local mods have no upload date.
    pub fn assess_mod(&self, m: &Mod) -> Freshness {
        freshness::assess(mod_upload_date(m), &self.updates)
    }

    pub fn assess(&self, date: Option<i64>) -> Freshness {
        freshness::assess(date, &self.updates)
    }
}

/// Upload date of the installed GameBanana file.
pub fn mod_upload_date(m: &Mod) -> Option<i64> {
    match m.source {
        ModSource::Gamebanana => m.remote_updated_at,
        _ => None,
    }
}

#[derive(Serialize, Deserialize)]
struct Cache {
    fetched_at: i64,
    updates: Vec<GameUpdate>,
}

fn cache_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join(CACHE_FILE)
}

fn load_cache(cache_dir: &Path) -> Option<Cache> {
    let text = std::fs::read_to_string(cache_path(cache_dir)).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_cache(cache_dir: &Path, cache: &Cache) {
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(cache_dir)?;
        let tmp = cache_dir.join(format!("{CACHE_FILE}.tmp"));
        std::fs::write(
            &tmp,
            serde_json::to_vec(cache).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(tmp, cache_path(cache_dir))
    };
    if let Err(e) = write() {
        tracing::warn!(error = %e, "could not cache game update list");
    }
}

fn fetch_updates() -> Result<Vec<GameUpdate>, String> {
    let items = kombini_steam::news::fetch_news(100)?;
    Ok(items
        .into_iter()
        .filter_map(|i| {
            freshness::classify_news(&i.title, &i.tags).map(|major| GameUpdate {
                date: i.date,
                title: i.title,
                major,
                url: Some(i.url).filter(|u| !u.is_empty()),
            })
        })
        .collect())
}

/// Builds the timeline from `build` and an optional fetched list.
pub fn assemble(build: BuildInfo, fetched: &[GameUpdate], fetched_at: Option<i64>) -> GameTimeline {
    let build_time = build.version_date.as_deref().and_then(|v| {
        // build_info_from joins "date time"; split the time back off.
        let mut parts: Vec<&str> = v.split_whitespace().collect();
        let time = parts.last().filter(|p| p.contains(':')).copied();
        if time.is_some() {
            parts.pop();
        }
        freshness::parse_version_date(&parts.join(" "), time)
    });
    let mut updates = freshness::merge_updates(fetched);
    if let Some(bt) = build_time {
        let covered = updates
            .iter()
            .any(|u| u.date <= bt + BUILD_MATCH_SECS && u.date >= bt - BUILD_MATCH_SECS)
            || updates.first().is_some_and(|u| u.date > bt);
        if !covered {
            updates.insert(
                0,
                GameUpdate {
                    date: bt,
                    title: format!(
                        "Game build {}",
                        build.client_version.as_deref().unwrap_or("?")
                    ),
                    major: false,
                    url: None,
                },
            );
        }
    }
    GameTimeline {
        build,
        build_time,
        updates,
        fetched_at,
    }
}

impl Engine {
    /// The installed version and the update list. With `refresh`, refetches
    /// Steam news when the cache is older than [`REFRESH_SECS`]; network
    /// errors fall back to the cache and the built-in list.
    pub fn game_timeline(&self, refresh: bool) -> GameTimeline {
        let build = kombini_steam::read_build_info(&self.paths().root);
        let cache_dir = &self.dirs().cache;
        let mut cache = load_cache(cache_dir);
        let now = kombini_storage::now();
        let stale = cache
            .as_ref()
            .is_none_or(|c| now - c.fetched_at > REFRESH_SECS);
        if refresh && stale {
            match fetch_updates() {
                Ok(updates) => {
                    let fresh = Cache {
                        fetched_at: now,
                        updates,
                    };
                    save_cache(cache_dir, &fresh);
                    cache = Some(fresh);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Steam news unavailable; using cached update list")
                }
            }
        }
        let (fetched, fetched_at) = cache
            .map(|c| (c.updates, Some(c.fetched_at)))
            .unwrap_or_default();
        assemble(build, &fetched, fetched_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(date: &str) -> BuildInfo {
        BuildInfo {
            client_version: Some("6861".into()),
            version_date: Some(date.into()),
            ..BuildInfo::default()
        }
    }

    #[test]
    fn silent_build_becomes_a_minor_update() {
        let t = assemble(build("Nov 09 2026 10:12:11"), &[], None);
        let first = t.latest().unwrap();
        assert_eq!(first.title, "Game build 6861");
        assert!(!first.major);
        assert_eq!(
            t.latest_major().unwrap().title,
            "Halloween 2026 + Gameplay Update"
        );
        // A mod uploaded after Halloween but before the silent build.
        assert!(matches!(
            t.assess(Some(1_793_500_000)),
            Freshness::MinorBehind { count: 1, .. }
        ));
        assert!(t.assess(Some(1_700_000_000)).maybe_outdated());
    }

    #[test]
    fn announced_build_is_not_duplicated() {
        let fetched = [GameUpdate {
            date: 1_794_200_000,
            title: "Minor Update - 11-08-2026".into(),
            major: false,
            url: None,
        }];
        let t = assemble(build("Nov 09 2026 10:12:11"), &fetched, Some(1));
        assert_eq!(t.latest().unwrap().title, "Minor Update - 11-08-2026");
    }
}
