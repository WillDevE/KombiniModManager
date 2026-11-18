#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use kombini_vpk::*;
use md5::{Digest, Md5};
use tempfile::TempDir;

fn never() -> bool {
    false
}

fn crc(data: &[u8]) -> u32 {
    crc32fast::hash(data)
}

fn bytes_source(s: &str) -> EntrySource {
    EntrySource::Bytes(s.as_bytes().to_vec())
}

// ---------------------------------------------------------------------------
// Hand assembly helpers
// ---------------------------------------------------------------------------

struct Part {
    crc: u32,
    preload: Vec<u8>,
    archive_index: u16,
    offset: u32,
    length: u32,
}

struct RawFile {
    ext: &'static str,
    dir: &'static str,
    base: &'static str,
    parts: Vec<Part>,
}

fn raw_tree(files: &[RawFile]) -> Vec<u8> {
    let mut grouped: BTreeMap<&str, BTreeMap<&str, Vec<&RawFile>>> = BTreeMap::new();
    for f in files {
        let dir = if f.dir.is_empty() { " " } else { f.dir };
        grouped
            .entry(f.ext)
            .or_default()
            .entry(dir)
            .or_default()
            .push(f);
    }
    let mut t = Vec::new();
    for (ext, dirs) in grouped {
        t.extend_from_slice(ext.as_bytes());
        t.push(0);
        for (dir, files) in dirs {
            t.extend_from_slice(dir.as_bytes());
            t.push(0);
            for f in files {
                t.extend_from_slice(f.base.as_bytes());
                t.push(0);
                for (i, p) in f.parts.iter().enumerate() {
                    t.extend_from_slice(&p.crc.to_le_bytes());
                    t.extend_from_slice(&(p.preload.len() as u16).to_le_bytes());
                    t.extend_from_slice(&p.archive_index.to_le_bytes());
                    t.extend_from_slice(&p.offset.to_le_bytes());
                    t.extend_from_slice(&p.length.to_le_bytes());
                    let term: u16 = if i + 1 == f.parts.len() { 0xFFFF } else { 0 };
                    t.extend_from_slice(&term.to_le_bytes());
                    t.extend_from_slice(&p.preload);
                }
            }
            t.push(0);
        }
        t.push(0);
    }
    t.push(0);
    t
}

fn assemble(version: u32, tree: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0x55AA_1234u32.to_le_bytes());
    out.extend_from_slice(&version.to_le_bytes());
    out.extend_from_slice(&(tree.len() as u32).to_le_bytes());
    if version == 2 {
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0u8; 12]);
    }
    out.extend_from_slice(tree);
    out.extend_from_slice(data);
    out
}

fn embedded_part(data: &[u8], offset: u32) -> Part {
    Part {
        crc: crc(data),
        preload: Vec::new(),
        archive_index: EMBEDDED,
        offset,
        length: data.len() as u32,
    }
}

fn write_file(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, data).unwrap();
    p
}

fn sample_entries() -> Vec<(String, EntrySource)> {
    vec![
        (
            "models/heroes/inferno.vmdl_c".to_string(),
            bytes_source("inferno model bytes"),
        ),
        (
            "models/props/crate.vmdl_c".to_string(),
            bytes_source("crate"),
        ),
        (
            "materials/skin.vmat_c".to_string(),
            bytes_source("material data here, longer"),
        ),
        ("readme.txt".to_string(), bytes_source("hello")),
        ("empty.txt".to_string(), EntrySource::Bytes(Vec::new())),
    ]
}

fn sample_vpk(dir: &Path, name: &str) -> PathBuf {
    let out = dir.join(name);
    write_vpk(&out, sample_entries(), &never).unwrap();
    out
}

/// A v2 root plus one external chunk, with a preload entry, a split entry and an embedded entry.
/// Returns (root path, expected contents by path).
fn chunked_vpk(dir: &Path) -> (PathBuf, HashMap<&'static str, Vec<u8>>) {
    let chunk: Vec<u8> = b"AAAAABBBBBCCCCC".to_vec();
    let embedded = b"embedded!".to_vec();
    let files = vec![
        RawFile {
            ext: "txt",
            dir: "",
            base: "pre",
            parts: vec![Part {
                crc: crc(b"HEADCCCC"),
                preload: b"HEAD".to_vec(),
                archive_index: 0,
                offset: 11,
                length: 4,
            }],
        },
        RawFile {
            ext: "bin",
            dir: "sub",
            base: "split",
            parts: vec![
                Part {
                    crc: crc(b"AAAAABBBBBCCCCC"),
                    preload: Vec::new(),
                    archive_index: 0,
                    offset: 0,
                    length: 5,
                },
                Part {
                    crc: crc(b"AAAAABBBBBCCCCC"),
                    preload: Vec::new(),
                    archive_index: 0,
                    offset: 5,
                    length: 10,
                },
            ],
        },
        RawFile {
            ext: "txt",
            dir: "",
            base: "emb",
            parts: vec![embedded_part(&embedded, 0)],
        },
    ];
    // "pre.txt" = preload "HEAD" + chunk bytes 11..15 ("CCCC").
    let tree = raw_tree(&files);
    let root = write_file(dir, "foo_dir.vpk", &assemble(2, &tree, &embedded));
    write_file(dir, "foo_000.vpk", &chunk);
    let mut expected = HashMap::new();
    expected.insert("pre.txt", b"HEADCCCC".to_vec());
    expected.insert("sub/split.bin", chunk);
    expected.insert("emb.txt", embedded);
    (root, expected)
}

// ---------------------------------------------------------------------------
// Writer / reader round trips
// ---------------------------------------------------------------------------

