#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::Cell;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use kombini_archive::{
    ArchiveError, ArchiveKind, Backend, ExtractOptions, ExtractReport, Limits, backend_status,
    detect, detect_bytes, discover_vpks, extract, post_validate, sanitize_entry,
};
use sevenz_rust2::encoder_options::{AesEncoderOptions, Lzma2Options};
use sevenz_rust2::{ArchiveEntry, ArchiveWriter};
use tempfile::TempDir;
use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

const VPK_MAGIC: [u8; 4] = [0x34, 0x12, 0xAA, 0x55];
const MIB: u64 = 1024 * 1024;

// ---------------------------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------------------------

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn vpk_bytes() -> Vec<u8> {
    let mut data = VPK_MAGIC.to_vec();
    data.extend_from_slice(&[2, 0, 0, 0, 0, 0, 0, 0]);
    data
}

fn run_with(
    archive: &Path,
    dest: &Path,
    limits: Limits,
    cancel: &dyn Fn() -> bool,
) -> Result<ExtractReport, ArchiveError> {
    let progress = |_: u64, _: Option<u64>| {};
    extract(
        archive,
        dest,
        &ExtractOptions {
            limits,
            cancel,
            progress: &progress,
        },
    )
}

fn run_limits(archive: &Path, dest: &Path, limits: Limits) -> Result<ExtractReport, ArchiveError> {
    run_with(archive, dest, limits, &|| false)
}

fn run(archive: &Path, dest: &Path) -> Result<ExtractReport, ArchiveError> {
    let size = fs::metadata(archive).unwrap().len();
    run_limits(archive, dest, Limits::for_archive_size(size))
}

fn write_zip(path: &Path, entries: &[(&str, &[u8])], method: CompressionMethod) {
    let file = fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(method);
    for (name, data) in entries {
        zw.start_file(*name, options).unwrap();
        zw.write_all(data).unwrap();
    }
    zw.finish().unwrap();
}

fn write_7z(path: &Path, entries: &[(&str, &[u8])]) {
    let mut writer = ArchiveWriter::create(path).unwrap();
    for (name, data) in entries {
        writer
            .push_archive_entry(ArchiveEntry::new_file(name), Some(Cursor::new(*data)))
            .unwrap();
    }
    writer.finish().unwrap();
}

/// Sets the "encrypted" general-purpose flag (bit 0) in every local and central header.
fn mark_zip_encrypted(path: &Path) {
    let mut bytes = fs::read(path).unwrap();
    let mut i = 0;
    while i + 9 < bytes.len() {
        if &bytes[i..i + 4] == b"PK\x03\x04" {
            bytes[i + 6] |= 1;
        } else if &bytes[i..i + 4] == b"PK\x01\x02" {
            bytes[i + 8] |= 1;
        }
        i += 1;
    }
    fs::write(path, bytes).unwrap();
}

fn tmp() -> TempDir {
    tempfile::tempdir().unwrap()
}

/// Entries of `dir` (non-recursive) as a sorted name list; used to prove nothing escaped.
fn list_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// ---------------------------------------------------------------------------------------------
// 1. detection
// ---------------------------------------------------------------------------------------------

#[test]
fn detect_bytes_recognizes_every_magic() {
    assert_eq!(detect_bytes(b"PK\x03\x04rest"), ArchiveKind::Zip);
    assert_eq!(detect_bytes(b"PK\x05\x06"), ArchiveKind::Zip);
    assert_eq!(detect_bytes(b"PK\x07\x08"), ArchiveKind::Zip);
    assert_eq!(
        detect_bytes(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, 0, 4]),
        ArchiveKind::SevenZ
    );
    assert_eq!(detect_bytes(b"Rar!\x1A\x07\x00rest"), ArchiveKind::Rar);
    assert_eq!(detect_bytes(b"Rar!\x1A\x07\x01\x00"), ArchiveKind::Rar);
    assert_eq!(
        detect_bytes(&[0x34, 0x12, 0xAA, 0x55, 1, 2]),
        ArchiveKind::Vpk
    );
    // Near misses and garbage.
    assert_eq!(detect_bytes(b"Rar!\x1A\x07\x01"), ArchiveKind::Unknown);
    assert_eq!(detect_bytes(b"PK\x01\x02"), ArchiveKind::Unknown);
    assert_eq!(detect_bytes(b"PK"), ArchiveKind::Unknown);
    assert_eq!(detect_bytes(&[]), ArchiveKind::Unknown);
    assert_eq!(
        detect_bytes(b"this is not an archive"),
        ArchiveKind::Unknown
    );
}

