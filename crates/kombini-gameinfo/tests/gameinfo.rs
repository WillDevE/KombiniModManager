#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;

use kombini_gameinfo::{
    EditError, GameInfoError, GameInfoFile, IssueCode, MANAGED_END, MANAGED_START, analyze,
    ensure_text, remove_other_manager_mounts_text, repair_markers_text, unpatch_text,
};

const STOCK: &[u8] = include_bytes!("fixtures/stock.gi");
const LEGACY_MANUAL: &[u8] = include_bytes!("fixtures/legacy_manual.gi");
const LEGACY_BLOCK: &[u8] = include_bytes!("fixtures/legacy_block.gi");
const DMM: &[u8] = include_bytes!("fixtures/dmm.gi");
const COMMENT_FIRST: &[u8] = include_bytes!("fixtures/comment_first.gi");
const NO_SEARCHPATHS: &[u8] = include_bytes!("fixtures/no_searchpaths.gi");

const CITADEL_LINE: &str = "\t\t\tGame\t\t\t\tcitadel\r\n";

fn stock() -> String {
    String::from_utf8(STOCK.to_vec()).unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// Full one-shard managed block as it appears in a CRLF, 3-tab file.
fn block_crlf(shards: u8) -> String {
    let mut s = format!("\t\t\t{MANAGED_START}\r\n");
    for p in kombini_gameinfo::addon_paths(shards) {
        s.push_str(&format!("\t\t\tGame {p}\r\n"));
    }
    s.push_str(
        "\t\t\tMod citadel\r\n\t\t\tWrite citadel\r\n\t\t\tMod core\r\n\t\t\tWrite core\r\n",
    );
    s.push_str(&format!("\t\t\t{MANAGED_END}\r\n"));
    s
}

fn setup(bytes: &[u8]) -> (tempfile::TempDir, GameInfoFile) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gameinfo.gi");
    fs::write(&path, bytes).unwrap();
    (dir, GameInfoFile::new(path))
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

#[test]
fn stock_gets_block_above_citadel_and_everything_else_is_identical() {
    let input = stock();
    let out = ensure_text(&input, 1, true).unwrap();
    assert_eq!(
        out,
        input.replacen(CITADEL_LINE, &format!("{}{CITADEL_LINE}", block_crlf(1)), 1)
    );
    assert!(analyze(&out).patched);
    assert!(!out.replace("\r\n", "").contains('\n'), "CRLF must be kept");
}

#[test]
fn legacy_manual_addon_line_is_replaced_by_full_block() {
    let out = ensure_text(&text(LEGACY_MANUAL), 1, true).unwrap();
    assert_eq!(out, ensure_text(&stock(), 1, true).unwrap());
    assert_eq!(count(&out, "citadel/addons"), 1);
}

#[test]
fn legacy_kombini_block_is_replaced_by_full_block() {
    let out = ensure_text(&text(LEGACY_BLOCK), 1, true).unwrap();
    assert_eq!(out, ensure_text(&stock(), 1, true).unwrap());
    assert_eq!(count(&out, MANAGED_START), 1);
    assert_eq!(count(&out, MANAGED_END), 1);
}

#[test]
fn already_correct_is_idempotent_and_not_rewritten() {
    let (dir, file) = setup(STOCK);
    let first = file.ensure(1).unwrap();
    assert!(first.changed);
    let bytes = fs::read(file.path()).unwrap();
    let mtime = fs::metadata(file.path()).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(30));
    let second = file.ensure(1).unwrap();
    assert!(!second.changed);
    assert!(!second.backup_created);
    assert_eq!(fs::read(file.path()).unwrap(), bytes);
    assert_eq!(
        fs::metadata(file.path()).unwrap().modified().unwrap(),
        mtime
    );
    let t = text(&bytes);
    assert_eq!(ensure_text(&t, 1, true).unwrap(), t);
    drop(dir);
}

#[test]
fn ten_shards_write_ten_addon_lines_and_one_mod_write_set() {
    let out = ensure_text(&stock(), 10, true).unwrap();
    assert_eq!(count(&out, "\t\t\tGame citadel/addons"), 10);
    assert!(out.contains("Game citadel/addons10\r\n"));
    for line in ["Mod citadel", "Write citadel", "Mod core", "Write core"] {
        assert_eq!(count(&out, &format!("\t\t\t{line}\r\n")), 1, "{line}");
    }
    assert_eq!(ensure_text(&out, 10, true).unwrap(), out);
    // Values above 10 are clamped.
    assert_eq!(ensure_text(&stock(), 99, true).unwrap(), out);
}

