//! Import/export against synthetic game folders.

use crate::{interchange, modprofile, unmanaged};
use kombini_engine::install::InstallMeta;
use kombini_engine::{AppDirs, Engine, EngineConfig, NoProgress, ProcessCheck};
use kombini_vpk::EntrySource;
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct NotRunning;
impl ProcessCheck for NotRunning {
    fn is_running(&self) -> bool {
        false
    }
}

struct Game {
    dir: tempfile::TempDir,
    root: PathBuf,
}

impl Game {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Deadlock");
        let citadel = root.join("game/citadel");
        std::fs::create_dir_all(citadel.join("addons")).unwrap();
        std::fs::write(
            citadel.join("gameinfo.gi"),
            include_str!("../../kombini-gameinfo/tests/fixtures/stock.gi"),
        )
        .unwrap();
        Self { dir, root }
    }

    fn open(&self, data: &str) -> Engine {
        Engine::open(EngineConfig {
            dirs: AppDirs {
                data: self.dir.path().join(data),
                cache: self.dir.path().join(format!("{data}-cache")),
            },
            game_root: self.root.clone(),
            process: Arc::new(NotRunning),
        })
        .unwrap()
        .0
    }

    fn addons(&self) -> PathBuf {
        self.root.join("game/citadel/addons")
    }

    fn listing(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(self.addons())
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_file())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        v.sort();
        v
    }
}

fn vpk(path: &Path, entries: &[(&str, &str)]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    kombini_vpk::write_vpk(
        path,
        entries
            .iter()
            .map(|(p, c)| ((*p).to_owned(), EntrySource::Bytes(c.as_bytes().to_vec())))
            .collect(),
        &|| false,
    )
    .unwrap();
}

#[test]
fn adopts_mounted_and_parked_files_in_order() {
    let g = Game::new();
    vpk(&g.addons().join("pak01_dir.vpk"), &[("a/first.txt", "1")]);
    vpk(&g.addons().join("pak02_dir.vpk"), &[("a/second.txt", "2")]);
    vpk(
        &g.addons().join("12345_cool_skin_dir.vpk"),
        &[("b/parked.txt", "3")],
    );
    let e = g.open("data");
    let candidates = unmanaged::scan(&e);
    assert_eq!(candidates.len(), 3);
    assert_eq!(candidates[0].slot, Some(1));
    assert_eq!(candidates[1].slot, Some(2));
    assert_eq!(candidates[2].remote_id, Some(12345));
    assert_eq!(candidates[2].title, "Cool Skin");

    let report = unmanaged::adopt(&e, &candidates, &NoProgress).unwrap();
    assert_eq!(report.adopted.len(), 3, "{:?}", report.failed);
    assert!(e.foreign_files().is_empty());
    let order = e.load_order();
    assert_eq!(order, report.adopted[..2].to_vec());
    assert_eq!(report.adopted[2].as_str(), "gb12345");
    // Two mounted paks + one parked file named by its new id.
    let names = g.listing();
    assert_eq!(names.len(), 3);
    assert!(names.contains(&"pak01_dir.vpk".to_string()));
    assert!(names.contains(&"pak02_dir.vpk".to_string()));
    assert!(names.iter().any(|n| n.starts_with("gb12345_")));
}

#[test]
fn interchange_round_trip_between_installs() {
    let g = Game::new();
    let e = g.open("data");
    let src = g.dir.path().join("src");
    vpk(&src.join("Blue/pak01_dir.vpk"), &[("x/blue.txt", "b")]);
    vpk(&src.join("Red/pak01_dir.vpk"), &[("x/red.txt", "r")]);
    let id = e
        .install_dir(
            &src,
            InstallMeta {
                title: Some("Colors".into()),
                remote_id: Some(77),
                ..InstallMeta::default()
            },
            &NoProgress,
        )
        .unwrap()
        .mod_id;
    e.set_selection(&id, &["Red/pak01_dir.vpk".into()]).unwrap();
    e.set_enabled(std::slice::from_ref(&id), true).unwrap();
    let bundle = g.dir.path().join("bundle");
    interchange::export(&e, &bundle, true, &NoProgress).unwrap();
    drop(e);

    // A fresh library on another "machine" (new data dir, same game).
    let g2 = Game::new();
    let e2 = g2.open("data2");
    let report = interchange::import(&e2, &bundle, None, &NoProgress).unwrap();
    assert_eq!(report.imported.len(), 1, "{:?}", report.warnings);
    assert_eq!(report.profiles.len(), 1);
    let imported = &report.imported[0];
    assert_eq!(imported.as_str(), "gb77");
    let selected: Vec<String> = e2.with_state(|s| {
        s.mods[imported]
            .selected_roots()
            .map(|f| f.rel_path.clone())
            .collect()
    });
    assert_eq!(selected, vec!["Red/pak01_dir.vpk"]);
    assert_eq!(e2.load_order(), vec![imported.clone()]);
    assert_eq!(
        g2.listing().iter().filter(|n| n.starts_with("pak")).count(),
        1
    );

    // Importing again skips what exists.
    let again = interchange::import(&e2, &bundle, None, &NoProgress).unwrap();
    assert!(again.imported.is_empty());
    assert_eq!(again.skipped.len(), 1);
}

