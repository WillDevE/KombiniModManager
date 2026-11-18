//! End-to-end engine tests against synthetic game folders.

use crate::install::InstallMeta;
use crate::snapshots::SnapshotKind;
use crate::testutil::{Fixture, make_vpk};
use crate::{NoProgress, ProcessCheck};
use kombini_core::{ErrorCode, ModId, Placement};
use std::sync::atomic::Ordering;

fn meta(title: &str) -> InstallMeta {
    InstallMeta {
        title: Some(title.into()),
        ..InstallMeta::default()
    }
}

fn install(f: &Fixture, e: &crate::Engine, name: &str, entries: &[(&str, &str)]) -> ModId {
    let vpk = make_vpk(&f.scratch(name).join("pak01_dir.vpk"), entries);
    e.install_paths(&[vpk], meta(name), &NoProgress)
        .unwrap()
        .mod_id
}

fn mounted_name(e: &crate::Engine, id: &ModId) -> Option<(u8, String)> {
    e.with_state(|s| {
        s.mods[id].roots().find_map(|f| match &f.placement {
            Placement::Mounted { shard, file_name } => Some((*shard, file_name.clone())),
            _ => None,
        })
    })
}

#[test]
fn install_enable_disable_cycle() {
    let f = Fixture::new();
    let e = f.open();
    let id = install(&f, &e, "skin", &[("models/heroes/haze/a.vmdl_c", "A")]);
    assert!(id.as_str().starts_with("local_"));
    // New mods start disabled and parked under their disabled name.
    let parked = f.listing(1);
    assert_eq!(parked.len(), 1);
    assert!(parked[0].starts_with(id.as_str()), "{parked:?}");
    assert_eq!(
        e.with_state(|s| s.mods[&id].heroes.clone()),
        vec!["Haze".to_string()]
    );

    let r = e.set_enabled(std::slice::from_ref(&id), true).unwrap();
    assert_eq!(r.mounted_roots, 1);
    assert_eq!(f.listing(1), vec!["pak01_dir.vpk"]);
    let gi = f.gameinfo();
    assert!(gi.contains("Kombini Mod Manager"), "{gi}");
    assert!(gi.contains("citadel/addons"), "{gi}");

    e.set_enabled(std::slice::from_ref(&id), false).unwrap();
    assert_eq!(f.listing(1), parked);
}

#[test]
fn load_order_controls_slots_and_swaps() {
    let f = Fixture::new();
    let e = f.open();
    let a = install(&f, &e, "a", &[("x/a.txt", "AAAA")]);
    let b = install(&f, &e, "b", &[("x/b.txt", "BBBBBBBB")]);
    e.set_enabled(&[a.clone(), b.clone()], true).unwrap();
    assert_eq!(mounted_name(&e, &a), Some((1, "pak01_dir.vpk".into())));
    assert_eq!(mounted_name(&e, &b), Some((1, "pak02_dir.vpk".into())));
    let size_of = |n: &str| std::fs::metadata(f.addons(1).join(n)).unwrap().len();
    let a_size = size_of("pak01_dir.vpk");

    e.reorder_enabled(&[b.clone(), a.clone()]).unwrap();
    assert_eq!(mounted_name(&e, &b), Some((1, "pak01_dir.vpk".into())));
    assert_eq!(mounted_name(&e, &a), Some((1, "pak02_dir.vpk".into())));
    assert_eq!(size_of("pak02_dir.vpk"), a_size);
    assert_eq!(e.load_order(), vec![b.clone(), a.clone()]);

    e.set_pinned(&a, true).unwrap();
    assert_eq!(mounted_name(&e, &a), Some((1, "pak01_dir.vpk".into())));
}

#[test]
fn foreign_files_are_untouched_and_reserve_slots() {
    let f = Fixture::new();
    std::fs::write(f.addons(1).join("pak01_dir.vpk"), b"foreign").unwrap();
    std::fs::write(f.addons(1).join("readme.vpk"), b"other").unwrap();
    let e = f.open();
    assert_eq!(e.foreign_files().len(), 2);
    let id = install(&f, &e, "m", &[("a/b.txt", "x")]);
    e.set_enabled(std::slice::from_ref(&id), true).unwrap();
    assert_eq!(mounted_name(&e, &id), Some((1, "pak02_dir.vpk".into())));
    assert_eq!(
        std::fs::read(f.addons(1).join("pak01_dir.vpk")).unwrap(),
        b"foreign"
    );
    e.uninstall(std::slice::from_ref(&id)).unwrap();
    assert_eq!(f.listing(1), vec!["pak01_dir.vpk", "readme.vpk"]);
}

