//! Performance budgets for the VPK inspector path. Timings are
//! printed always and asserted only in optimized builds:
//! `cargo test --release -p kombini-vpk --test budgets -- --nocapture`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use kombini_vpk::{EntryQuery, EntrySource, SortKey, VpkArchive, VpkIndex, write_vpk};
use std::time::{Duration, Instant};

/// Entries in the live game's `pak01_dir.vpk` (order of magnitude).
const ENTRIES: usize = 158_000;

fn budget(label: &str, took: Duration, limit: Duration) {
    println!("{label}: {took:?} (budget {limit:?})");
    if !cfg!(debug_assertions) {
        assert!(took <= limit, "{label} took {took:?}, budget {limit:?}");
    }
}

#[test]
fn inspector_budgets_158k_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big_dir.vpk");
    let entries: Vec<(String, EntrySource)> = (0..ENTRIES)
        .map(|i| {
            let folder = [
                "models/heroes",
                "materials/ui",
                "sounds/hero",
                "panorama/images",
            ][i % 4];
            (
                format!("{folder}/group_{:03}/asset_{i:06}.vtex_c", i % 997),
                EntrySource::Bytes(vec![(i % 251) as u8]),
            )
        })
        .collect();
    write_vpk(&path, entries, &|| false).unwrap();

    let started = Instant::now();
    let archive = VpkArchive::open(&path).unwrap();
    let index = VpkIndex::new(archive);
    let open = started.elapsed();
    assert_eq!(index.len(), ENTRIES);

    let q = |filter: &str, offset: usize, sort: SortKey| EntryQuery {
        filter: filter.to_owned(),
        offset,
        limit: 200,
        sort,
        descending: false,
    };

    let t = Instant::now();
    let first = index.query(&q("", 0, SortKey::Path));
    let first_page = open + t.elapsed();
    assert_eq!(first.rows.len(), 200);
    assert_eq!(first.total, ENTRIES);

    let t = Instant::now();
    let filtered = index.query(&q("group_042", 0, SortKey::Path));
    let filter = t.elapsed();
    assert!(filtered.total > 0 && filtered.total < ENTRIES);

    let t = Instant::now();
    let deep = index.query(&q("", ENTRIES - 200, SortKey::Size));
    let sorted_last_page = t.elapsed();
    assert_eq!(deep.rows.len(), 200);

    budget(
        "open + index 158k entries",
        open,
        Duration::from_millis(800),
    );
    budget(
        "first page (open + query)",
        first_page,
        Duration::from_secs(1),
    );
    budget("filter 158k paths", filter, Duration::from_millis(100));
    budget(
        "last page sorted by size",
        sorted_last_page,
        Duration::from_millis(250),
    );
}