#[test]
fn round_trip_write_open_read() {
    let tmp = TempDir::new().unwrap();
    let path = sample_vpk(tmp.path(), "out_dir.vpk");
    let archive = VpkArchive::open(&path).unwrap();
    assert_eq!(archive.version(), 2);
    assert_eq!(archive.entries().len(), 5);
    assert!(archive.chunk_indices().is_empty());
    assert!(archive.missing_chunks().is_empty());
    for (name, source) in sample_entries() {
        let EntrySource::Bytes(expected) = source else {
            unreachable!()
        };
        let got = archive.read_file(&name).unwrap();
        assert_eq!(got, expected, "{name}");
        let entry = archive.entries().iter().find(|e| e.path == name).unwrap();
        assert_eq!(entry.crc32, crc(&expected));
        assert!(entry.is_embedded());
        assert_eq!(entry.size(), expected.len() as u64);
        assert_eq!(archive.read_entry(entry).unwrap(), expected);
    }
    let report = archive.verify(&never).unwrap();
    assert_eq!(report.checked, 5);
    assert!(report.failures.is_empty());
    // Case-insensitive fallback.
    assert_eq!(archive.read_file("README.TXT").unwrap(), b"hello");
    assert!(archive.read_file("nope.txt").is_err());
    assert!(is_vpk(&path));
    assert!(!is_vpk(&tmp.path().join("missing.vpk")));
}

#[test]
fn write_from_file_and_from_vpk_sources() {
    let tmp = TempDir::new().unwrap();
    let src = sample_vpk(tmp.path(), "src_dir.vpk");
    let archive = Arc::new(VpkArchive::open(&src).unwrap());
    let loose = write_file(tmp.path(), "loose.bin", b"loose file contents");
    let out = tmp.path().join("mixed_dir.vpk");
    let report = write_vpk(
        &out,
        vec![
            (
                "copied/crate.vmdl_c".to_string(),
                EntrySource::FromVpk {
                    archive: archive.clone(),
                    path: "models/props/crate.vmdl_c".to_string(),
                },
            ),
            (
                "\\loose\\dir/file.bin".to_string(),
                EntrySource::File(loose),
            ),
            ("/lead.txt".to_string(), bytes_source("x")),
        ],
        &never,
    )
    .unwrap();
    assert_eq!(report.entries, 3);
    assert_eq!(report.bytes, std::fs::metadata(&out).unwrap().len());
    let a = VpkArchive::open(&out).unwrap();
    assert_eq!(a.read_file("copied/crate.vmdl_c").unwrap(), b"crate");
    assert_eq!(
        a.read_file("loose/dir/file.bin").unwrap(),
        b"loose file contents"
    );
    assert_eq!(a.read_file("lead.txt").unwrap(), b"x");
    assert!(a.verify(&never).unwrap().failures.is_empty());
}

#[test]
fn output_is_deterministic() {
    let tmp = TempDir::new().unwrap();
    let a = tmp.path().join("a_dir.vpk");
    let b = tmp.path().join("b_dir.vpk");
    write_vpk(&a, sample_entries(), &never).unwrap();
    let mut reversed = sample_entries();
    reversed.reverse();
    write_vpk(&b, reversed, &never).unwrap();
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
}

#[test]
fn other_md5_section_is_correct() {
    let tmp = TempDir::new().unwrap();
    let path = sample_vpk(tmp.path(), "md5_dir.vpk");
    let data = std::fs::read(&path).unwrap();
    let u32_at = |at: usize| u32::from_le_bytes(data[at..at + 4].try_into().unwrap());
    assert_eq!(u32_at(0), 0x55AA_1234);
    assert_eq!(u32_at(4), 2);
    let tree_size = u32_at(8) as usize;
    let data_size = u32_at(12) as usize;
    assert_eq!(u32_at(16), 0, "archive md5 section size");
    assert_eq!(u32_at(20), 48, "other md5 section size");
    assert_eq!(u32_at(24), 0, "signature section size");
    assert_eq!(data.len(), 28 + tree_size + data_size + 48);
    let tree = &data[28..28 + tree_size];
    let section = &data[28 + tree_size + data_size..];
    assert_eq!(section.len(), 48);
    assert_eq!(&section[..16], Md5::digest(tree).as_slice());
    assert_eq!(&section[16..32], Md5::digest([]).as_slice());
    let mut whole = Md5::new();
    whole.update(&data[..data.len() - 16]);
    assert_eq!(&section[32..], whole.finalize().as_slice());
}

#[test]
fn write_validation_errors() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("x_dir.vpk");
    let err = write_vpk(&out, Vec::new(), &never).unwrap_err();
    assert!(matches!(&err, VpkError::Invalid(m) if m == "cannot write an empty VPK"));
    let err = write_vpk(
        &out,
        vec![
            ("a/b.txt".into(), bytes_source("1")),
            ("a\\b.txt".into(), bytes_source("2")),
        ],
        &never,
    )
    .unwrap_err();
    assert!(matches!(err, VpkError::Collision(p) if p == "a/b.txt"));
    let err = write_vpk(&out, vec![("noext".into(), bytes_source("1"))], &never).unwrap_err();
    assert!(matches!(err, VpkError::Invalid(_)));
    let err = write_vpk(&out, vec![("dir/trail.".into(), bytes_source("1"))], &never).unwrap_err();
    assert!(matches!(err, VpkError::Invalid(_)));
    let err = write_vpk(&out, vec![("../x.txt".into(), bytes_source("1"))], &never).unwrap_err();
    assert!(matches!(err, VpkError::UnsafePath(_)));
    let err = write_vpk(
        &out,
        vec![(
            "a.txt".into(),
            EntrySource::File(tmp.path().join("missing.bin")),
        )],
        &never,
    )
    .unwrap_err();
    assert!(matches!(err, VpkError::Io { .. }));
    assert!(!out.exists());
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[test]
fn write_cancel_leaves_no_files() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("c_dir.vpk");
    let calls = AtomicUsize::new(0);
    let cancel = || calls.fetch_add(1, Ordering::SeqCst) >= 3;
    let err = write_vpk(&out, sample_entries(), &cancel).unwrap_err();
    assert!(matches!(err, VpkError::Cancelled));
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[test]
fn overwrites_existing_output() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("o_dir.vpk");
    write_vpk(&out, vec![("a.txt".into(), bytes_source("one"))], &never).unwrap();
    write_vpk(&out, vec![("a.txt".into(), bytes_source("two"))], &never).unwrap();
    assert_eq!(
        VpkArchive::open(&out).unwrap().read_file("a.txt").unwrap(),
        b"two"
    );
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
}