#[test]
fn game_running_blocks_changes() {
    let f = Fixture::new();
    let e = f.open();
    let id = install(&f, &e, "m", &[("a/b.txt", "x")]);
    let before = f.listing(1);
    f.process.0.store(true, Ordering::SeqCst);
    assert!(f.process.is_running());
    let err = e.set_enabled(std::slice::from_ref(&id), true).unwrap_err();
    assert_eq!(err.code, ErrorCode::GameRunning);
    assert_eq!(f.listing(1), before);
    // Metadata edits are still allowed.
    e.update_mod(
        &id,
        crate::library::ModPatch {
            favorite: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
}

#[test]
fn missing_files_are_detected_and_restored() {
    let f = Fixture::new();
    let e = f.open();
    let id = install(&f, &e, "m", &[("a/b.txt", "x")]);
    e.set_enabled(std::slice::from_ref(&id), true).unwrap();
    std::fs::remove_file(f.addons(1).join("pak01_dir.vpk")).unwrap();
    let r = e.rescan().unwrap();
    assert_eq!(r.missing_files, 1);
    let health = e.health();
    assert!(health.issues.iter().any(|i| i.code == "missing_files"));
    e.restore_files(&NoProgress).unwrap();
    assert_eq!(f.listing(1), vec!["pak01_dir.vpk"]);
    assert!(!e.health().issues.iter().any(|i| i.code == "missing_files"));
}

#[test]
fn manual_rename_is_followed_by_hash() {
    let f = Fixture::new();
    let e = f.open();
    let id = install(&f, &e, "m", &[("a/b.txt", "x")]);
    e.set_enabled(std::slice::from_ref(&id), true).unwrap();
    std::fs::rename(
        f.addons(1).join("pak01_dir.vpk"),
        f.addons(1).join("pak07_dir.vpk"),
    )
    .unwrap();
    let r = e.rescan().unwrap();
    assert_eq!((r.missing_files, r.relocated, r.foreign_files), (0, 1, 0));
    assert_eq!(mounted_name(&e, &id), Some((1, "pak07_dir.vpk".into())));
    // The next change normalizes it back.
    e.restore_files(&NoProgress).unwrap();
    assert_eq!(f.listing(1), vec!["pak01_dir.vpk"]);
}

#[test]
fn state_survives_reopen() {
    let f = Fixture::new();
    let id = {
        let e = f.open();
        let id = install(&f, &e, "m", &[("a/b.txt", "x")]);
        e.set_enabled(std::slice::from_ref(&id), true).unwrap();
        id
    };
    let e = f.open();
    assert_eq!(e.load_order(), vec![id.clone()]);
    assert_eq!(mounted_name(&e, &id), Some((1, "pak01_dir.vpk".into())));
    assert!(e.foreign_files().is_empty());
}

#[test]
fn game_update_reset_gameinfo_is_reapplied_on_open() {
    let f = Fixture::new();
    {
        let e = f.open();
        let id = install(&f, &e, "m", &[("a/b.txt", "x")]);
        e.set_enabled(std::slice::from_ref(&id), true).unwrap();
    }
    std::fs::write(
        f.root.join("game/citadel/gameinfo.gi"),
        crate::testutil::STOCK_GAMEINFO,
    )
    .unwrap();
    let (_, report) = crate::Engine::open(crate::EngineConfig {
        dirs: f.dirs(),
        game_root: f.root.clone(),
        process: f.process.clone(),
    })
    .unwrap();
    assert!(report.gameinfo_reapplied);
    assert!(f.gameinfo().contains("Kombini Mod Manager"));
}

#[test]
fn reinstall_replaces_content_and_keeps_state() {
    let f = Fixture::new();
    let e = f.open();
    let mut m = meta("m");
    m.remote_id = Some(42);
    let v1 = make_vpk(
        &f.scratch("v1").join("pak01_dir.vpk"),
        &[("a/b.txt", "one")],
    );
    let id = e
        .install_paths(&[v1], m.clone(), &NoProgress)
        .unwrap()
        .mod_id;
    assert_eq!(id.as_str(), "gb42");
    e.set_enabled(std::slice::from_ref(&id), true).unwrap();
    let before = std::fs::read(f.addons(1).join("pak01_dir.vpk")).unwrap();
    let v2 = make_vpk(
        &f.scratch("v2").join("pak01_dir.vpk"),
        &[("a/b.txt", "two!")],
    );
    let r = e.install_paths(&[v2], m, &NoProgress).unwrap();
    assert!(r.replaced);
    assert_eq!(f.listing(1), vec!["pak01_dir.vpk"]);
    assert_ne!(
        std::fs::read(f.addons(1).join("pak01_dir.vpk")).unwrap(),
        before
    );
    assert_eq!(e.load_order(), vec![id]);
}

#[test]
fn variants_select_first_and_can_switch() {
    let f = Fixture::new();
    let e = f.open();
    let dir = f.scratch("variants");
    make_vpk(
        &dir.join("Blue").join("pak01_dir.vpk"),
        &[("a/blue.txt", "b")],
    );
    make_vpk(
        &dir.join("Red").join("pak01_dir.vpk"),
        &[("a/red.txt", "r")],
    );
    let id = e
        .install_paths(&[dir], InstallMeta::default(), &NoProgress)
        .unwrap()
        .mod_id;
    let selected = |e: &crate::Engine| {
        e.with_state(|s| {
            s.mods[&id]
                .selected_roots()
                .map(|f| f.rel_path.clone())
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(selected(&e), vec!["Blue/pak01_dir.vpk"]);
    e.set_enabled(std::slice::from_ref(&id), true).unwrap();
    e.set_selection(&id, &["Red/pak01_dir.vpk".into()]).unwrap();
    assert_eq!(selected(&e), vec!["Red/pak01_dir.vpk"]);
    let names = f.listing(1);
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"pak01_dir.vpk".to_string()));
}

#[test]
fn zip_install_and_rejections() {
    use std::io::Write;
    let f = Fixture::new();
    let e = f.open();
    let vpk = make_vpk(&f.scratch("z").join("pak05_dir.vpk"), &[("a/z.txt", "z")]);
    let zip_path = f.scratch("zips").join("Cool Mod.zip");
    {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        zip.start_file(
            "Cool Mod/pak05_dir.vpk",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(&std::fs::read(&vpk).unwrap()).unwrap();
        zip.start_file("readme.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"hi").unwrap();
        zip.finish().unwrap();
    }
    let r = e
        .install_archive(&zip_path, InstallMeta::default(), &NoProgress)
        .unwrap();
    assert_eq!(r.title, "Cool Mod");
    assert_eq!(r.roots, 1);

    let empty = f.scratch("zips").join("empty.zip");
    {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&empty).unwrap());
        zip.start_file("readme.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"hi").unwrap();
        zip.finish().unwrap();
    }
    let err = e
        .install_archive(&empty, InstallMeta::default(), &NoProgress)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NoVpkInArchive);

    let bad = f.scratch("bad").join("pak01_dir.vpk");
    std::fs::write(&bad, b"not a vpk at all, definitely").unwrap();
    let err = e
        .install_paths(&[bad], InstallMeta::default(), &NoProgress)
        .unwrap_err();
    assert!(
        matches!(
            err.code,
            ErrorCode::InvalidVpk | ErrorCode::UnsupportedArchive
        ),
        "{err:?}"
    );
    // Failed installs leave no temp folders behind.
    let tmp = f.dirs().extract_tmp();
    assert!(!tmp.exists() || std::fs::read_dir(tmp).unwrap().count() == 0);
}

#[test]
fn snapshots_restore_uninstalled_mods() {
    let f = Fixture::new();
    let e = f.open();
    let a = install(&f, &e, "a", &[("x/a.txt", "a")]);
    e.set_enabled(std::slice::from_ref(&a), true).unwrap();
    let snap = e
        .create_snapshot("Good state", SnapshotKind::Manual)
        .unwrap();
    assert_eq!((snap.mods, snap.enabled), (1, 1));
    e.uninstall(std::slice::from_ref(&a)).unwrap();
    assert!(f.listing(1).is_empty());
    let r = e.restore_snapshot(&snap.id, &NoProgress).unwrap();
    assert!(r.safety_snapshot.is_some());
    assert_eq!(r.missing_blobs, 0);
    assert_eq!(f.listing(1), vec!["pak01_dir.vpk"]);
    assert_eq!(e.load_order(), vec![a]);

    // GC keeps blobs referenced by snapshots.
    let gc = e.gc_blobs().unwrap();
    assert_eq!(gc.removed, 0);

    // Export / import round trip.
    let out = f.scratch("export").join("snap.kmmsnap");
    e.export_snapshot(&snap.id, &out, &NoProgress).unwrap();
    let imported = e.import_snapshot(&out, &NoProgress).unwrap();
    assert!(imported.label.starts_with("Imported"));
    assert_eq!(imported.mods, 1);
}

#[test]
fn auto_snapshot_retention() {
    let f = Fixture::new();
    let e = f.open();
    let mut s = e.settings();
    s.snapshot_retention = 2;
    e.set_settings(s).unwrap();
    for i in 0..5 {
        e.create_snapshot(&format!("auto {i}"), SnapshotKind::Auto)
            .unwrap();
    }
    e.create_snapshot("manual", SnapshotKind::Manual).unwrap();
    let list = e.list_snapshots().unwrap();
    assert_eq!(list.iter().filter(|s| s.kind == "auto").count(), 2);
    assert_eq!(list.iter().filter(|s| s.kind == "manual").count(), 1);
}

#[test]
fn conflicts_and_profiles() {
    let f = Fixture::new();
    let e = f.open();
    let a = install(&f, &e, "a", &[("shared/x.txt", "a"), ("only/a.txt", "a")]);
    let b = install(&f, &e, "b", &[("shared/x.txt", "bb")]);
    e.set_enabled(&[a.clone(), b.clone()], true).unwrap();
    let view = e.conflicts();
    assert_eq!(view.conflicts.len(), 1);
    assert_eq!(view.conflicts[0].winner, a);
    assert_eq!(view.fully_overridden, vec![b.clone()]);
    e.set_ignored_conflict("shared/x.txt", true).unwrap();
    assert!(e.conflicts().conflicts.is_empty());

    // A second profile with nothing enabled.
    let p2 = e.create_profile("Vanilla-ish", None).unwrap();
    e.activate_profile(&p2, &NoProgress).unwrap();
    assert!(f.listing(1).iter().all(|n| !n.starts_with("pak")));
    let default = kombini_core::ProfileId::default_profile();
    e.activate_profile(&default, &NoProgress).unwrap();
    assert_eq!(f.listing(1), vec!["pak01_dir.vpk", "pak02_dir.vpk"]);
    e.delete_profile(&p2).unwrap();
    let err = e.delete_profile(&default).unwrap_err();
    assert_eq!(err.code, ErrorCode::LastProfile);

    // Groups toggle members.
    let g = e.create_group("Skins", &[a.clone(), b.clone()]).unwrap();
    e.set_group_enabled(&g, false).unwrap();
    assert!(e.load_order().is_empty());
}

#[test]
fn bisect_through_engine() {
    let f = Fixture::new();
    let e = f.open();
    let ids: Vec<ModId> = (0..4)
        .map(|i| {
            install(
                &f,
                &e,
                &format!("m{i}"),
                &[(&format!("p/{i}.txt") as &str, "x")],
            )
        })
        .collect();
    e.set_enabled(&ids, true).unwrap();
    let culprit = ids[2].clone();
    let mut b = e.bisect_start().unwrap();
    while b.result.is_none() {
        assert_eq!(e.load_order().len(), b.testing.len());
        b = e.bisect_report(b.testing.contains(&culprit)).unwrap();
    }
    assert_eq!(
        b.result,
        Some(crate::bisect::BisectResult::Found { mod_id: culprit })
    );
    e.bisect_finish().unwrap();
    assert_eq!(e.load_order(), ids);
    assert!(e.bisect_state().is_none());
}

#[test]
fn vanilla_launch_bookkeeping_reapplies_on_reopen() {
    let f = Fixture::new();
    {
        let e = f.open();
        let id = install(&f, &e, "m", &[("a/b.txt", "x")]);
        e.set_enabled(std::slice::from_ref(&id), true).unwrap();
        let mut settings = e.settings();
        settings.auto_reapply_gameinfo = false;
        e.set_settings(settings).unwrap();
        // Simulate the vanilla unpatch (Steam itself is not started in tests).
        e.db.lock()
            .set_setting(crate::launch::VANILLA_PENDING_KEY, &true)
            .unwrap();
        e.unpatch_gameinfo().unwrap();
        assert!(!f.gameinfo().contains("Kombini Mod Manager"));
    }
    // Even with auto-reapply off, a pending vanilla session is undone.
    f.open();
    assert!(f.gameinfo().contains("Kombini Mod Manager"));
}

#[test]
fn adopting_a_foreign_file_takes_its_slot() {
    let f = Fixture::new();
    let foreign_vpk = make_vpk(&f.addons(1).join("pak01_dir.vpk"), &[("adopt/me.txt", "x")]);
    let original = std::fs::read(&foreign_vpk).unwrap();
    let e = f.open();
    let foreign = e.foreign_files();
    assert_eq!(foreign.len(), 1);
    // Copy out and install parked, then swap it in for the original.
    let staging = f.scratch("adopt");
    std::fs::copy(&foreign_vpk, staging.join("pak01_dir.vpk")).unwrap();
    let id = e
        .install_dir(&staging, meta("Adopted"), &NoProgress)
        .unwrap()
        .mod_id;
    let adopted = id.clone();
    e.adopt_foreign("adopt", &NoProgress, &foreign, move |state| {
        let p = state.profile_mut();
        p.enabled.insert(adopted.clone());
        p.order.retain(|m| m != &adopted);
        p.order.insert(0, adopted);
        Ok("Adopted 1 mod".into())
    })
    .unwrap();
    assert!(e.foreign_files().is_empty());
    assert_eq!(mounted_name(&e, &id), Some((1, "pak01_dir.vpk".into())));
    assert_eq!(
        std::fs::read(f.addons(1).join("pak01_dir.vpk")).unwrap(),
        original
    );
    assert_eq!(f.listing(1), vec!["pak01_dir.vpk"]);
    // A stale request is refused.
    let err = e
        .adopt_foreign("adopt", &NoProgress, &foreign, |_| Ok(String::new()))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
}

/// Budget: applying a 100-mod load order must take < 2 s on an SSD and
/// stay atomic. Asserted in optimized builds only; timings always printed.
#[test]
fn budget_apply_load_order_100_mods() {
    use std::time::{Duration, Instant};
    let f = Fixture::new();
    let e = f.open();
    let ids: Vec<ModId> = (0..100)
        .map(|i| {
            let path = format!("x/{i}.txt");
            let body = "z".repeat(4096 + i);
            install(&f, &e, &format!("m{i}"), &[(path.as_str(), body.as_str())])
        })
        .collect();
    let t = Instant::now();
    e.set_enabled(&ids, true).unwrap();
    let enable = t.elapsed();
    let mut reversed = ids.clone();
    reversed.reverse();
    let t = Instant::now();
    e.reorder_enabled(&reversed).unwrap();
    let reorder = t.elapsed();
    assert_eq!(e.load_order(), reversed);
    assert_eq!(
        mounted_name(&e, &ids[99]),
        Some((1, "pak01_dir.vpk".into()))
    );
    println!("enable 100 mods: {enable:?}; reverse order of 100: {reorder:?}");
    if !cfg!(debug_assertions) {
        assert!(enable < Duration::from_secs(2), "enable took {enable:?}");
        assert!(reorder < Duration::from_secs(2), "reorder took {reorder:?}");
    }
}

#[test]
fn merge_keeps_load_order_priority_and_disables_sources() {
    let f = Fixture::new();
    let e = f.open();
    let a = install(&f, &e, "a", &[("x/shared.txt", "from-a"), ("x/a.txt", "A")]);
    let b = install(&f, &e, "b", &[("x/shared.txt", "from-b"), ("x/b.txt", "B")]);
    e.set_enabled(&[a.clone(), b.clone()], true).unwrap();
    e.reorder_enabled(&[a.clone(), b.clone()]).unwrap(); // a wins
    let r = e
        .merge_mods(&[b.clone(), a.clone()], "Combo", true, &NoProgress)
        .unwrap();
    assert_eq!(r.entries, 3);
    assert_eq!(r.overridden_paths, 1);
    let merged = r.install.mod_id.clone();
    let blob = e.with_state(|s| s.mods[&merged].roots().next().unwrap().blob.clone());
    let archive = kombini_vpk::VpkArchive::open(&e.blobs().path_for(&blob)).unwrap();
    assert_eq!(archive.read_file("x/shared.txt").unwrap(), b"from-a");
    assert_eq!(e.load_order(), vec![merged.clone()]);
    assert_eq!(mounted_name(&e, &merged), Some((1, "pak01_dir.vpk".into())));
    assert!(e.merge_mods(&[a], "One", false, &NoProgress).is_err());
}

/// A compiled resource holding one KV3 DATA block (same layout the
/// kombini-repair tests use).
fn kv3_resource(value: &kombini_source2::kv3::Value) -> Vec<u8> {
    use kombini_source2::kv3;
    let payload = kv3::encode(value, &kv3::Format([0; 16]));
    let mut bytes = vec![0; 28];
    bytes[4..6].copy_from_slice(&12u16.to_le_bytes());
    bytes[8..12].copy_from_slice(&8u32.to_le_bytes());
    bytes[12..16].copy_from_slice(&1u32.to_le_bytes());
    bytes[16..20].copy_from_slice(b"DATA");
    bytes[20..24].copy_from_slice(&8u32.to_le_bytes());
    bytes[24..28].copy_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
    bytes.extend(payload);
    let len = u32::try_from(bytes.len()).unwrap();
    bytes[..4].copy_from_slice(&len.to_le_bytes());
    bytes
}

#[test]
fn repair_fills_outdated_hero_data_and_keeps_authored_values() {
    use kombini_source2::kv3::Value;
    use kombini_vpk::EntrySource;
    let obj = |pairs: Vec<(&str, Value)>| {
        Value::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    };
    let f = Fixture::new();
    let entry = "scripts/heroes.vdata_c";
    // The installed game: hero_a gained a field, and a new hero exists.
    let game = obj(vec![
        (
            "hero_a",
            obj(vec![
                ("hp", Value::Int(100)),
                ("new_field", Value::Bool(true)),
            ]),
        ),
        ("hero_new", obj(vec![("hp", Value::Int(5))])),
    ]);
    let pak = f.root.join("game").join("citadel").join("pak01_dir.vpk");
    kombini_vpk::write_vpk(
        &pak,
        vec![(entry.into(), EntrySource::Bytes(kv3_resource(&game)))],
        &|| false,
    )
    .unwrap();
    let e = f.open();
    // A mod built against the older game, with an authored value.
    let old = obj(vec![("hero_a", obj(vec![("hp", Value::Int(999))]))]);
    let vpk = f.scratch("stale").join("pak01_dir.vpk");
    kombini_vpk::write_vpk(
        &vpk,
        vec![
            (entry.into(), EntrySource::Bytes(kv3_resource(&old))),
            ("keep.txt".into(), EntrySource::Bytes(b"authored".to_vec())),
        ],
        &|| false,
    )
    .unwrap();
    let id = e
        .install_paths(&[vpk], meta("stale"), &NoProgress)
        .unwrap()
        .mod_id;
    e.set_enabled(std::slice::from_ref(&id), true).unwrap();
    let clean = install(&f, &e, "clean", &[("x/y.txt", "fine")]);

    let scan = e.scan_mod_issues(&NoProgress).unwrap();
    let found = scan.iter().find(|m| m.mod_id == id).unwrap();
    assert_eq!(found.issues.len(), 1, "{:?}", found.issues);
    let issue = &found.issues[0];
    assert_eq!(issue.kind, crate::repair::IssueKind::OutdatedData);
    assert!(issue.repairable);
    assert!(issue.detail.contains("hero_new"), "{}", issue.detail);
    assert!(
        scan.iter()
            .find(|m| m.mod_id == clean)
            .unwrap()
            .issues
            .is_empty()
    );

    let old_blob = e.with_state(|s| s.mods[&id].roots().next().unwrap().blob.clone());
    let out = e.repair_mod(&id, &NoProgress).unwrap();
    assert_eq!((out.files, out.entries), (1, 1));
    assert!(out.remaining.is_empty(), "{:?}", out.remaining);
    let new_blob = e.with_state(|s| s.mods[&id].roots().next().unwrap().blob.clone());
    assert_ne!(old_blob, new_blob);
    assert!(e.blobs().contains(&old_blob), "original kept for snapshots");

    let archive = kombini_vpk::VpkArchive::open(&e.blobs().path_for(&new_blob)).unwrap();
    let fixed = kombini_source2::decode_kv3_resource(&archive.read_file(entry).unwrap()).unwrap();
    let hero_a = fixed.get("hero_a").unwrap();
    assert_eq!(
        hero_a.get("hp"),
        Some(&Value::Int(999)),
        "authored value kept"
    );
    assert_eq!(hero_a.get("new_field"), Some(&Value::Bool(true)));
    assert!(fixed.get("hero_new").is_some());
    assert_eq!(archive.read_file("keep.txt").unwrap(), b"authored");
    // Still mounted, now from the repaired blob.
    assert_eq!(mounted_name(&e, &id), Some((1, "pak01_dir.vpk".into())));
    assert!(
        !e.list_snapshots().unwrap().is_empty(),
        "auto snapshot taken"
    );

    let again = e.scan_mod_issues(&NoProgress).unwrap();
    assert!(again.iter().all(|m| m.issues.is_empty()));
    // Nothing left to do: a second repair changes nothing.
    let noop = e.repair_mod(&id, &NoProgress).unwrap();
    assert_eq!(noop.files, 0);
}

#[test]
fn update_keeps_previous_version_and_rolls_back() {
    let f = Fixture::new();
    let e = f.open();
    let mut m = meta("m");
    m.remote_id = Some(7);
    m.version = Some("1.0".into());
    let v1 = make_vpk(
        &f.scratch("r1").join("pak01_dir.vpk"),
        &[("a/b.txt", "one")],
    );
    let id = e
        .install_paths(&[v1], m.clone(), &NoProgress)
        .unwrap()
        .mod_id;
    e.set_enabled(std::slice::from_ref(&id), true).unwrap();
    let first = std::fs::read(f.addons(1).join("pak01_dir.vpk")).unwrap();
    m.version = Some("2.0".into());
    let v2 = make_vpk(
        &f.scratch("r2").join("pak01_dir.vpk"),
        &[("a/b.txt", "two!")],
    );
    e.install_paths(&[v2], m.clone(), &NoProgress).unwrap();
    let second = std::fs::read(f.addons(1).join("pak01_dir.vpk")).unwrap();
    assert_ne!(first, second);
    e.with_state(|s| {
        let h = &s.mods[&id].history;
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].version.as_deref(), Some("1.0"));
    });
    // Reinstalling identical content adds no history entry.
    let v2b = make_vpk(
        &f.scratch("r2b").join("pak01_dir.vpk"),
        &[("a/b.txt", "two!")],
    );
    e.install_paths(&[v2b], m, &NoProgress).unwrap();
    assert_eq!(e.with_state(|s| s.mods[&id].history.len()), 1);
    // GC keeps the old version's blob.
    e.gc_blobs().unwrap();
    e.restore_version(&id, 0).unwrap();
    assert_eq!(
        std::fs::read(f.addons(1).join("pak01_dir.vpk")).unwrap(),
        first
    );
    e.with_state(|s| {
        let m = &s.mods[&id];
        assert_eq!(m.version.as_deref(), Some("1.0"));
        assert_eq!(m.history[0].version.as_deref(), Some("2.0"));
    });
    // And forward again.
    e.restore_version(&id, 0).unwrap();
    assert_eq!(
        std::fs::read(f.addons(1).join("pak01_dir.vpk")).unwrap(),
        second
    );
    assert!(e.restore_version(&id, 5).is_err());
    // Keeping the replaced version as its own mod places both files.
    let report = e.keep_version_as_mod(&id, 0).unwrap();
    assert!(report.summary.starts_with("Added "), "{}", report.summary);
    e.with_state(|s| {
        assert!(s.mods[&id].history.is_empty());
        let (new_id, kept) = s
            .mods
            .iter()
            .find(|(k, _)| **k != id)
            .expect("a second mod");
        assert_eq!(kept.version.as_deref(), Some("1.0"));
        let profile = s.profile();
        assert!(profile.enabled.contains(new_id));
        let at = profile.order.iter().position(|o| o == &id).unwrap();
        assert_eq!(profile.order.get(at + 1), Some(new_id));
    });
    assert_eq!(
        std::fs::read(f.addons(1).join("pak01_dir.vpk")).unwrap(),
        second
    );
    assert_eq!(
        std::fs::read(f.addons(1).join("pak02_dir.vpk")).unwrap(),
        first
    );
    assert!(e.keep_version_as_mod(&id, 0).is_err());
}