#[test]
fn shard_count_change_shrinks_block() {
    let three = ensure_text(&stock(), 3, true).unwrap();
    assert_eq!(count(&three, "\t\t\tGame citadel/addons"), 3);
    let one = ensure_text(&three, 1, true).unwrap();
    assert_eq!(one, ensure_text(&stock(), 1, true).unwrap());
    let grown = ensure_text(&one, 3, true).unwrap();
    assert_eq!(grown, three);
}

#[test]
fn unpatch_returns_exact_stock() {
    let patched = ensure_text(&stock(), 3, true).unwrap();
    assert_eq!(unpatch_text(&patched), stock());

    let (_dir, file) = setup(STOCK);
    file.ensure(2).unwrap();
    let report = file.unpatch().unwrap();
    assert!(report.changed);
    assert_eq!(fs::read(file.path()).unwrap(), STOCK);
    assert!(!file.unpatch().unwrap().changed);
}

#[test]
fn utf8_bom_and_lf_round_trip() {
    let lf = stock().replace("\r\n", "\n");
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(lf.as_bytes());
    let (_dir, file) = setup(&bytes);
    let status = file.status();
    assert!(status.has_bom);
    assert!(!status.crlf);
    file.ensure(1).unwrap();
    let out = fs::read(file.path()).unwrap();
    assert_eq!(&out[..3], &[0xEF, 0xBB, 0xBF]);
    assert!(!out.contains(&b'\r'));
    assert_eq!(&out[3..6], b"\"Ga");
    file.unpatch().unwrap();
    assert_eq!(fs::read(file.path()).unwrap(), bytes);
}

#[test]
fn latin1_byte_in_comment_survives() {
    let mut bytes = b"// caf\xE9 comment\r\n".to_vec();
    bytes.extend_from_slice(STOCK);
    let (_dir, file) = setup(&bytes);
    file.ensure(1).unwrap();
    let out = fs::read(file.path()).unwrap();
    assert!(out.starts_with(b"// caf\xE9 comment\r\n"));
    assert!(
        !out.windows(2).any(|w| w == [0xC3, 0xA9]),
        "must not be re-encoded as UTF-8"
    );
    file.unpatch().unwrap();
    assert_eq!(fs::read(file.path()).unwrap(), bytes);
}

#[test]
fn read_only_file_is_made_writable() {
    let (_dir, file) = setup(STOCK);
    let mut perms = fs::metadata(file.path()).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(file.path(), perms).unwrap();
    assert!(file.status().read_only);
    assert!(
        file.status()
            .issues
            .iter()
            .any(|i| i.code == IssueCode::ReadOnly)
    );
    let report = file.ensure(1).unwrap();
    assert!(report.changed);
    assert!(report.cleared_read_only);
    assert!(file.status().patched);
}

#[test]
fn backup_is_created_once_and_never_overwritten() {
    let (_dir, file) = setup(STOCK);
    let first = file.ensure(1).unwrap();
    assert!(first.backup_created);
    assert_eq!(fs::read(file.backup_path()).unwrap(), STOCK);
    assert!(
        file.backup_path()
            .to_string_lossy()
            .ends_with("gameinfo.gi.bak")
    );
    let second = file.ensure(2).unwrap();
    assert!(second.changed);
    assert!(!second.backup_created);
    assert_eq!(fs::read(file.backup_path()).unwrap(), STOCK);
}

#[test]
fn missing_search_paths_leaves_file_untouched() {
    assert_eq!(
        ensure_text(&text(NO_SEARCHPATHS), 1, true),
        Err(EditError::MissingSearchPaths)
    );
    let (_dir, file) = setup(NO_SEARCHPATHS);
    let err = file.ensure(1).unwrap_err();
    assert!(matches!(
        err,
        GameInfoError::Edit(EditError::MissingSearchPaths)
    ));
    assert_eq!(fs::read(file.path()).unwrap(), NO_SEARCHPATHS);
    assert!(!file.backup_path().exists());
    let status = file.status();
    assert!(
        status
            .issues
            .iter()
            .any(|i| i.code == IssueCode::MissingSearchPaths)
    );
}