#[test]
fn tampered_bundle_is_rejected() {
    let g = Game::new();
    let e = g.open("data");
    let src = g.dir.path().join("src");
    vpk(&src.join("pak01_dir.vpk"), &[("x/a.txt", "a")]);
    e.install_dir(&src, InstallMeta::default(), &NoProgress)
        .unwrap();
    let bundle = g.dir.path().join("bundle");
    interchange::export(&e, &bundle, false, &NoProgress).unwrap();
    let g2 = Game::new();
    let e2 = g2.open("data2");
    // Corrupt the bundled file.
    let file = walk(&bundle.join("files")).pop().unwrap();
    let mut bytes = std::fs::read(&file).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    std::fs::write(&file, bytes).unwrap();
    let report = interchange::import(&e2, &bundle, None, &NoProgress).unwrap();
    assert!(report.imported.is_empty());
    assert!(
        report.warnings[0].contains("corrupt"),
        "{:?}",
        report.warnings
    );
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        if e.path().is_dir() {
            out.extend(walk(&e.path()));
        } else {
            out.push(e.path());
        }
    }
    out
}

#[test]
fn mod_profile_maps_installed_and_reports_missing() {
    let g = Game::new();
    let e = g.open("data");
    let src = g.dir.path().join("src");
    vpk(&src.join("pak01_dir.vpk"), &[("x/a.txt", "a")]);
    let id = e
        .install_dir(
            &src,
            InstallMeta {
                remote_id: Some(5),
                title: Some("Five".into()),
                ..InstallMeta::default()
            },
            &NoProgress,
        )
        .unwrap()
        .mod_id;
    e.set_enabled(std::slice::from_ref(&id), true).unwrap();
    let (doc, warnings) = modprofile::export(&e, None).unwrap();
    assert!(warnings.is_empty());
    let mut doc2 = modprofile::decode(&modprofile::encode(&doc).unwrap()).unwrap();
    doc2.profile = serde_json::json!({"name": "Shared"});
    doc2.mods.push(modprofile::Entry {
        source: "gamebanana".into(),
        reference: modprofile::ModRef {
            submission_id: Some(999),
            file_id: None,
        },
        enabled: Some(true),
        priority: Some(1),
        hint: modprofile::Hint {
            name: Some("Not installed".into()),
            ..Default::default()
        },
    });
    let result = modprofile::apply(&e, &doc2, true, &NoProgress).unwrap();
    assert_eq!(result.enabled, vec![id.clone()]);
    assert_eq!(result.missing.len(), 1);
    assert_eq!(result.missing[0].submission_id, 999);
    assert_eq!(e.with_state(|s| s.profile().name.clone()), "Shared");
    assert_eq!(e.load_order(), vec![id]);
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn migrates_a_dmm_library_with_profiles() {
    use crate::managers;
    let g = Game::new();
    let addons = g.addons();
    vpk(&addons.join("pak01_dir.vpk"), &[("a/cool.txt", "1")]);
    vpk(&addons.join("222_other_dir.vpk"), &[("a/other.txt", "2")]);
    write(
        &addons.join(".dmm.json"),
        r#"{"version":3,"mods":{
            "111":{"enabled":true,"order":0,"shard":1,"currentVpks":["pak01_dir.vpk"],"disabledVpks":[],"originalVpkNames":["cool.vpk"]},
            "222":{"enabled":false,"order":1,"currentVpks":[],"disabledVpks":["222_other_dir.vpk"]}}}"#,
    );
    let alt = addons.join("profile_1_ab12_Alt");
    vpk(&alt.join("pak01_dir.vpk"), &[("a/third.txt", "3")]);
    write(
        &alt.join(".dmm.json"),
        r#"{"version":3,"mods":{"333":{"enabled":true,"order":0,"currentVpks":["pak01_dir.vpk"]}}}"#,
    );
    let data = g.dir.path().join("dmm-data");
    let inner = serde_json::json!({
        "state": {
            "activeProfileId": "p1",
            "localMods": [
                {"remoteId": "111", "name": "Cool Skin", "author": "Bob",
                 "selectedDownloads": [{"url": "https://gamebanana.com/dl/9001", "name": "cool.zip"}]},
                {"remoteId": "222", "name": "Other"}
            ],
            "profiles": {
                "p1": {"name": "Main", "folderName": null, "isDefault": true,
                       "enabledMods": {"111": {"enabled": true}}, "mods": []},
                "p2": {"name": "Alt", "folderName": "profile_1_ab12_Alt", "isDefault": false,
                       "enabledMods": {"333": {"enabled": true}},
                       "mods": [{"remoteId": "333", "name": "Third"}]}
            }
        },
        "version": 3
    });
    write(
        &data.join("state.json"),
        &serde_json::json!({ "local-config": inner.to_string() }).to_string(),
    );

    let citadel = g.root.join("game/citadel");
    let found = managers::read_dmm(&citadel, std::slice::from_ref(&data)).unwrap();
    assert_eq!(found.doc.mods.len(), 3, "{:?}", found.doc.warnings);
    assert_eq!(found.doc.profiles.len(), 2);
    assert_eq!(found.enabled_count(), 1);
    assert_eq!(found.bookkeeping.len(), 2);

    let e = g.open("data");
    let report = managers::migrate(&e, &found, &NoProgress).unwrap();
    assert_eq!(report.imported, 3, "{:?}", report.warnings);
    assert_eq!(report.profiles, 2);
    assert_eq!(report.moved, 5);
    let backup = report.backup.unwrap();
    assert!(backup.join("addons/.dmm.json").is_file());
    assert!(
        backup
            .join("addons/profile_1_ab12_Alt/pak01_dir.vpk")
            .is_file()
    );
    assert!(!alt.exists(), "the emptied DMM profile folder is removed");
    assert!(e.foreign_files().is_empty());

    e.with_state(|s| {
        let cool = s.mods.values().find(|m| m.remote_id == Some(111)).unwrap();
        assert_eq!(cool.title, "Cool Skin");
        assert_eq!(cool.author.as_deref(), Some("Bob"));
        let other = s.mods.values().find(|m| m.remote_id == Some(222)).unwrap();
        let third = s.mods.values().find(|m| m.remote_id == Some(333)).unwrap();
        let active = s.profile();
        assert_eq!(active.name, "Main");
        assert!(active.enabled.contains(&cool.id));
        assert!(!active.enabled.contains(&other.id));
        assert!(!active.enabled.contains(&third.id));
        let alt = s.profiles.iter().find(|p| p.name == "Alt").unwrap();
        assert!(alt.enabled.contains(&third.id));
    });
    // Kombini's own files only: one mounted pak, the rest parked by id.
    let names = g.listing();
    assert_eq!(names.iter().filter(|n| n.starts_with("pak")).count(), 1);
    assert!(
        names
            .iter()
            .all(|n| n.starts_with("pak") || n.starts_with("gb"))
    );
    assert!(!addons.join(".dmm.json").exists());
}