#[test]
fn reads_v1() {
    let tmp = TempDir::new().unwrap();
    let data = b"v1 payload";
    let files = vec![RawFile {
        ext: "txt",
        dir: "docs",
        base: "one",
        parts: vec![embedded_part(data, 0)],
    }];
    let tree = raw_tree(&files);
    let path = write_file(tmp.path(), "old_dir.vpk", &assemble(1, &tree, data));
    let a = VpkArchive::open(&path).unwrap();
    assert_eq!(a.version(), 1);
    assert_eq!(a.entries().len(), 1);
    assert_eq!(a.entries()[0].path, "docs/one.txt");
    assert_eq!(a.read_file("docs/one.txt").unwrap(), data);
    assert!(a.verify(&never).unwrap().failures.is_empty());
}

#[test]
fn reads_chunks_preload_and_split_entries() {
    let tmp = TempDir::new().unwrap();
    let (root, expected) = chunked_vpk(tmp.path());
    let a = VpkArchive::open(&root).unwrap();
    assert_eq!(a.chunk_indices(), &[0]);
    assert_eq!(a.chunk_path(0), tmp.path().join("foo_000.vpk"));
    assert!(a.missing_chunks().is_empty());
    assert_eq!(a.entries().len(), 4);
    assert_eq!(a.file_paths().len(), 3);
    for (path, bytes) in &expected {
        assert_eq!(&a.read_file(path).unwrap(), bytes, "{path}");
    }
    let pre = a.entries().iter().find(|e| e.path == "pre.txt").unwrap();
    assert_eq!(pre.preload_len, 4);
    assert_eq!(pre.size(), 8);
    assert!(!pre.is_embedded());
    assert_eq!(a.read_entry(pre).unwrap(), b"HEADCCCC");
    let report = a.verify(&never).unwrap();
    assert_eq!(report.checked, 3);
    assert!(report.failures.is_empty(), "{:?}", report.failures);

    // Merged output drops chunk files and preload structure but keeps the bytes.
    let idx = VpkIndex::new(VpkArchive::open(&root).unwrap());
    assert_eq!(idx.len(), 3);
    let page = idx.query(&EntryQuery {
        filter: "split".into(),
        offset: 0,
        limit: 10,
        sort: SortKey::Path,
        descending: false,
    });
    assert_eq!(page.rows[0].size, 15);
    assert_eq!(page.rows[0].archive_index, Some(0));

    // Missing chunk is detected and reported as such when read.
    std::fs::remove_file(tmp.path().join("foo_000.vpk")).unwrap();
    let a = VpkArchive::open(&root).unwrap();
    assert_eq!(a.missing_chunks(), vec![tmp.path().join("foo_000.vpk")]);
    let err = a.read_file("sub/split.bin").unwrap_err();
    assert!(matches!(err, VpkError::MissingChunk { ref chunk, .. } if chunk == "foo_000.vpk"));
    assert_eq!(a.verify(&never).unwrap().failures.len(), 2);
    assert_eq!(a.read_file("emb.txt").unwrap(), b"embedded!");
}

#[test]
fn truncated_chunk_is_reported() {
    let tmp = TempDir::new().unwrap();
    let (root, _) = chunked_vpk(tmp.path());
    write_file(tmp.path(), "foo_000.vpk", b"AAAAAB");
    let a = VpkArchive::open(&root).unwrap();
    let err = a.read_file("sub/split.bin").unwrap_err();
    assert!(matches!(err, VpkError::Truncated { .. }), "{err}");
}

#[test]
fn split_entry_with_fragment_crcs_verifies() {
    let tmp = TempDir::new().unwrap();
    let files = vec![RawFile {
        ext: "bin",
        dir: "",
        base: "frag",
        parts: vec![
            Part {
                crc: crc(b"one"),
                preload: Vec::new(),
                archive_index: EMBEDDED,
                offset: 0,
                length: 3,
            },
            Part {
                crc: crc(b"two"),
                preload: Vec::new(),
                archive_index: EMBEDDED,
                offset: 3,
                length: 3,
            },
        ],
    }];
    let path = write_file(
        tmp.path(),
        "f_dir.vpk",
        &assemble(2, &raw_tree(&files), b"onetwo"),
    );
    let a = VpkArchive::open(&path).unwrap();
    assert_eq!(a.read_file("frag.bin").unwrap(), b"onetwo");
    assert!(a.verify(&never).unwrap().failures.is_empty());
}

// ---------------------------------------------------------------------------
// Malformed input
// ---------------------------------------------------------------------------

fn valid_bytes() -> Vec<u8> {
    let tmp = TempDir::new().unwrap();
    std::fs::read(sample_vpk(tmp.path(), "v_dir.vpk")).unwrap()
}

fn open_bytes(bytes: &[u8]) -> Result<VpkArchive> {
    let tmp = TempDir::new().unwrap();
    let p = write_file(tmp.path(), "m_dir.vpk", bytes);
    VpkArchive::open(&p)
}