#[test]
fn detect_ignores_the_extension() {
    let dir = tmp();
    let zip_named_rar = dir.path().join("mod.rar");
    write_zip(
        &zip_named_rar,
        &[("a.txt", b"hi")],
        CompressionMethod::Stored,
    );
    assert_eq!(detect(&zip_named_rar).unwrap(), ArchiveKind::Zip);

    let garbage = dir.path().join("mod.zip");
    fs::write(&garbage, b"definitely garbage").unwrap();
    assert_eq!(detect(&garbage).unwrap(), ArchiveKind::Unknown);

    let empty = dir.path().join("empty.7z");
    fs::write(&empty, b"").unwrap();
    assert_eq!(detect(&empty).unwrap(), ArchiveKind::Unknown);

    assert!(detect(&dir.path().join("missing")).is_err());
    assert_eq!(detect(&fixture("probe.rar")).unwrap(), ArchiveKind::Rar);
}

// ---------------------------------------------------------------------------------------------
// 2. sanitize_entry (public API)
// ---------------------------------------------------------------------------------------------

#[test]
fn sanitize_entry_public_contract() {
    assert_eq!(sanitize_entry("a/b.vpk").as_deref(), Some("a/b.vpk"));
    assert_eq!(sanitize_entry("./a/./b.vpk").as_deref(), Some("a/b.vpk"));
    assert_eq!(sanitize_entry("a\\b.vpk").as_deref(), Some("a/b.vpk"));
    for bad in [
        "../x",
        "a/../../x",
        "/abs",
        "C:/x",
        "a/C:x",
        "a/b:stream",
        "a/b./c",
        "a/b /c",
        "",
        ".",
    ] {
        assert_eq!(sanitize_entry(bad), None, "{bad:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// 3. ZIP
// ---------------------------------------------------------------------------------------------

#[test]
fn zip_normal_extract() {
    let dir = tmp();
    let archive = dir.path().join("mod.zip");
    let vpk = vpk_bytes();
    write_zip(
        &archive,
        &[
            ("Blue\\pak01_dir.vpk", &vpk),
            ("readme.txt", b"hello"),
            ("sub/", b""),
        ],
        CompressionMethod::Deflated,
    );
    let before = fs::read(&archive).unwrap();
    let dest = dir.path().join("out");
    let report = run(&archive, &dest).unwrap();
    assert_eq!(report.backend, Backend::Zip);
    assert_eq!(report.files, vec!["Blue/pak01_dir.vpk", "readme.txt"]);
    assert_eq!(report.bytes, vpk.len() as u64 + 5);
    assert!(report.nested.is_empty());
    assert!(report.warnings.is_empty());
    assert_eq!(fs::read(dest.join("readme.txt")).unwrap(), b"hello");
    assert!(dest.join("sub").is_dir());
    assert_eq!(
        fs::read(&archive).unwrap(),
        before,
        "source archive must be untouched"
    );
}

#[test]
fn zip_traversal_is_rejected_and_nothing_escapes() {
    let dir = tmp();
    let sandbox = dir.path().join("sandbox");
    fs::create_dir(&sandbox).unwrap();
    let archive = sandbox.join("evil.zip");
    write_zip(
        &archive,
        &[("ok.txt", b"fine"), ("../evil.txt", b"pwned")],
        CompressionMethod::Stored,
    );
    let dest = sandbox.join("out");
    let err = run(&archive, &dest).unwrap_err();
    assert!(
        matches!(err, ArchiveError::UnsafePath(ref p) if p == "../evil.txt"),
        "{err:?}"
    );
    assert!(!sandbox.join("evil.txt").exists());
    assert!(!dir.path().join("evil.txt").exists());
    // Validation happens before anything is written, so even the good entry is absent.
    assert!(list_names(&dest).is_empty());
}

#[test]
fn zip_absolute_and_drive_paths_are_rejected() {
    for name in ["/abs.txt", "C:/Windows/x.txt", "a/b:stream", "a/b./c.txt"] {
        let dir = tmp();
        let archive = dir.path().join("evil.zip");
        write_zip(&archive, &[(name, b"x")], CompressionMethod::Stored);
        let err = run(&archive, &dir.path().join("out")).unwrap_err();
        assert!(
            matches!(err, ArchiveError::UnsafePath(_)),
            "{name}: {err:?}"
        );
    }
}

#[test]
fn zip_bomb_is_stopped_while_writing() {
    let dir = tmp();
    let archive = dir.path().join("bomb.zip");
    {
        let file = fs::File::create(&archive).unwrap();
        let mut zw = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .large_file(true);
        zw.start_file("zeros.bin", options).unwrap();
        let chunk = vec![0u8; MIB as usize];
        for _ in 0..300 {
            zw.write_all(&chunk).unwrap();
        }
        zw.finish().unwrap();
    }
    let compressed = fs::metadata(&archive).unwrap().len();
    assert!(
        compressed < 10 * MIB,
        "bomb should compress well, got {compressed}"
    );

    let dest = dir.path().join("out");
    let started = Instant::now();
    let err = run_limits(
        &archive,
        &dest,
        Limits {
            max_bytes: 10 * MIB,
            max_entries: 100,
        },
    )
    .unwrap_err();
    let elapsed = started.elapsed();
    println!("zip bomb rejected in {elapsed:?}");
    assert!(
        matches!(err, ArchiveError::TooLarge { limit } if limit == 10 * MIB),
        "{err:?}"
    );
    let written = fs::metadata(dest.join("zeros.bin")).unwrap().len();
    assert!(written <= 10 * MIB, "wrote {written} bytes");
    assert!(elapsed.as_secs() < 20, "bomb handling took {elapsed:?}");
}

#[test]
fn zip_too_many_entries() {
    let dir = tmp();
    let archive = dir.path().join("many.zip");
    let names: Vec<String> = (0..20).map(|i| format!("f{i}.txt")).collect();
    let entries: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"x"[..])).collect();
    write_zip(&archive, &entries, CompressionMethod::Stored);
    let err = run_limits(
        &archive,
        &dir.path().join("out"),
        Limits {
            max_bytes: 10 * MIB,
            max_entries: 10,
        },
    )
    .unwrap_err();
    assert!(
        matches!(err, ArchiveError::TooManyEntries { limit: 10 }),
        "{err:?}"
    );
    assert!(list_names(&dir.path().join("out")).is_empty());
}

#[test]
fn zip_encrypted_entry_is_detected() {
    // The zip crate cannot write encrypted archives without extra features, so a normal archive is
    // patched: bit 0 of the general purpose flag marks an entry as encrypted.
    let dir = tmp();
    let archive = dir.path().join("secret.zip");
    write_zip(
        &archive,
        &[("a.txt", b"plain"), ("b.vpk", &vpk_bytes())],
        CompressionMethod::Stored,
    );
    mark_zip_encrypted(&archive);
    let err = run(&archive, &dir.path().join("out")).unwrap_err();
    assert!(matches!(err, ArchiveError::Encrypted), "{err:?}");
    assert_eq!(err.to_string(), "This archive is password-protected.");
}

#[test]
fn zip_symlink_entry_is_rejected() {
    let dir = tmp();
    let archive = dir.path().join("link.zip");
    {
        let file = fs::File::create(&archive).unwrap();
        let mut zw = zip::ZipWriter::new(file);
        zw.start_file("real.txt", SimpleFileOptions::default())
            .unwrap();
        zw.write_all(b"x").unwrap();
        zw.add_symlink("shortcut", "real.txt", SimpleFileOptions::default())
            .unwrap();
        zw.finish().unwrap();
    }
    let dest = dir.path().join("out");
    let err = run(&archive, &dest).unwrap_err();
    assert!(
        matches!(err, ArchiveError::Link(ref n) if n == "shortcut"),
        "{err:?}"
    );
    assert!(list_names(&dest).is_empty());
}

#[test]
fn zip_corrupt_archive_fails_cleanly() {
    let dir = tmp();
    let archive = dir.path().join("broken.zip");
    fs::write(&archive, b"PK\x03\x04 this is cut off").unwrap();
    let err = run(&archive, &dir.path().join("out")).unwrap_err();
    assert!(
        matches!(
            err,
            ArchiveError::Failed {
                backend: Backend::Zip,
                ..
            }
        ),
        "{err:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// 4. 7z
// ---------------------------------------------------------------------------------------------

#[test]
fn sevenz_normal_extract() {
    let dir = tmp();
    let archive = dir.path().join("mod.7z");
    let vpk = vpk_bytes();
    write_7z(
        &archive,
        &[
            ("Red/pak01_dir.vpk", &vpk),
            ("notes/readme.txt", b"hello 7z"),
        ],
    );
    let dest = dir.path().join("out");
    let report = run(&archive, &dest).unwrap();
    assert_eq!(report.backend, Backend::SevenZ);
    assert_eq!(report.files, vec!["Red/pak01_dir.vpk", "notes/readme.txt"]);
    assert_eq!(
        fs::read(dest.join("notes/readme.txt")).unwrap(),
        b"hello 7z"
    );
    assert_eq!(report.bytes, vpk.len() as u64 + 8);
}

#[test]
fn sevenz_traversal_name_is_rejected() {
    let dir = tmp();
    let sandbox = dir.path().join("sandbox");
    fs::create_dir(&sandbox).unwrap();
    let archive = sandbox.join("evil.7z");
    write_7z(&archive, &[("ok.txt", b"fine"), ("../evil.txt", b"pwned")]);
    let dest = sandbox.join("out");
    let err = run(&archive, &dest).unwrap_err();
    assert!(
        matches!(err, ArchiveError::UnsafePath(ref p) if p == "../evil.txt"),
        "{err:?}"
    );
    assert!(!sandbox.join("evil.txt").exists());
    assert!(!dir.path().join("evil.txt").exists());
    assert!(list_names(&dest).is_empty());
}

#[test]
fn sevenz_size_limit_is_enforced_while_writing() {
    let dir = tmp();
    let archive = dir.path().join("big.7z");
    let data = vec![0u8; 4 * MIB as usize];
    write_7z(&archive, &[("zeros.bin", &data)]);
    let err = run_limits(
        &archive,
        &dir.path().join("out"),
        Limits {
            max_bytes: MIB,
            max_entries: 10,
        },
    )
    .unwrap_err();
    assert!(
        matches!(err, ArchiveError::TooLarge { limit } if limit == MIB),
        "{err:?}"
    );
}

#[test]
fn sevenz_too_many_entries() {
    let dir = tmp();
    let archive = dir.path().join("many.7z");
    let names: Vec<String> = (0..20).map(|i| format!("f{i}.txt")).collect();
    let entries: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"x"[..])).collect();
    write_7z(&archive, &entries);
    let err = run_limits(
        &archive,
        &dir.path().join("out"),
        Limits {
            max_bytes: 10 * MIB,
            max_entries: 5,
        },
    )
    .unwrap_err();
    assert!(
        matches!(err, ArchiveError::TooManyEntries { limit: 5 }),
        "{err:?}"
    );
}

#[test]
fn sevenz_encrypted_is_detected() {
    let dir = tmp();
    let archive = dir.path().join("secret.7z");
    let mut writer = ArchiveWriter::create(&archive).unwrap();
    writer.set_content_methods(vec![
        AesEncoderOptions::new("hunter2".into()).into(),
        Lzma2Options::from_level(1).into(),
    ]);
    writer
        .push_archive_entry(
            ArchiveEntry::new_file("secret.vpk"),
            Some(Cursor::new(vpk_bytes())),
        )
        .unwrap();
    writer.finish().unwrap();
    let err = run(&archive, &dir.path().join("out")).unwrap_err();
    assert!(matches!(err, ArchiveError::Encrypted), "{err:?}");
}

// ---------------------------------------------------------------------------------------------
// 5. VPK passthrough, unknown, destination rules, cancellation
// ---------------------------------------------------------------------------------------------

#[test]
fn bare_vpk_is_copied_even_when_named_zip() {
    let dir = tmp();
    let archive = dir.path().join("x.zip");
    let data = vpk_bytes();
    fs::write(&archive, &data).unwrap();
    let dest = dir.path().join("out");
    let report = run(&archive, &dest).unwrap();
    assert_eq!(report.backend, Backend::Copy);
    assert_eq!(report.files, vec!["x.zip"]);
    assert_eq!(report.bytes, data.len() as u64);
    assert_eq!(fs::read(dest.join("x.zip")).unwrap(), data);
    assert!(fs::metadata(&archive).is_ok(), "source must not be deleted");
}

#[test]
fn unknown_format_is_unsupported_and_dest_untouched() {
    let dir = tmp();
    let archive = dir.path().join("mod.zip");
    fs::write(&archive, b"not an archive at all").unwrap();
    let dest = dir.path().join("out");
    let err = run(&archive, &dest).unwrap_err();
    assert!(
        matches!(err, ArchiveError::Unsupported(ref p) if p == &archive),
        "{err:?}"
    );
    assert!(!dest.exists());
}

#[test]
fn destination_must_be_empty() {
    let dir = tmp();
    let archive = dir.path().join("mod.zip");
    write_zip(&archive, &[("a.txt", b"x")], CompressionMethod::Stored);
    let dest = dir.path().join("out");
    fs::create_dir(&dest).unwrap();
    fs::write(dest.join("leftover.txt"), b"old").unwrap();
    let err = run(&archive, &dest).unwrap_err();
    assert!(
        matches!(err, ArchiveError::DestinationNotEmpty(_)),
        "{err:?}"
    );
    assert_eq!(list_names(&dest), vec!["leftover.txt"]);

    // An existing empty directory is fine, and so is a missing one (nested creation included).
    let empty = dir.path().join("empty");
    fs::create_dir(&empty).unwrap();
    assert!(run(&archive, &empty).is_ok());
    assert!(run(&archive, &dir.path().join("a").join("b").join("c")).is_ok());
}

#[test]
fn cancellation_returns_cancelled() {
    let dir = tmp();
    let archive = dir.path().join("mod.zip");
    write_zip(&archive, &[("a.txt", b"x")], CompressionMethod::Stored);
    let err = run_with(
        &archive,
        &dir.path().join("out"),
        Limits::for_archive_size(100),
        &|| true,
    )
    .unwrap_err();
    assert!(matches!(err, ArchiveError::Cancelled), "{err:?}");
}

#[test]
fn cancellation_between_entries_stops_extraction() {
    let dir = tmp();
    let archive = dir.path().join("mod.zip");
    write_zip(
        &archive,
        &[("a.txt", b"x"), ("b.txt", b"y"), ("c.txt", b"z")],
        CompressionMethod::Stored,
    );
    let polls = Cell::new(0u32);
    let cancel = || {
        polls.set(polls.get() + 1);
        polls.get() > 2
    };
    let dest = dir.path().join("out");
    let err = run_with(&archive, &dest, Limits::for_archive_size(100), &cancel).unwrap_err();
    assert!(matches!(err, ArchiveError::Cancelled), "{err:?}");
    assert!(list_names(&dest).len() < 3);
}

#[test]
fn progress_is_reported() {
    let dir = tmp();
    let archive = dir.path().join("big.zip");
    let chunk = vec![7u8; 3 * MIB as usize];
    write_zip(&archive, &[("big.bin", &chunk)], CompressionMethod::Stored);
    let last = Cell::new((0u64, None));
    let progress = |bytes: u64, total: Option<u64>| last.set((bytes, total));
    let cancel = || false;
    extract(
        &archive,
        &dir.path().join("out"),
        &ExtractOptions {
            limits: Limits::for_archive_size(10 * MIB),
            cancel: &cancel,
            progress: &progress,
        },
    )
    .unwrap();
    assert_eq!(last.get(), (3 * MIB, Some(3 * MIB)));
}

// ---------------------------------------------------------------------------------------------
// 6. nested archives
// ---------------------------------------------------------------------------------------------

#[test]
fn nested_archive_is_expanded_one_level() {
    let dir = tmp();
    let inner = dir.path().join("inner.7z");
    write_7z(&inner, &[("pak01_dir.vpk", &vpk_bytes())]);
    let inner_bytes = fs::read(&inner).unwrap();

    let outer = dir.path().join("outer.zip");
    write_zip(
        &outer,
        &[("Readme.txt", b"see inside"), ("inner.7z", &inner_bytes)],
        CompressionMethod::Stored,
    );
    let dest = dir.path().join("out");
    let report = run(&outer, &dest).unwrap();
    assert_eq!(report.backend, Backend::Zip);
    assert_eq!(report.nested, vec!["inner.7z"]);
    assert_eq!(
        report.files,
        vec!["Readme.txt", "inner__extracted/pak01_dir.vpk"]
    );
    assert!(
        !dest.join("inner.7z").exists(),
        "nested archive must be removed"
    );
    let vpks = discover_vpks(&dest);
    assert_eq!(vpks.len(), 1);
    assert_eq!(vpks[0].rel_path, "inner__extracted/pak01_dir.vpk");
    assert!(vpks[0].valid_signature);
}

#[test]
fn nested_archive_in_subfolder_keeps_its_folder() {
    let dir = tmp();
    let inner = dir.path().join("inner.zip");
    write_zip(
        &inner,
        &[("a.vpk", &vpk_bytes())],
        CompressionMethod::Stored,
    );
    let inner_bytes = fs::read(&inner).unwrap();
    let outer = dir.path().join("outer.zip");
    write_zip(
        &outer,
        &[("mods/skin.v2.zip", &inner_bytes)],
        CompressionMethod::Stored,
    );
    let dest = dir.path().join("out");
    let report = run(&outer, &dest).unwrap();
    assert_eq!(report.nested, vec!["mods/skin.v2.zip"]);
    assert_eq!(report.files, vec!["mods/skin.v2__extracted/a.vpk"]);
}

#[test]
fn nested_archives_are_not_touched_when_a_vpk_exists() {
    let dir = tmp();
    let inner = dir.path().join("inner.zip");
    write_zip(
        &inner,
        &[("b.vpk", &vpk_bytes())],
        CompressionMethod::Stored,
    );
    let inner_bytes = fs::read(&inner).unwrap();
    let outer = dir.path().join("outer.zip");
    write_zip(
        &outer,
        &[("main.vpk", &vpk_bytes()), ("extras.zip", &inner_bytes)],
        CompressionMethod::Stored,
    );
    let dest = dir.path().join("out");
    let report = run(&outer, &dest).unwrap();
    assert!(report.nested.is_empty());
    assert_eq!(report.files, vec!["extras.zip", "main.vpk"]);
}

#[test]
fn nested_expansion_is_only_one_level_deep() {
    let dir = tmp();
    let deepest = dir.path().join("deepest.zip");
    write_zip(
        &deepest,
        &[("c.vpk", &vpk_bytes())],
        CompressionMethod::Stored,
    );
    let middle = dir.path().join("middle.zip");
    write_zip(
        &middle,
        &[("deepest.zip", &fs::read(&deepest).unwrap())],
        CompressionMethod::Stored,
    );
    let outer = dir.path().join("outer.zip");
    write_zip(
        &outer,
        &[("middle.zip", &fs::read(&middle).unwrap())],
        CompressionMethod::Stored,
    );
    let dest = dir.path().join("out");
    let report = run(&outer, &dest).unwrap();
    assert_eq!(report.nested, vec!["middle.zip"]);
    assert_eq!(report.files, vec!["middle__extracted/deepest.zip"]);
}

#[test]
fn nested_archive_uses_the_remaining_budget() {
    let dir = tmp();
    let inner = dir.path().join("inner.zip");
    let big = vec![0u8; 2 * MIB as usize];
    write_zip(&inner, &[("big.bin", &big)], CompressionMethod::Deflated);
    let inner_bytes = fs::read(&inner).unwrap();
    let outer = dir.path().join("outer.zip");
    write_zip(
        &outer,
        &[("inner.zip", &inner_bytes)],
        CompressionMethod::Stored,
    );
    let err = run_limits(
        &outer,
        &dir.path().join("out"),
        Limits {
            max_bytes: MIB,
            max_entries: 100,
        },
    )
    .unwrap_err();
    assert!(matches!(err, ArchiveError::TooLarge { .. }), "{err:?}");
}

// ---------------------------------------------------------------------------------------------
// 7. discover_vpks, post_validate
// ---------------------------------------------------------------------------------------------

#[test]
fn discover_vpks_skips_junk_and_lists_variants() {
    let dir = tmp();
    let root = dir.path();
    let vpk = vpk_bytes();
    for rel in [
        "Blue/pak01_dir.vpk",
        "Red/pak01_dir.vpk",
        "__MACOSX/Blue/._pak01_dir.vpk",
        "__MACOSX/Blue/pak01_dir.vpk",
        ".hidden/pak01_dir.vpk",
        "Blue/.pak99_dir.vpk",
        "Upper/SKIN.VPK",
    ] {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, &vpk).unwrap();
    }
    fs::write(root.join("Blue").join("bad.vpk"), b"nope").unwrap();
    fs::write(root.join("Blue").join("Thumbs.db"), b"x").unwrap();
    fs::write(root.join("Blue").join("desktop.ini"), b"x").unwrap();
    fs::write(root.join("Blue").join("readme.txt"), b"x").unwrap();

    let found = discover_vpks(root);
    let rels: Vec<&str> = found.iter().map(|v| v.rel_path.as_str()).collect();
    assert_eq!(
        rels,
        vec![
            "Blue/bad.vpk",
            "Blue/pak01_dir.vpk",
            "Red/pak01_dir.vpk",
            "Upper/SKIN.VPK"
        ]
    );
    assert!(!found[0].valid_signature);
    assert!(found[1].valid_signature);
    assert_eq!(found[1].size, vpk.len() as u64);
}