#[test]
fn duplicate_start_marker_is_corrupt_and_repair_fixes_it() {
    let good = ensure_text(&stock(), 2, true).unwrap();
    let start_line = format!("\t\t\t{MANAGED_START}\r\n");
    let corrupt = good.replacen(&start_line, &format!("{start_line}{start_line}"), 1);
    assert_eq!(
        ensure_text(&corrupt, 2, true),
        Err(EditError::MarkersCorrupt)
    );

    let (_dir, file) = setup(corrupt.as_bytes());
    assert!(
        file.status()
            .issues
            .iter()
            .any(|i| i.code == IssueCode::MarkersCorrupt)
    );
    assert!(matches!(
        file.ensure(2),
        Err(GameInfoError::Edit(EditError::MarkersCorrupt))
    ));
    assert_eq!(fs::read(file.path()).unwrap(), corrupt.as_bytes());

    let report = file.repair_markers(2).unwrap();
    assert!(report.changed);
    assert_eq!(text(&fs::read(file.path()).unwrap()), good);
    assert!(file.status().patched);
    assert_eq!(repair_markers_text(&good, 2, true).unwrap(), good);
}

#[test]
fn unpaired_end_marker_is_repaired() {
    let good = ensure_text(&stock(), 1, true).unwrap();
    let broken = good.replacen(&format!("\t\t\t{MANAGED_START}\r\n"), "", 1);
    assert_eq!(
        ensure_text(&broken, 1, true),
        Err(EditError::MarkersCorrupt)
    );
    assert_eq!(repair_markers_text(&broken, 1, true).unwrap(), good);
}

#[test]
fn conditional_line_is_kept_and_not_counted() {
    let line = "\t\t\tGame\t\tcitadel/addons [$WIN64]\r\n";
    let input = stock().replacen(CITADEL_LINE, &format!("{line}{CITADEL_LINE}"), 1);
    let a = analyze(&input);
    assert!(a.addon_paths.is_empty());
    assert_eq!(a.duplicate_addon_mounts, 0);
    assert!(a.mounts.iter().any(|m| m.conditional));
    let out = ensure_text(&input, 1, true).unwrap();
    assert!(out.contains(line));
    assert!(analyze(&out).patched);
    assert_eq!(analyze(&out).addon_paths, vec!["citadel/addons"]);
}

#[test]
fn compound_key_satisfies_role_and_is_not_removed() {
    let compound = "\t\t\tGame+Mod\t\tcitadel\r\n";
    let input = stock().replacen(CITADEL_LINE, compound, 1);
    let a = analyze(&input);
    assert!(!a.missing_roles.contains(&"Mod citadel".to_string()));
    assert!(a.missing_roles.contains(&"Write citadel".to_string()));
    let m = a.mounts.iter().find(|m| m.key == "Game+Mod").unwrap();
    assert_eq!(m.roles, vec!["game", "mod"]);
    let out = ensure_text(&input, 1, true).unwrap();
    assert!(out.contains(compound));
    assert!(analyze(&out).patched);
    assert_eq!(ensure_text(&out, 1, true).unwrap(), out);
}

#[test]
fn dmm_fence_is_detected_kept_and_removable() {
    let input = text(DMM);
    let a = analyze(&input);
    assert_eq!(a.other_manager.as_deref(), Some("DMM"));
    assert_eq!(a.addon_paths, vec!["citadel/addons"]);
    assert_eq!(a.duplicate_addon_mounts, 0);
    assert!(
        a.mounts
            .iter()
            .any(|m| m.in_foreign_fence && m.path == "citadel/addons")
    );

    let fenced = "\t\t\tGame citadel/addons // dmm-search-path\r\n";
    let out = ensure_text(&input, 1, true).unwrap();
    assert!(out.contains(fenced));
    let after = analyze(&out);
    assert!(after.patched);
    assert_eq!(after.addon_paths.len(), 2);
    assert_eq!(after.other_manager.as_deref(), Some("DMM"));
    assert_eq!(ensure_text(&out, 1, true).unwrap(), out);

    let cleaned = remove_other_manager_mounts_text(&out);
    assert!(!cleaned.contains(fenced));
    assert!(cleaned.contains("// BEGIN DMM"));
    assert!(cleaned.contains("// END DMM"));
    assert_eq!(cleaned, out.replacen(fenced, "", 1));
    assert_eq!(analyze(&cleaned).addon_paths, vec!["citadel/addons"]);
    assert_eq!(remove_other_manager_mounts_text(&cleaned), cleaned);

    let (_dir, file) = setup(DMM);
    assert!(
        file.status()
            .issues
            .iter()
            .any(|i| i.code == IssueCode::OtherManager)
    );
    file.ensure(1).unwrap();
    assert!(file.remove_other_manager_mounts().unwrap().changed);
    assert!(!file.remove_other_manager_mounts().unwrap().changed);
}