#[test]
fn malformed_headers() {
    let valid = valid_bytes();
    assert!(open_bytes(&valid).is_ok());

    assert!(matches!(
        open_bytes(&valid[..8]).unwrap_err(),
        VpkError::TooSmall { .. }
    ));
    assert!(matches!(
        open_bytes(&[]).unwrap_err(),
        VpkError::TooSmall { .. }
    ));

    let mut bad_sig = valid.clone();
    bad_sig[0] ^= 0xFF;
    assert!(matches!(
        open_bytes(&bad_sig).unwrap_err(),
        VpkError::BadSignature { .. }
    ));

    let mut bad_ver = valid.clone();
    bad_ver[4] = 9;
    assert!(matches!(
        open_bytes(&bad_ver).unwrap_err(),
        VpkError::UnsupportedVersion { version: 9, .. }
    ));

    // v2 header cut short (valid first 12 bytes, 20 bytes total).
    assert!(matches!(
        open_bytes(&valid[..20]).unwrap_err(),
        VpkError::TooSmall { .. }
    ));

    // Truncated tree.
    assert!(matches!(
        open_bytes(&valid[..40]).unwrap_err(),
        VpkError::Malformed { .. }
    ));

    // tree_size larger than the file.
    let mut huge = valid.clone();
    huge[8..12].copy_from_slice(&(valid.len() as u32 + 1000).to_le_bytes());
    assert!(matches!(
        open_bytes(&huge).unwrap_err(),
        VpkError::Malformed { .. }
    ));

    // tree_size zero and over the limit.
    let mut zero = valid.clone();
    zero[8..12].copy_from_slice(&0u32.to_le_bytes());
    assert!(matches!(
        open_bytes(&zero).unwrap_err(),
        VpkError::Malformed { .. }
    ));
    let mut over = valid.clone();
    over[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        open_bytes(&over).unwrap_err(),
        VpkError::Malformed { .. }
    ));
}

#[test]
fn malformed_trees() {
    // Unterminated string.
    let bytes = assemble(2, b"txtnoterminator", b"");
    assert!(matches!(
        open_bytes(&bytes).unwrap_err(),
        VpkError::Malformed { .. }
    ));

    // Entry cut off.
    let bytes = assemble(2, b"txt\0 \0name\0\x01\x02\x03", b"");
    assert!(matches!(
        open_bytes(&bytes).unwrap_err(),
        VpkError::Malformed { .. }
    ));

    // Bad terminator.
    let mut tree = raw_tree(&[RawFile {
        ext: "txt",
        dir: "",
        base: "a",
        parts: vec![embedded_part(b"x", 0)],
    }]);
    let term_at = tree.iter().position(|&b| b == b'a').unwrap() + 2 + 16;
    assert_eq!(&tree[term_at..term_at + 2], &[0xFF, 0xFF]);
    tree[term_at] = 0x34;
    tree[term_at + 1] = 0x12;
    assert!(matches!(
        open_bytes(&assemble(2, &tree, b"x")).unwrap_err(),
        VpkError::Malformed { reason, .. } if reason.contains("terminator")
    ));

    // Preload runs past the tree.
    let mut tree = raw_tree(&[RawFile {
        ext: "txt",
        dir: "",
        base: "a",
        parts: vec![embedded_part(b"x", 0)],
    }]);
    let preload_at = tree.iter().position(|&b| b == b'a').unwrap() + 2 + 4;
    tree[preload_at] = 200;
    assert!(matches!(
        open_bytes(&assemble(2, &tree, b"x")).unwrap_err(),
        VpkError::Malformed { .. }
    ));

    // Embedded data past EOF.
    let tree = raw_tree(&[RawFile {
        ext: "txt",
        dir: "",
        base: "a",
        parts: vec![Part {
            crc: 0,
            preload: Vec::new(),
            archive_index: EMBEDDED,
            offset: 0,
            length: 100,
        }],
    }]);
    assert!(matches!(
        open_bytes(&assemble(2, &tree, b"abc")).unwrap_err(),
        VpkError::Malformed { reason, .. } if reason.contains("past end of file")
    ));
}

