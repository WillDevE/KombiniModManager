//! Deadlock build information from appmanifest and steam.inf.

use crate::vdf::parse_kv;
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct AppManifest {
    pub install_dir: Option<String>,
    pub build_id: Option<String>,
    pub last_updated: Option<i64>,
    pub size_on_disk: Option<u64>,
    pub state_flags: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct SteamInf {
    pub client_version: Option<String>,
    pub version_date: Option<String>,
    pub version_time: Option<String>,
    pub source_revision: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct BuildInfo {
    pub build_id: Option<String>,
    pub client_version: Option<String>,
    pub version_date: Option<String>,
    pub last_updated: Option<i64>,
    /// Steam reports an update in progress / required for the game.
    pub update_in_progress: bool,
    /// Changes whenever the game updates; compared against the stored value.
    pub fingerprint: String,
}

/// Steam `StateFlags` bits meaning "not ready": UpdateRequired, UpdateRunning,
/// UpdatePaused, UpdateStarted.
const UPDATING_FLAGS: u32 = 2 | 256 | 512 | 1024;

pub fn parse_app_manifest(text: &str) -> AppManifest {
    let kv = parse_kv(text);
    let Some(state) = kv.get("AppState") else {
        return AppManifest::default();
    };
    let s = |key: &str| {
        state
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    AppManifest {
        install_dir: s("installdir"),
        build_id: s("buildid"),
        last_updated: s("LastUpdated").and_then(|v| v.parse().ok()),
        size_on_disk: s("SizeOnDisk").and_then(|v| v.parse().ok()),
        state_flags: s("StateFlags").and_then(|v| v.parse().ok()),
    }
}

pub fn parse_steam_inf(text: &str) -> SteamInf {
    let mut inf = SteamInf::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let value = Some(value.to_owned());
        match key.trim().to_ascii_lowercase().as_str() {
            "clientversion" => inf.client_version = value,
            "versiondate" => inf.version_date = value,
            "versiontime" => inf.version_time = value,
            "sourcerevision" => inf.source_revision = value,
            _ => {}
        }
    }
    inf
}

pub fn build_info_from(manifest: &AppManifest, inf: &SteamInf) -> BuildInfo {
    let version_date = match (&inf.version_date, &inf.version_time) {
        (Some(d), Some(t)) => Some(format!("{d} {t}")),
        (Some(d), None) => Some(d.clone()),
        _ => None,
    };
    let dash = |v: &Option<String>| v.clone().unwrap_or_else(|| "-".to_owned());
    BuildInfo {
        build_id: manifest.build_id.clone(),
        client_version: inf.client_version.clone(),
        version_date,
        last_updated: manifest.last_updated,
        update_in_progress: manifest
            .state_flags
            .is_some_and(|f| f & UPDATING_FLAGS != 0),
        fingerprint: format!(
            "{}:{}:{}",
            dash(&manifest.build_id),
            dash(&inf.client_version),
            dash(&inf.source_revision)
        ),
    }
}

/// Reads steam.inf and the appmanifest next to `<lib>/steamapps/common/<root>`.
/// Missing files produce `None` fields, never an error.
pub fn read_build_info(root: &Path) -> BuildInfo {
    let inf = std::fs::read_to_string(root.join("game").join("citadel").join("steam.inf"))
        .map(|t| parse_steam_inf(&t))
        .unwrap_or_default();
    let manifest = root
        .parent()
        .and_then(Path::parent)
        .map(|steamapps| steamapps.join(format!("appmanifest_{}.acf", crate::APP_ID)))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| parse_app_manifest(&t))
        .unwrap_or_default();
    build_info_from(&manifest, &inf)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = "\"AppState\"\n{\n\t\"appid\"\t\t\"1422450\"\n\t\"installdir\"\t\t\"Deadlock\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"LastUpdated\"\t\t\"1762650000\"\n\t\"SizeOnDisk\"\t\t\"30000000000\"\n\t\"buildid\"\t\t\"20512345\"\n}\n";
    const INF: &str = "ClientVersion=6861\r\nServerVersion=6861\r\nSourceRevision=11219368\r\nVersionDate=Nov 09 2026\r\nVersionTime=10:12:11\r\n";

    #[test]
    fn parses_manifest_and_inf() {
        let m = parse_app_manifest(MANIFEST);
        assert_eq!(m.install_dir.as_deref(), Some("Deadlock"));
        assert_eq!(m.build_id.as_deref(), Some("20512345"));
        assert_eq!(m.size_on_disk, Some(30_000_000_000));
        assert_eq!(m.state_flags, Some(4));
        let inf = parse_steam_inf(INF);
        assert_eq!(inf.client_version.as_deref(), Some("6861"));
        let b = build_info_from(&m, &inf);
        assert!(!b.update_in_progress);
        assert_eq!(b.fingerprint, "20512345:6861:11219368");
        assert_eq!(b.version_date.as_deref(), Some("Nov 09 2026 10:12:11"));
        let updating = AppManifest {
            state_flags: Some(1026),
            ..m
        };
        assert!(build_info_from(&updating, &inf).update_in_progress);
        let bad = parse_app_manifest("\"AppState\" { \"StateFlags\" \"x\" }");
        assert_eq!(bad.state_flags, None);
    }

    #[test]
    fn reads_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let steamapps = dir.path().join("steamapps");
        let root = steamapps.join("common").join("Deadlock");
        let citadel = root.join("game").join("citadel");
        std::fs::create_dir_all(&citadel).unwrap();
        std::fs::write(citadel.join("steam.inf"), INF).unwrap();
        std::fs::write(steamapps.join("appmanifest_1422450.acf"), MANIFEST).unwrap();
        let b = read_build_info(&root);
        assert_eq!(b.build_id.as_deref(), Some("20512345"));
        assert_eq!(b.client_version.as_deref(), Some("6861"));
        let missing = read_build_info(dir.path());
        assert_eq!(missing.fingerprint, "-:-:-");
    }
}