#[test]
fn unknown_fence_is_reported_as_unknown() {
    let input = stock().replacen(
        CITADEL_LINE,
        &format!("\t\t\t// >>> Some Tool >>>\r\n\t\t\tGame citadel/addons\r\n\t\t\t// <<< Some Tool <<<\r\n{CITADEL_LINE}"),
        1,
    );
    assert_eq!(analyze(&input).other_manager.as_deref(), Some("unknown"));
}

#[test]
fn quoted_key_and_value_are_recognized() {
    let line = "\t\t\t\"Game\"\t\t\"citadel/addons\"\r\n";
    let input = stock().replacen(CITADEL_LINE, &format!("{line}{CITADEL_LINE}"), 1);
    let a = analyze(&input);
    assert_eq!(a.addon_paths, vec!["citadel/addons"]);
    assert_eq!(a.duplicate_addon_mounts, 1);
    assert_eq!(
        a.mounts
            .iter()
            .find(|m| m.path == "citadel/addons")
            .unwrap()
            .key,
        "Game"
    );
    let out = ensure_text(&input, 1, true).unwrap();
    assert!(!out.contains(line));
    assert_eq!(out, ensure_text(&stock(), 1, true).unwrap());
}

#[test]
fn searchpaths_word_in_comment_is_ignored() {
    let input = text(COMMENT_FIRST);
    let a = analyze(&input);
    assert!(a.search_paths_found);
    let (start, end) = a.search_paths_body.unwrap();
    assert!(input[start..end].contains("citadel"));
    assert!(start > input.find("\t\tSearchPaths").unwrap());
    let out = ensure_text(&input, 1, true).unwrap();
    assert!(out.starts_with(&input[..input.find("\t\t\tGame\t\tcitadel").unwrap()]));
    assert!(out.contains("// SearchPaths { Game citadel/addons }"));
    assert!(analyze(&out).patched);
    assert_eq!(analyze(&out).addon_paths, vec!["citadel/addons"]);
}

#[test]
fn language_paths_are_preserved_and_game_language_never_added() {
    let out = ensure_text(&stock(), 1, true).unwrap();
    assert!(out.contains("\t\t\tGame_UILanguage\t\tcitadel_*LANGUAGE*\r\n"));
    assert!(out.contains("\t\t\tGame_LowViolence\tcitadel_lv\r\n"));
    assert!(!out.contains("Game_Language"));
    assert!(!analyze(&out).obsolete_game_language);

    let with_old = stock().replacen(
        CITADEL_LINE,
        &format!("\t\t\tGame_Language\tcitadel_english\r\n{CITADEL_LINE}"),
        1,
    );
    assert!(analyze(&with_old).obsolete_game_language);
    let out = ensure_text(&with_old, 1, true).unwrap();
    assert_eq!(count(&out, "Game_Language"), 1);
    let (_dir, file) = setup(with_old.as_bytes());
    assert!(
        file.status()
            .issues
            .iter()
            .any(|i| i.code == IssueCode::ObsoleteGameLanguage)
    );
}

#[test]
fn stock_status_is_not_patched_with_addons_not_mounted() {
    let (_dir, file) = setup(STOCK);
    let status = file.status();
    assert!(status.exists && status.readable);
    assert!(!status.patched);
    assert!(!status.has_bom);
    assert!(status.crlf);
    let codes: Vec<_> = status.issues.iter().map(|i| i.code).collect();
    assert_eq!(codes, vec![IssueCode::AddonsNotMounted]);
    assert_eq!(
        status.issues[0].message,
        "citadel/addons is not mounted; mods will not load"
    );
}

#[test]
fn missing_core_roles_report_keybinding_warning() {
    let partial = "\t\t\tGame citadel/addons\r\n\t\t\tMod citadel\r\n\t\t\tWrite citadel\r\n";
    let input = stock().replacen(CITADEL_LINE, &format!("{partial}{CITADEL_LINE}"), 1);
    let (_dir, file) = setup(input.as_bytes());
    let status = file.status();
    assert!(!status.patched);
    let issue = status
        .issues
        .iter()
        .find(|i| i.code == IssueCode::MissingModWrite)
        .unwrap();
    assert!(issue.message.starts_with("Missing Mod core, Write core; "));
    assert!(issue.message.contains("user_keys_default"));
    // ensure repairs it without duplicating the manual lines.
    let out = ensure_text(&input, 1, true).unwrap();
    assert_eq!(out, ensure_text(&stock(), 1, true).unwrap());
}