/// Real-world shape (GameBanana "Vodka - Bebop"): an empty readme.txt with
/// an embedded placeholder offset of 0xFFFFFFFF next to normal entries.
#[test]
fn zero_length_entry_with_placeholder_offset_opens() {
    let data = b"hello";
    let tree = raw_tree(&[
        RawFile {
            ext: "txt",
            dir: "",
            base: "readme",
            parts: vec![Part {
                crc: 0,
                preload: Vec::new(),
                archive_index: EMBEDDED,
                offset: u32::MAX,
                length: 0,
            }],
        },
        RawFile {
            ext: "txt",
            dir: "d",
            base: "a",
            parts: vec![embedded_part(data, 0)],
        },
    ]);
    let tmp = TempDir::new().unwrap();
    let p = write_file(tmp.path(), "pak02_dir.vpk", &assemble(2, &tree, data));
    let archive = VpkArchive::open(&p).unwrap();
    assert_eq!(archive.read_file("readme.txt").unwrap(), b"");
    assert_eq!(archive.read_file("d/a.txt").unwrap(), data);
    assert!(archive.verify(&never).unwrap().failures.is_empty());
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

#[test]
fn random_mutations_never_panic() {
    let tmp = TempDir::new().unwrap();
    let (chunk_root, _) = chunked_vpk(tmp.path());
    let bases: Vec<Vec<u8>> = vec![valid_bytes(), std::fs::read(&chunk_root).unwrap()];
    let work = TempDir::new().unwrap();
    // The mutated chunked root needs its chunk sibling.
    write_file(work.path(), "m_000.vpk", b"AAAAABBBBBCCCCC");
    let path = work.path().join("m_dir.vpk");
    let mut rng = Lcg(0x1234_5678_9ABC_DEF0);
    let mut opened = 0usize;
    for i in 0..2000 {
        let mut bytes = bases[i % 2].clone();
        let flips = 1 + (rng.next() % 3) as usize;
        for _ in 0..flips {
            let at = (rng.next() as usize) % bytes.len();
            bytes[at] = rng.next() as u8;
        }
        if rng.next().is_multiple_of(8) {
            let cut = (rng.next() as usize) % bytes.len();
            bytes.truncate(cut);
        }
        std::fs::write(&path, &bytes).unwrap();
        if let Ok(a) = VpkArchive::open(&path) {
            opened += 1;
            for e in a.entries() {
                let _ = a.read_entry(e);
            }
            let _ = a.read_file("a.txt");
            let _ = a.verify(&never);
            let _ = a.missing_chunks();
            let idx = VpkIndex::new(a);
            let _ = idx.folders("");
            let _ = idx.query(&EntryQuery {
                filter: "t".into(),
                offset: 0,
                limit: 10,
                sort: SortKey::Size,
                descending: true,
            });
        }
    }
    assert!(opened > 0, "mutations should sometimes still parse");
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

fn extract_all(archive: &VpkArchive, dest: &Path, verify_crc: bool) -> Result<Vec<PathBuf>> {
    extract(
        archive,
        dest,
        &ExtractOptions {
            selected: None,
            verify_crc,
            cancel: &never,
            progress: &|_, _| {},
        },
    )
}

#[test]
fn extract_writes_files_and_reports_progress() {
    let tmp = TempDir::new().unwrap();
    let archive = VpkArchive::open(&sample_vpk(tmp.path(), "e_dir.vpk")).unwrap();
    let dest = tmp.path().join("out");
    let ticks = std::sync::Mutex::new(Vec::new());
    let written = extract(
        &archive,
        &dest,
        &ExtractOptions {
            selected: None,
            verify_crc: true,
            cancel: &never,
            progress: &|d, t| ticks.lock().unwrap().push((d, t)),
        },
    )
    .unwrap();
    assert_eq!(written.len(), 5);
    assert_eq!(
        std::fs::read(dest.join("models/heroes/inferno.vmdl_c")).unwrap(),
        b"inferno model bytes"
    );
    assert_eq!(std::fs::read(dest.join("readme.txt")).unwrap(), b"hello");
    assert_eq!(std::fs::read(dest.join("empty.txt")).unwrap(), b"");
    let ticks = ticks.into_inner().unwrap();
    assert_eq!(ticks.last(), Some(&(5, 5)));
    assert_eq!(ticks.len(), 5);

    // Selected subset.
    let dest2 = tmp.path().join("out2");
    let selected = vec!["readme.txt".to_string()];
    let written = extract(
        &archive,
        &dest2,
        &ExtractOptions {
            selected: Some(&selected),
            verify_crc: true,
            cancel: &never,
            progress: &|_, _| {},
        },
    )
    .unwrap();
    assert_eq!(written, vec![dest2.join("readme.txt")]);

    // Cancel.
    let err = extract(
        &archive,
        &tmp.path().join("out3"),
        &ExtractOptions {
            selected: None,
            verify_crc: true,
            cancel: &|| true,
            progress: &|_, _| {},
        },
    )
    .unwrap_err();
    assert!(matches!(err, VpkError::Cancelled));
}

#[test]
fn extract_chunked_archive() {
    let tmp = TempDir::new().unwrap();
    let (root, expected) = chunked_vpk(tmp.path());
    let a = VpkArchive::open(&root).unwrap();
    let dest = tmp.path().join("out");
    extract_all(&a, &dest, true).unwrap();
    for (path, bytes) in expected {
        assert_eq!(std::fs::read(dest.join(path)).unwrap(), bytes, "{path}");
    }
}

#[test]
fn extract_rejects_traversal_before_writing() {
    let tmp = TempDir::new().unwrap();
    let files = vec![
        RawFile {
            ext: "txt",
            dir: "..",
            base: "x",
            parts: vec![embedded_part(b"evil", 0)],
        },
        RawFile {
            ext: "txt",
            dir: "",
            base: "fine",
            parts: vec![embedded_part(b"ok", 4)],
        },
    ];
    let path = write_file(
        tmp.path(),
        "evil_dir.vpk",
        &assemble(2, &raw_tree(&files), b"evilok"),
    );
    let a = VpkArchive::open(&path).unwrap();
    assert!(a.entries().iter().any(|e| e.path == "../x.txt"));
    let dest = tmp.path().join("dest");
    let err = extract_all(&a, &dest, true).unwrap_err();
    assert!(
        matches!(err, VpkError::UnsafePath(ref p) if p == "../x.txt"),
        "{err}"
    );
    assert!(!tmp.path().join("x.txt").exists());
    assert!(!dest.join("fine.txt").exists());
}

#[test]
fn extract_crc_mismatch() {
    let tmp = TempDir::new().unwrap();
    let files = vec![RawFile {
        ext: "txt",
        dir: "",
        base: "bad",
        parts: vec![Part {
            crc: 0xDEAD_BEEF,
            preload: Vec::new(),
            archive_index: EMBEDDED,
            offset: 0,
            length: 4,
        }],
    }];
    let path = write_file(
        tmp.path(),
        "crc_dir.vpk",
        &assemble(2, &raw_tree(&files), b"data"),
    );
    let a = VpkArchive::open(&path).unwrap();
    let err = extract_all(&a, &tmp.path().join("o1"), true).unwrap_err();
    assert!(matches!(
        err,
        VpkError::CrcMismatch { expected: 0xDEAD_BEEF, actual, .. } if actual == crc(b"data")
    ));
    assert!(!tmp.path().join("o1/bad.txt").exists());
    // Without verification the data is written.
    extract_all(&a, &tmp.path().join("o2"), false).unwrap();
    assert_eq!(
        std::fs::read(tmp.path().join("o2/bad.txt")).unwrap(),
        b"data"
    );
    // verify() reports it.
    let report = a.verify(&never).unwrap();
    assert_eq!(report.failures, vec!["bad.txt".to_string()]);
    assert!(matches!(
        a.verify(&|| true).unwrap_err(),
        VpkError::Cancelled
    ));
}

// ---------------------------------------------------------------------------
// Merge
// ---------------------------------------------------------------------------

struct MergeFixture {
    tmp: TempDir,
    a: PathBuf,
    b: PathBuf,
}

fn merge_fixture() -> MergeFixture {
    let tmp = TempDir::new().unwrap();
    let a = tmp.path().join("a_dir.vpk");
    let b = tmp.path().join("b_dir.vpk");
    write_vpk(
        &a,
        vec![
            ("a.txt".into(), bytes_source("A-a")),
            ("b.txt".into(), bytes_source("A-b")),
        ],
        &never,
    )
    .unwrap();
    write_vpk(
        &b,
        vec![
            ("b.txt".into(), bytes_source("B-b")),
            ("c.txt".into(), bytes_source("B-c")),
        ],
        &never,
    )
    .unwrap();
    MergeFixture { tmp, a, b }
}

fn opts(policy: CollisionPolicy) -> MergeOptions {
    MergeOptions {
        policy,
        overrides: HashMap::new(),
        extra_files: Vec::new(),
    }
}

fn merged_map(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let a = VpkArchive::open(path).unwrap();
    a.file_paths()
        .into_iter()
        .map(|p| (p.to_string(), a.read_file(p).unwrap()))
        .collect()
}

fn expect(pairs: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
        .collect()
}

#[test]
fn merge_last_first_error() {
    let f = merge_fixture();
    let inputs = vec![f.a.clone(), f.b.clone()];
    let out = f.tmp.path().join("m_dir.vpk");

    let r = merge_vpks(&inputs, &out, &opts(CollisionPolicy::Last), &never).unwrap();
    assert_eq!(r.total_entries, 3);
    assert_eq!(r.inputs, 2);
    assert!(!r.strict);
    assert_eq!(r.overridden_paths, vec!["b.txt".to_string()]);
    assert_eq!(
        merged_map(&out),
        expect(&[("a.txt", "A-a"), ("b.txt", "B-b"), ("c.txt", "B-c")])
    );

    let r = merge_vpks(&inputs, &out, &opts(CollisionPolicy::First), &never).unwrap();
    assert_eq!(r.total_entries, 3);
    assert!(r.overridden_paths.is_empty());
    assert_eq!(
        merged_map(&out),
        expect(&[("a.txt", "A-a"), ("b.txt", "A-b"), ("c.txt", "B-c")])
    );

    let err = merge_vpks(&inputs, &out, &opts(CollisionPolicy::Error), &never).unwrap_err();
    assert!(matches!(err, VpkError::Collision(p) if p == "b.txt"));

    // Error policy without collisions works and is strict.
    let r = merge_vpks(&inputs[..1], &out, &opts(CollisionPolicy::Error), &never).unwrap();
    assert!(r.strict);
    assert_eq!(r.total_entries, 2);

    let err = merge_vpks(&[], &out, &opts(CollisionPolicy::Last), &never).unwrap_err();
    assert!(matches!(err, VpkError::Invalid(_)));
}

#[test]
fn merge_overrides() {
    let f = merge_fixture();
    let inputs = vec![f.a.clone(), f.b.clone()];
    let out = f.tmp.path().join("m_dir.vpk");

    // Override to the earlier input beats the "last wins" policy and records no override.
    let mut o = opts(CollisionPolicy::Last);
    o.overrides.insert("b.txt".into(), 0);
    let r = merge_vpks(&inputs, &out, &o, &never).unwrap();
    assert_eq!(merged_map(&out)["b.txt"], b"A-b");
    assert!(r.overridden_paths.is_empty());

    // Override to the later input beats "first wins" (the earlier input skips the path).
    let mut o = opts(CollisionPolicy::First);
    o.overrides.insert("b.txt".into(), 1);
    let r = merge_vpks(&inputs, &out, &o, &never).unwrap();
    assert_eq!(merged_map(&out)["b.txt"], b"B-b");
    assert!(r.overridden_paths.is_empty());

    // Override also resolves a collision under the Error policy.
    let mut o = opts(CollisionPolicy::Error);
    o.overrides.insert("b.txt".into(), 1);
    merge_vpks(&inputs, &out, &o, &never).unwrap();
    assert_eq!(merged_map(&out)["b.txt"], b"B-b");
}

#[test]
fn merge_extra_files() {
    let f = merge_fixture();
    let inputs = vec![f.a.clone(), f.b.clone()];
    let out = f.tmp.path().join("m_dir.vpk");
    let extra = write_file(f.tmp.path(), "extra.bin", b"EXTRA");
    let extras = || {
        vec![
            ("b.txt".to_string(), extra.clone()),
            ("new/d.txt".to_string(), extra.clone()),
        ]
    };

    let mut o = opts(CollisionPolicy::Last);
    o.extra_files = extras();
    let r = merge_vpks(&inputs, &out, &o, &never).unwrap();
    assert_eq!(r.total_entries, 4);
    assert_eq!(r.overridden_paths, vec!["b.txt".to_string()]);
    assert_eq!(
        merged_map(&out),
        expect(&[
            ("a.txt", "A-a"),
            ("b.txt", "EXTRA"),
            ("c.txt", "B-c"),
            ("new/d.txt", "EXTRA")
        ])
    );

    let mut o = opts(CollisionPolicy::First);
    o.extra_files = extras();
    let r = merge_vpks(&inputs, &out, &o, &never).unwrap();
    assert_eq!(r.overridden_paths, vec!["b.txt".to_string()]);
    assert_eq!(merged_map(&out)["b.txt"], b"A-b");
    assert_eq!(merged_map(&out)["new/d.txt"], b"EXTRA");

    let mut o = opts(CollisionPolicy::Error);
    o.extra_files = extras();
    let err = merge_vpks(&inputs[..1], &out, &o, &never).unwrap_err();
    assert!(matches!(err, VpkError::Collision(p) if p == "b.txt"));

    let mut o = opts(CollisionPolicy::Last);
    o.extra_files = vec![("../evil.txt".to_string(), extra.clone())];
    let err = merge_vpks(&inputs, &out, &o, &never).unwrap_err();
    assert!(matches!(err, VpkError::UnsafePath(_)));
}

#[test]
fn merge_chunked_input() {
    let tmp = TempDir::new().unwrap();
    let (root, expected) = chunked_vpk(tmp.path());
    let out = tmp.path().join("merged_dir.vpk");
    let r = merge_vpks(&[root], &out, &opts(CollisionPolicy::Last), &never).unwrap();
    assert_eq!(r.total_entries, 3);
    let m = merged_map(&out);
    for (path, bytes) in expected {
        assert_eq!(m[path], bytes);
    }
}

// ---------------------------------------------------------------------------
// Query
// ---------------------------------------------------------------------------

fn query_index(tmp: &Path) -> VpkIndex {
    let out = tmp.join("q_dir.vpk");
    write_vpk(
        &out,
        vec![
            (
                "models/heroes/inferno.vmdl_c".into(),
                EntrySource::Bytes(vec![0; 300]),
            ),
            (
                "models/heroes/haze.vmdl_c".into(),
                EntrySource::Bytes(vec![1; 100]),
            ),
            (
                "models/props/crate.vmdl_c".into(),
                EntrySource::Bytes(vec![2; 200]),
            ),
            (
                "materials/Heroes_Skin.vmat_c".into(),
                EntrySource::Bytes(vec![3; 50]),
            ),
            ("panorama/hero.png".into(), EntrySource::Bytes(vec![4; 10])),
            ("root.txt".into(), EntrySource::Bytes(vec![5; 5])),
        ],
        &never,
    )
    .unwrap();
    VpkIndex::new(VpkArchive::open(&out).unwrap())
}

fn q(filter: &str, offset: usize, limit: usize, sort: SortKey, descending: bool) -> EntryQuery {
    EntryQuery {
        filter: filter.into(),
        offset,
        limit,
        sort,
        descending,
    }
}

#[test]
fn query_filter_sort_and_paging() {
    let tmp = TempDir::new().unwrap();
    let idx = query_index(tmp.path());
    assert_eq!(idx.len(), 6);
    assert!(!idx.is_empty());
    assert_eq!(idx.archive().entries().len(), 6);

    let all = idx.query(&q("", 0, 100, SortKey::Path, false));
    assert_eq!(all.total, 6);
    let paths: Vec<_> = all.rows.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "materials/Heroes_Skin.vmat_c",
            "models/heroes/haze.vmdl_c",
            "models/heroes/inferno.vmdl_c",
            "models/props/crate.vmdl_c",
            "panorama/hero.png",
            "root.txt",
        ]
    );
    let desc = idx.query(&q("", 0, 100, SortKey::Path, true));
    assert_eq!(desc.rows.first().unwrap().path, "root.txt");

    // Case-insensitive substring.
    let heroes = idx.query(&q("HEROES", 0, 100, SortKey::Path, false));
    assert_eq!(heroes.total, 3);
    // Multi-term: all must match.
    let multi = idx.query(&q("heroes  vmdl", 0, 100, SortKey::Path, false));
    assert_eq!(multi.total, 2);
    let none = idx.query(&q("heroes zzz", 0, 100, SortKey::Path, false));
    assert_eq!(none.total, 0);
    assert!(none.rows.is_empty());
    // Whitespace-only filter matches everything.
    assert_eq!(idx.query(&q("   ", 0, 100, SortKey::Path, false)).total, 6);

    // Sort by size descending.
    let by_size = idx.query(&q("", 0, 100, SortKey::Size, true));
    let sizes: Vec<u64> = by_size.rows.iter().map(|r| r.size).collect();
    assert_eq!(sizes, vec![300, 200, 100, 50, 10, 5]);
    let by_size_asc = idx.query(&q("", 0, 100, SortKey::Size, false));
    assert_eq!(by_size_asc.rows.first().unwrap().size, 5);

    // Crc / archive sorts keep every row.
    assert_eq!(idx.query(&q("", 0, 100, SortKey::Crc, false)).rows.len(), 6);
    assert_eq!(
        idx.query(&q("", 0, 100, SortKey::Archive, true)).rows.len(),
        6
    );

    // Paging keeps totals.
    let p1 = idx.query(&q("", 0, 4, SortKey::Path, false));
    let p2 = idx.query(&q("", 4, 4, SortKey::Path, false));
    assert_eq!((p1.total, p1.rows.len(), p1.offset), (6, 4, 0));
    assert_eq!((p2.total, p2.rows.len(), p2.offset), (6, 2, 4));
    let beyond = idx.query(&q("", 50, 4, SortKey::Path, false));
    assert_eq!((beyond.total, beyond.rows.len()), (6, 0));

    // Row contents.
    let row = idx
        .query(&q("inferno", 0, 1, SortKey::Path, false))
        .rows
        .remove(0);
    assert_eq!(row.size, 300);
    assert_eq!(row.crc32, crc(&[0; 300]));
    assert_eq!(row.archive_index, None);
    assert_eq!(row.preload, 0);

    // Limit is clamped to at least 1.
    assert_eq!(idx.query(&q("", 0, 0, SortKey::Path, false)).rows.len(), 1);
}