#[test]
fn migrates_a_grimoire_library_with_profiles() {
    use crate::managers;
    let g = Game::new();
    let citadel = g.root.join("game/citadel");
    vpk(
        &citadel.join("grimoire/pak01_dir.vpk"),
        &[("locker/cards.txt", "L")],
    );
    vpk(
        &citadel.join("grimoire/pak05_dir.vpk"),
        &[("a/glow.txt", "G")],
    );
    vpk(&g.addons().join("pak01_dir.vpk"), &[("a/skin.txt", "S")]);
    vpk(
        &g.addons().join(".disabled/old_dir.vpk"),
        &[("a/old.txt", "O")],
    );
    let data = g.dir.path().join("Grimoire");
    write(
        &data.join("mod-metadata.json"),
        r#"{
            "grimoire/pak05_dir.vpk": {"modName": "Glow", "gameBananaId": 500},
            "pak01_dir.vpk": {"modName": "Skin A", "gameBananaId": 501, "sha256": "AA", "author": "Ann"},
            "old_dir.vpk": {"modName": "Old", "variantLabel": "blue"}
        }"#,
    );
    write(
        &data.join("profiles.json"),
        r#"[{"id": "profile_1", "name": "Ranked", "mods": [
            {"fileName": "pak09_dir.vpk", "enabled": true, "priority": 0, "sha256": "aa"},
            {"fileName": "old_dir.vpk", "enabled": true, "priority": 1}
        ]}]"#,
    );

    let found = managers::read_grimoire(&citadel, &data).unwrap();
    assert_eq!(found.doc.mods.len(), 3, "the Locker's pak01 is not a mod");
    assert_eq!(found.enabled_count(), 2);

    let e = g.open("data");
    let report = managers::migrate(&e, &found, &NoProgress).unwrap();
    assert_eq!(report.imported, 3, "{:?}", report.warnings);
    assert_eq!(report.profiles, 2);
    assert!(citadel.join("grimoire/pak01_dir.vpk").is_file());
    assert!(!citadel.join("grimoire/pak05_dir.vpk").exists());

    e.with_state(|s| {
        let by = |t: &str| s.mods.values().find(|m| m.title == t).unwrap().id.clone();
        let (glow, skin, old) = (by("Glow"), by("Skin A"), by("Old (blue)"));
        assert_eq!(s.mods[&skin].remote_id, Some(501));
        let active = s.profile();
        assert_eq!(active.name, "Grimoire");
        assert!(active.enabled.contains(&glow) && active.enabled.contains(&skin));
        assert!(!active.enabled.contains(&old));
        assert_eq!(active.order[..2], [glow, skin.clone()]);
        let ranked = s
            .profiles
            .iter()
            .find(|p| p.name == "Grimoire: Ranked")
            .unwrap();
        assert!(ranked.enabled.contains(&skin) && ranked.enabled.contains(&old));
    });
}