#[test]
fn addons_below_base_is_reported() {
    let input = stock().replacen(
        CITADEL_LINE,
        &format!("{CITADEL_LINE}\t\t\tGame citadel/addons\r\n"),
        1,
    );
    let (_dir, file) = setup(input.as_bytes());
    let codes: Vec<_> = file.status().issues.iter().map(|i| i.code).collect();
    assert!(codes.contains(&IssueCode::AddonsBelowBase));
    // Reapplying moves them above the base mount.
    file.ensure(1).unwrap();
    assert!(file.status().patched);
}

#[test]
fn duplicate_manual_mounts_are_reported_and_removed() {
    let good = ensure_text(&stock(), 1, true).unwrap();
    let input = good.replacen(
        CITADEL_LINE,
        &format!("\t\t\tGame citadel/addons\r\n{CITADEL_LINE}"),
        1,
    );
    let (_dir, file) = setup(input.as_bytes());
    let status = file.status();
    assert_eq!(status.analysis.as_ref().unwrap().duplicate_addon_mounts, 1);
    assert!(
        status
            .issues
            .iter()
            .any(|i| i.code == IssueCode::DuplicateAddonMounts)
    );
    file.ensure(1).unwrap();
    assert_eq!(text(&fs::read(file.path()).unwrap()), good);
}

#[test]
fn missing_file_status_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let file = GameInfoFile::for_game_root(dir.path());
    assert!(
        file.path().ends_with("game/citadel/gameinfo.gi")
            || file.path().ends_with("game\\citadel\\gameinfo.gi")
    );
    let status = file.status();
    assert!(!status.exists);
    assert_eq!(status.issues[0].code, IssueCode::Missing);
    assert_eq!(status.issues[0].message, "gameinfo.gi was not found");
    assert!(matches!(file.ensure(1), Err(GameInfoError::NotFound(_))));
}

#[test]
fn no_temp_files_are_left_behind() {
    let (dir, file) = setup(STOCK);
    file.ensure(1).unwrap();
    let names: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|n| !n.ends_with(".tmp")), "{names:?}");
}

/// Live, read-only check against a real install. Never writes.
#[test]
#[ignore = "needs KMM_LIVE_GAME pointing at a Deadlock install"]
fn live_gameinfo_is_analyzed_read_only() {
    let Some(root) = std::env::var_os("KMM_LIVE_GAME") else {
        eprintln!("KMM_LIVE_GAME is not set; skipping");
        return;
    };
    let path = std::path::Path::new(&root)
        .join("game")
        .join("citadel")
        .join("gameinfo.gi");
    let bytes = fs::read(&path).unwrap();
    let decoded = kombini_gameinfo::decode(&bytes);
    let analysis = analyze(&decoded.text);
    println!("path: {}", path.display());
    println!(
        "bom={} crlf={} encoding={:?}",
        decoded.bom, decoded.crlf, decoded.encoding
    );
    println!(
        "search_paths_found={} body={:?}",
        analysis.search_paths_found, analysis.search_paths_body
    );
    for m in &analysis.mounts {
        println!(
            "  line {:>4}: {:<18} {:<20} cond={} managed={} fence={}",
            m.line_index, m.key, m.path, m.conditional, m.in_managed_block, m.in_foreign_fence
        );
    }
    println!(
        "managed_block={:?} markers_ok={} addon_paths={:?} addons_above_citadel={} missing_roles={:?} duplicates={} other_manager={:?} obsolete_game_language={} patched={}",
        analysis.managed_block,
        analysis.markers_ok,
        analysis.addon_paths,
        analysis.addons_above_citadel,
        analysis.missing_roles,
        analysis.duplicate_addon_mounts,
        analysis.other_manager,
        analysis.obsolete_game_language,
        analysis.patched
    );
    assert!(analysis.search_paths_found);
    let once = ensure_text(&decoded.text, 3, decoded.crlf).unwrap();
    let twice = ensure_text(&once, 3, decoded.crlf).unwrap();
    assert_eq!(once, twice);
    assert!(analyze(&once).patched);
    assert_eq!(unpatch_text(&once), unpatch_text(&decoded.text));
}