#[test]
fn query_limit_clamps_to_1000() {
    let tmp = TempDir::new().unwrap();
    let entries: Vec<(String, EntrySource)> = (0..1200)
        .map(|i| {
            (
                format!("bulk/f{i:04}.bin"),
                EntrySource::Bytes(vec![0u8; 1]),
            )
        })
        .collect();
    let out = tmp.path().join("bulk_dir.vpk");
    write_vpk(&out, entries, &never).unwrap();
    let idx = VpkIndex::new(VpkArchive::open(&out).unwrap());
    let page = idx.query(&q("", 0, 5000, SortKey::Path, false));
    assert_eq!(page.total, 1200);
    assert_eq!(page.rows.len(), 1000);
    assert_eq!(page.rows[0].path, "bulk/f0000.bin");
}

#[test]
fn folders_listing() {
    let tmp = TempDir::new().unwrap();
    let idx = query_index(tmp.path());
    let root = idx.folders("");
    let names: Vec<(&str, bool, usize)> = root
        .iter()
        .map(|n| (n.name.as_str(), n.is_dir, n.file_count))
        .collect();
    assert_eq!(
        names,
        vec![
            ("materials", true, 1),
            ("models", true, 3),
            ("panorama", true, 1),
            ("root.txt", false, 1)
        ]
    );
    assert_eq!(root[1].size, 600);
    assert_eq!(root[3].size, 5);

    let models = idx.folders("models");
    let names: Vec<(&str, bool, usize)> = models
        .iter()
        .map(|n| (n.name.as_str(), n.is_dir, n.file_count))
        .collect();
    assert_eq!(names, vec![("heroes", true, 2), ("props", true, 1)]);
    assert_eq!(idx.folders("models\\").len(), 2);
    assert_eq!(idx.folders("/models/").len(), 2);

    let heroes = idx.folders("models/heroes");
    let names: Vec<&str> = heroes.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(names, vec!["haze.vmdl_c", "inferno.vmdl_c"]);
    assert!(heroes.iter().all(|n| !n.is_dir));
    // A prefix must match a whole component.
    assert!(idx.folders("mode").is_empty());
    assert!(idx.folders("nothing/here").is_empty());
}