#[test]
fn discover_vpks_is_not_confused_by_a_dotted_root_directory() {
    // Temp directories are often named like ".tmpAbC123"; only components below the root count.
    let dir = tempfile::Builder::new()
        .prefix(".kombini-")
        .tempdir()
        .unwrap();
    fs::write(dir.path().join("a.vpk"), vpk_bytes()).unwrap();
    assert_eq!(discover_vpks(dir.path()).len(), 1);
}

#[test]
fn post_validate_lists_files_sorted_with_total() {
    let dir = tmp();
    fs::create_dir_all(dir.path().join("b")).unwrap();
    fs::write(dir.path().join("b").join("z.bin"), [0u8; 10]).unwrap();
    fs::write(dir.path().join("a.bin"), [0u8; 5]).unwrap();
    let (files, total) = post_validate(dir.path()).unwrap();
    assert_eq!(files, vec!["a.bin", "b/z.bin"]);
    assert_eq!(total, 15);
}

#[test]
fn post_validate_rejects_symlinks() {
    let dir = tmp();
    let target = dir.path().join("target.txt");
    fs::write(&target, b"x").unwrap();
    let out = dir.path().join("out");
    fs::create_dir(&out).unwrap();
    let link = out.join("shortcut");
    #[cfg(windows)]
    let created = std::os::windows::fs::symlink_file(&target, &link);
    #[cfg(unix)]
    let created = std::os::unix::fs::symlink(&target, &link);
    if let Err(e) = created {
        println!("skipping: cannot create symlinks here ({e})");
        return;
    }
    let err = post_validate(&out).unwrap_err();
    assert!(
        matches!(err, ArchiveError::Link(ref n) if n == "shortcut"),
        "{err:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// 8. limits
// ---------------------------------------------------------------------------------------------

#[test]
fn limits_for_archive_size_clamps() {
    let small = Limits::for_archive_size(1024);
    assert_eq!(small.max_bytes, 256 * MIB);
    assert_eq!(small.max_entries, 10_000);
    assert_eq!(Limits::for_archive_size(MIB).max_bytes, 256 * MIB);
    assert_eq!(Limits::for_archive_size(5 * MIB).max_bytes, 500 * MIB);
    assert_eq!(
        Limits::for_archive_size(1024 * MIB).max_bytes,
        16 * 1024 * MIB
    );
}

// ---------------------------------------------------------------------------------------------
// 9. backend status
// ---------------------------------------------------------------------------------------------

#[test]
fn backend_status_reports_a_working_builtin_rar() {
    let status = backend_status();
    println!("{status:#?}");
    println!("{}", serde_json_like(&status));
    assert!(status.zip);
    assert!(status.sevenz);
    assert!(
        status.rar_builtin,
        "built-in UnRAR must be able to list the probe archive"
    );
    if cfg!(windows) {
        let path = status.system_tar.expect("Windows ships tar.exe");
        assert!(path.ends_with("tar.exe"));
        assert!(status.system_tar_version.is_some_and(|v| !v.is_empty()));
    }
}

/// Tiny debug rendering (the crate has no serde_json dependency).
fn serde_json_like(status: &kombini_archive::BackendStatus) -> String {
    format!(
        "zip={} sevenz={} rar_builtin={} system_tar={:?} version={:?}",
        status.zip, status.sevenz, status.rar_builtin, status.system_tar, status.system_tar_version
    )
}

// ---------------------------------------------------------------------------------------------
// 10. RAR with the built-in backend
// ---------------------------------------------------------------------------------------------

#[test]
fn rar_probe_extracts_with_builtin_backend() {
    let dir = tmp();
    let archive = dir.path().join("probe.rar");
    fs::copy(fixture("probe.rar"), &archive).unwrap();
    let dest = dir.path().join("out");
    let report = run(&archive, &dest).unwrap();
    assert_eq!(report.backend, Backend::UnrarBuiltin);
    assert!(report.warnings.is_empty());
    assert_eq!(report.files, vec!["VERSION"]);
    assert_eq!(
        fs::read_to_string(dest.join("VERSION")).unwrap(),
        "unrar-0.4.0"
    );
}

#[test]
fn rar_named_zip_is_still_extracted_as_rar() {
    let dir = tmp();
    let archive = dir.path().join("mislabeled.zip");
    fs::copy(fixture("probe.rar"), &archive).unwrap();
    let report = run(&archive, &dir.path().join("out")).unwrap();
    assert_eq!(report.backend, Backend::UnrarBuiltin);
}

#[test]
fn rar_encrypted_entry_is_detected() {
    let dir = tmp();
    let archive = dir.path().join("crypted.rar");
    fs::copy(fixture("crypted.rar"), &archive).unwrap();
    let err = run(&archive, &dir.path().join("out")).unwrap_err();
    assert!(matches!(err, ArchiveError::Encrypted), "{err:?}");
}

#[test]
fn rar_missing_volume_is_reported_by_name() {
    let dir = tmp();
    let archive = dir.path().join("100M.part00002.rar");
    fs::copy(fixture("100M.part00002.rar"), &archive).unwrap();
    let err = run(&archive, &dir.path().join("out")).unwrap_err();
    assert!(
        matches!(err, ArchiveError::MissingVolume(ref n) if n == "100M.part00001.rar"),
        "{err:?}"
    );
    assert!(err.to_string().contains("100M.part00001.rar"));
}

#[test]
fn rar_size_limit_is_enforced_from_headers_before_extracting() {
    let dir = tmp();
    let archive = dir.path().join("probe.rar");
    fs::copy(fixture("probe.rar"), &archive).unwrap();
    let dest = dir.path().join("out");
    let err = run_limits(
        &archive,
        &dest,
        Limits {
            max_bytes: 5,
            max_entries: 10,
        },
    )
    .unwrap_err();
    assert!(
        matches!(err, ArchiveError::TooLarge { limit: 5 }),
        "{err:?}"
    );
    assert!(list_names(&dest).is_empty(), "nothing may be extracted");
}

#[test]
fn rar_corrupt_archive_reports_both_errors() {
    let dir = tmp();
    let archive = dir.path().join("broken.rar");
    fs::write(&archive, b"Rar!\x1A\x07\x00 truncated and corrupt").unwrap();
    let err = run(&archive, &dir.path().join("out")).unwrap_err();
    match err {
        ArchiveError::Failed { backend, message } => {
            assert_eq!(backend, Backend::UnrarBuiltin);
            assert!(message.contains("fallback tar.exe:"), "{message}");
        }
        other => panic!("unexpected error {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// 11. live downloads (read-only inputs, copied first)
// ---------------------------------------------------------------------------------------------

fn live_rar_paths() -> Vec<PathBuf> {
    let Some(local) = std::env::var_os("LOCALAPPDATA") else {
        println!("LOCALAPPDATA is not set; nothing to test");
        return Vec::new();
    };
    let downloads = PathBuf::from(local)
        .join("Kombini")
        .join("KombiniModManager")
        .join("downloads");
    [
        "1854979_yamato_red_d0d08.rar",
        "1870920_reze_human_327e5.rar",
    ]
    .iter()
    .map(|name| downloads.join(name))
    .filter(|path| {
        let exists = path.is_file();
        if !exists {
            println!("missing live fixture {}", path.display());
        }
        exists
    })
    .collect()
}

#[test]
#[ignore = "uses real GameBanana downloads from the old app's folder (read-only)"]
fn live_real_rar_downloads_extract_and_contain_vpks() {
    let archives = live_rar_paths();
    assert!(!archives.is_empty(), "no live RAR downloads found");
    for original in archives {
        let dir = tmp();
        let archive = dir.path().join(original.file_name().unwrap());
        fs::copy(&original, &archive).unwrap();
        let dest = dir.path().join("out");
        let started = Instant::now();
        let report = run(&archive, &dest).unwrap();
        let elapsed = started.elapsed();
        let vpks = discover_vpks(&dest);
        println!("== {}", original.display());
        println!("   backend : {:?}", report.backend);
        println!("   elapsed : {elapsed:?}");
        println!("   bytes   : {}", report.bytes);
        println!("   warnings: {:?}", report.warnings);
        println!("   nested  : {:?}", report.nested);
        println!("   files   : {:#?}", report.files);
        println!("   vpks    : {vpks:#?}");
        assert!(!vpks.is_empty(), "expected at least one .vpk");
        assert!(vpks.iter().all(|v| v.valid_signature));
        assert_eq!(report.backend, Backend::UnrarBuiltin);
    }
}