#[test]
fn grimoire_files_from_one_gamebanana_page_stay_separate_mods() {
    use crate::managers;
    let g = Game::new();
    let citadel = g.root.join("game/citadel");
    vpk(&g.addons().join("pak01_dir.vpk"), &[("a/skin.txt", "S")]);
    vpk(&g.addons().join("pak02_dir.vpk"), &[("a/voice.txt", "V")]);
    vpk(&g.addons().join("pak03_dir.vpk"), &[("a/icons.txt", "I")]);
    let data = g.dir.path().join("Grimoire");
    write(
        &data.join("mod-metadata.json"),
        r#"{
            "pak01_dir.vpk": {"modName": "Rice", "gameBananaId": 700, "gameBananaFileId": 1, "sourceFileName": "skin.zip"},
            "pak02_dir.vpk": {"modName": "Rice", "gameBananaId": 700, "gameBananaFileId": 2, "sourceFileName": "voicelines.zip"},
            "pak03_dir.vpk": {"modName": "Rice", "gameBananaId": 700, "gameBananaFileId": 3, "sourceFileName": "icons.zip"}
        }"#,
    );
    let found = managers::read_grimoire(&citadel, &data).unwrap();
    let e = g.open("data");
    let report = managers::migrate(&e, &found, &NoProgress).unwrap();
    assert_eq!(report.imported, 3, "{:?}", report.warnings);

    e.with_state(|s| {
        assert_eq!(s.mods.len(), 3, "each file is its own mod");
        let mut files: Vec<_> = s.mods.values().map(|m| m.file_id).collect();
        files.sort();
        assert_eq!(files, [Some(1), Some(2), Some(3)]);
        assert!(s.mods.values().all(|m| m.remote_id == Some(700)));
        assert!(
            s.mods.values().all(|m| m.history.is_empty()),
            "nothing was replaced"
        );
        assert!(s.mods.contains_key(&kombini_core::ModId::gamebanana(700)));
        let active = s.profile();
        assert_eq!(active.enabled.len(), 3, "all three stay enabled");
    });

    // Importing the same files again adds nothing.
    let again = managers::read_grimoire(&citadel, &data).unwrap();
    assert!(
        again.doc.mods.is_empty() || {
            let r = managers::migrate(&e, &again, &NoProgress).unwrap();
            r.imported == 0
        }
    );
}