// ---------------------------------------------------------------------------
// Bundles / names
// ---------------------------------------------------------------------------

#[test]
fn group_bundles_per_directory() {
    let tmp = TempDir::new().unwrap();
    let valid = valid_bytes();
    let mk = |dir: &str, name: &str, data: &[u8]| -> PathBuf {
        let d = tmp.path().join(dir);
        std::fs::create_dir_all(&d).unwrap();
        write_file(&d, name, data)
    };
    let red_root = mk("Red", "pak01_dir.vpk", &valid);
    let red_chunk = mk("Red", "pak01_000.vpk", b"chunk data");
    let blue_root = mk("Blue", "pak01_dir.vpk", &valid);
    let blue_chunk1 = mk("Blue", "pak01_001.vpk", b"chunk data");
    let blue_chunk0 = mk("Blue", "PAK01_000.VPK", b"chunk data");
    let single = mk("Single", "cool_skin.vpk", &valid);
    let orphan = mk("Orphans", "ghost_000.vpk", b"not a vpk at all");
    let standalone = mk("Standalone", "odd_000.vpk", &valid);
    let other = mk("Other", "notes.txt", b"hi");
    // A chunk in another directory must not attach to Red.
    let stray = mk("Stray", "pak01_002.vpk", b"junk");

    let paths = vec![
        red_chunk.clone(),
        standalone.clone(),
        blue_chunk1.clone(),
        red_root.clone(),
        orphan.clone(),
        blue_root.clone(),
        single.clone(),
        other,
        blue_chunk0.clone(),
        stray.clone(),
        red_root.clone(), // duplicate
    ];
    let (bundles, orphans) = group_bundles(&paths);

    let roots: Vec<&PathBuf> = bundles.iter().map(|b| &b.root).collect();
    assert_eq!(roots, vec![&blue_root, &red_root, &single, &standalone]);
    assert_eq!(
        bundles[0].chunks,
        vec![(0, blue_chunk0.clone()), (1, blue_chunk1.clone())]
    );
    assert_eq!(bundles[1].chunks, vec![(0, red_chunk)]);
    assert!(bundles[2].chunks.is_empty());
    assert!(bundles[3].chunks.is_empty());
    assert_eq!(orphans, vec![orphan, stray]);
}

