//! Steam and Deadlock install discovery.

use crate::SteamError;
use crate::build::parse_app_manifest;
use crate::vdf::parse_kv;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DeadlockInstall {
    pub root: PathBuf,
    /// Steam library root that contains this install.
    pub library: Option<PathBuf>,
}

fn dedup_key(path: &Path) -> String {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    resolved
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

fn push_unique(out: &mut Vec<PathBuf>, seen: &mut BTreeSet<String>, path: PathBuf) {
    if seen.insert(dedup_key(&path)) {
        out.push(path);
    }
}

fn candidate_steam_dirs() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    #[cfg(windows)]
    {
        use winreg::RegKey;
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        let read = |root, key: &str, value: &str| -> Option<String> {
            RegKey::predef(root)
                .open_subkey(key)
                .ok()?
                .get_value::<String, _>(value)
                .ok()
        };
        if let Some(p) = read(HKEY_CURRENT_USER, r"Software\Valve\Steam", "SteamPath") {
            candidates.push(PathBuf::from(p));
        }
        if let Some(p) = read(
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\WOW6432Node\Valve\Steam",
            "InstallPath",
        ) {
            candidates.push(PathBuf::from(p));
        }
        if let Some(p) = read(HKEY_LOCAL_MACHINE, r"SOFTWARE\Valve\Steam", "InstallPath") {
            candidates.push(PathBuf::from(p));
        }
        candidates.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
        candidates.push(PathBuf::from(r"C:\Program Files\Steam"));
    }
    #[cfg(not(windows))]
    {
        if let Some(base) = directories::BaseDirs::new() {
            let home = base.home_dir();
            for rel in [
                ".local/share/Steam",
                ".steam/steam",
                ".steam/root",
                ".var/app/com.valvesoftware.Steam/.local/share/Steam",
                "snap/steam/common/.local/share/Steam",
            ] {
                candidates.push(home.join(rel));
            }
        }
    }
    candidates
}

/// Steam directories that contain a `steamapps` folder, deduplicated.
pub fn find_steam_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for dir in candidate_steam_dirs() {
        if dir.join("steamapps").is_dir() {
            push_unique(&mut out, &mut seen, dir);
        }
    }
    out
}

/// `path` values from libraryfolders.vdf text.
pub fn parse_library_folders(text: &str) -> Vec<PathBuf> {
    let kv = parse_kv(text);
    let Some(folders) = kv.get("libraryfolders") else {
        return Vec::new();
    };
    folders
        .children()
        .iter()
        .filter_map(|(_, entry)| entry.get("path").and_then(|p| p.as_str()))
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// The steam dir itself plus every library listed in its libraryfolders.vdf.
pub fn library_folders(steam_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    push_unique(&mut out, &mut seen, steam_dir.to_path_buf());
    let vdf = steam_dir.join("steamapps").join("libraryfolders.vdf");
    if let Ok(text) = std::fs::read_to_string(&vdf) {
        for lib in parse_library_folders(&text) {
            push_unique(&mut out, &mut seen, lib);
        }
    }
    out
}

/// Every Deadlock install found through Steam.
pub fn find_deadlock_installs() -> Vec<DeadlockInstall> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for steam in find_steam_dirs() {
        for library in library_folders(&steam) {
            let steamapps = library.join("steamapps");
            let mut candidates = Vec::new();
            let manifest = steamapps.join(format!("appmanifest_{}.acf", crate::APP_ID));
            if let Ok(text) = std::fs::read_to_string(&manifest) {
                let dir = parse_app_manifest(&text)
                    .install_dir
                    .unwrap_or_else(|| "Deadlock".to_owned());
                candidates.push(steamapps.join("common").join(dir));
            }
            candidates.push(steamapps.join("common").join("Deadlock"));
            for candidate in candidates {
                if let Ok(root) = validate_game_root(&candidate)
                    && seen.insert(dedup_key(&root))
                {
                    out.push(DeadlockInstall {
                        root,
                        library: Some(library.clone()),
                    });
                }
            }
        }
    }
    out
}

fn has_gameinfo(root: &Path) -> bool {
    root.join("game")
        .join("citadel")
        .join("gameinfo.gi")
        .is_file()
}

/// Accepts the game root, `game/`, `game/citadel/`, or the gameinfo.gi file,
/// and returns the root (the folder that contains `game/`).
pub fn validate_game_root(path: &Path) -> Result<PathBuf, SteamError> {
    let mut candidate = Some(path);
    for _ in 0..4 {
        let Some(c) = candidate else { break };
        if has_gameinfo(c) {
            return Ok(c.to_path_buf());
        }
        candidate = c.parent();
    }
    Err(SteamError::NotDeadlock(path.to_path_buf()))
}

/// The Steam directory that owns this game root, or the first detected one.
pub fn steam_dir_for(root: &Path) -> Option<PathBuf> {
    let dirs = find_steam_dirs();
    let root_key = dedup_key(root);
    for dir in &dirs {
        for library in library_folders(dir) {
            let common = dedup_key(&library.join("steamapps").join("common"));
            if root_key.starts_with(&common) {
                return Some(dir.clone());
            }
        }
    }
    dirs.into_iter().next()
}

/// Deadlock's per-account settings files (`userdata/<account>/1422450/
/// local/cfg/machine_convars.vcfg`), most recently written first. The
/// crosshair and other client settings are saved here.
pub fn machine_convars_files(steam_dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<(std::time::SystemTime, PathBuf)> =
        std::fs::read_dir(steam_dir.join("userdata"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|account| {
                account
                    .path()
                    .join(crate::APP_ID.to_string())
                    .join("local")
                    .join("cfg")
                    .join("machine_convars.vcfg")
            })
            .filter_map(|p| {
                let modified = std::fs::metadata(&p).and_then(|m| m.modified()).ok()?;
                Some((modified, p))
            })
            .collect();
    found.sort_by_key(|a| std::cmp::Reverse(a.0));
    found.into_iter().map(|(_, p)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_machine_convars_per_account() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("userdata/123/1422450/local/cfg");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(cfg.join("machine_convars.vcfg"), "x").unwrap();
        std::fs::create_dir_all(dir.path().join("userdata/456/1422450")).unwrap();
        let found = machine_convars_files(dir.path());
        assert_eq!(found, vec![cfg.join("machine_convars.vcfg")]);
        assert!(machine_convars_files(&dir.path().join("nope")).is_empty());
    }

    #[test]
    fn library_paths() {
        let text = "\"libraryfolders\"\n{\n\"0\" { \"path\" \"c:\\\\program files (x86)\\\\steam\" }\n\"1\" { \"path\" \"D:\\\\SteamLibrary\" }\n}";
        assert_eq!(
            parse_library_folders(text),
            vec![
                PathBuf::from(r"c:\program files (x86)\steam"),
                PathBuf::from(r"D:\SteamLibrary")
            ]
        );
        assert!(parse_library_folders("garbage").is_empty());
    }

    #[test]
    fn validate_root_variants() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Deadlock");
        let citadel = root.join("game").join("citadel");
        std::fs::create_dir_all(&citadel).unwrap();
        std::fs::write(citadel.join("gameinfo.gi"), "x").unwrap();
        for input in [
            root.clone(),
            root.join("game"),
            citadel.clone(),
            citadel.join("gameinfo.gi"),
        ] {
            assert_eq!(validate_game_root(&input).unwrap(), root, "{input:?}");
        }
        let empty = tempfile::tempdir().unwrap();
        assert!(matches!(
            validate_game_root(empty.path()),
            Err(SteamError::NotDeadlock(_))
        ));
    }
}