#[test]
fn enabled_root_parsing() {
    assert_eq!(parse_enabled_root("pak1_dir.vpk"), Some(1));
    assert_eq!(parse_enabled_root("pak001_dir.vpk"), Some(1));
    assert_eq!(parse_enabled_root("pak00_dir.vpk"), None);
    assert_eq!(parse_enabled_root("pak100_dir.vpk"), None);
    assert_eq!(parse_enabled_root("PAK07_DIR.VPK"), Some(7));
    assert!(is_canonical_enabled_root("pak05_dir.vpk"));
    assert!(!is_canonical_enabled_root("pak5_dir.vpk"));
    assert_eq!(enabled_root_name(5), "pak05_dir.vpk");
    assert_eq!(enabled_chunk_name(5, 3), "pak05_003.vpk");
    assert_eq!(
        parse_chunk_name("pak05_003.vpk"),
        Some(("pak05".to_string(), 3))
    );
    assert_eq!(root_base("pak05_dir.vpk"), "pak05");
}

// ---------------------------------------------------------------------------
// Live game archive (read-only)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "needs KMM_LIVE_GAME pointing at a Deadlock install"]
fn live_game_vpk() {
    let Ok(game) = std::env::var("KMM_LIVE_GAME") else {
        println!("KMM_LIVE_GAME not set; skipping");
        return;
    };
    let path = PathBuf::from(game)
        .join("game")
        .join("citadel")
        .join("pak01_dir.vpk");
    let t = std::time::Instant::now();
    let archive = VpkArchive::open(&path).unwrap();
    let open_time = t.elapsed();
    println!("entries: {}", archive.entries().len());
    println!(
        "version: {}, chunks: {}",
        archive.version(),
        archive.chunk_indices().len()
    );
    println!("open: {open_time:?}");
    assert!(archive.entries().len() > 100_000);
    assert!(archive.missing_chunks().is_empty());

    // Spot-check CRCs of a sample of real files (read-only).
    let t = std::time::Instant::now();
    let sample_paths: Vec<String> = archive
        .file_paths()
        .into_iter()
        .step_by(archive.entries().len() / 200)
        .map(str::to_string)
        .collect();
    for p in &sample_paths {
        let first = archive.entries().iter().find(|e| &e.path == p).unwrap();
        let data = archive.read_file(p).unwrap();
        let same_path = archive.entries().iter().filter(|e| &e.path == p).count();
        if same_path == 1 {
            assert_eq!(crc(&data), first.crc32, "{p}");
        }
    }
    println!(
        "spot-checked {} files in {:?}",
        sample_paths.len(),
        t.elapsed()
    );

    let t = std::time::Instant::now();
    let idx = VpkIndex::new(archive);
    println!(
        "index build ({} unique paths): {:?}",
        idx.len(),
        t.elapsed()
    );

    let t = std::time::Instant::now();
    let page = idx.query(&q("heroes", 0, 50, SortKey::Path, false));
    let query_time = t.elapsed();
    println!(
        "query 'heroes': total {} rows {} in {query_time:?}",
        page.total,
        page.rows.len()
    );
    let t = std::time::Instant::now();
    let page2 = idx.query(&q("heroes models", 0, 50, SortKey::Size, true));
    println!(
        "query 'heroes models' size desc: total {} in {:?}",
        page2.total,
        t.elapsed()
    );
    let t = std::time::Instant::now();
    let folders = idx.folders("");
    println!("root folders: {} in {:?}", folders.len(), t.elapsed());
    assert!(page.total > 0);
}
