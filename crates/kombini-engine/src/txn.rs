//! Journaled two-phase file transactions in the addon folders (§6.1).
//!
//! Phase 1 moves every source into a staging folder (or copies a blob there);
//! phase 2 moves every staged file to its target. The database commit records
//! the transaction id atomically with the new state; recovery rolls forward
//! when that id was committed and back otherwise.

use crate::fsops;
use crate::paths::GamePaths;
use kombini_core::{AppError, BlobHash, ErrorCode};
use kombini_storage::BlobStore;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Move { from: PathBuf, to: PathBuf },
    Place { blob: BlobHash, to: PathBuf },
    Remove { from: PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Staging,
    Placing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Journal {
    id: String,
    created_at: i64,
    phase: Phase,
    ops: Vec<Op>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TxnReport {
    pub id: String,
    pub moved: usize,
    pub placed: usize,
    pub removed: usize,
    /// Files parked in `_kombini/recovered` instead of being overwritten.
    pub recovered: Vec<String>,
}

fn journal_path(paths: &GamePaths, id: &str) -> PathBuf {
    paths.txn_dir().join(format!("{id}.journal.json"))
}

fn stage_path(paths: &GamePaths, id: &str, index: usize) -> PathBuf {
    paths.txn_dir().join(id).join(index.to_string())
}

fn trash_path(paths: &GamePaths, id: &str, index: usize) -> PathBuf {
    paths.trash_dir().join(id).join(index.to_string())
}

fn write_journal(paths: &GamePaths, journal: &Journal) -> Result<(), AppError> {
    let bytes = serde_json::to_vec_pretty(journal)
        .map_err(|e| AppError::internal(format!("journal serialization: {e}")))?;
    fsops::write_atomic(&journal_path(paths, &journal.id), &bytes)
}

fn lower(p: &Path) -> String {
    p.to_string_lossy().to_lowercase().replace('\\', "/")
}

/// Validates an op list: unique targets, and every existing target must be
/// vacated by another op in the same transaction.
pub fn validate(ops: &[Op]) -> Result<(), AppError> {
    let mut targets = BTreeSet::new();
    let mut vacated = BTreeSet::new();
    for op in ops {
        match op {
            Op::Move { from, .. } | Op::Remove { from } => {
                vacated.insert(lower(from));
            }
            Op::Place { .. } => {}
        }
    }
    for op in ops {
        let to = match op {
            Op::Move { to, .. } | Op::Place { to, .. } => to,
            Op::Remove { .. } => continue,
        };
        let key = lower(to);
        if !targets.insert(key.clone()) {
            return Err(AppError::new(
                ErrorCode::LayoutConflict,
                "Two mod files would get the same name",
            )
            .with_detail(to.display().to_string()));
        }
        if to.exists() && !vacated.contains(&key) {
            return Err(AppError::new(
                ErrorCode::LayoutConflict,
                "A file the manager does not own is in the way",
            )
            .with_detail(to.display().to_string()));
        }
    }
    Ok(())
}

/// Executes `ops`. `before_phase2` is a last-moment guard (game running);
/// `commit` persists the new state together with the transaction id.
pub fn execute(
    paths: &GamePaths,
    blobs: &BlobStore,
    ops: Vec<Op>,
    before_phase2: &dyn Fn() -> Result<(), AppError>,
    commit: &mut dyn FnMut(&str) -> Result<(), AppError>,
    progress: &dyn Fn(usize, usize),
) -> Result<TxnReport, AppError> {
    let id = format!(
        "t{}{}",
        kombini_storage::now(),
        kombini_storage::random_hex(3)
    );
    let mut report = TxnReport {
        id: id.clone(),
        ..TxnReport::default()
    };
    if ops.is_empty() {
        commit(&id)?;
        return Ok(report);
    }
    validate(&ops)?;
    for op in &ops {
        match op {
            Op::Move { from, to } => {
                paths.check_scope(from)?;
                paths.check_scope(to)?;
            }
            Op::Place { to, .. } => paths.check_scope(to)?,
            Op::Remove { from } => paths.check_scope(from)?,
        }
    }
    let mut journal = Journal {
        id: id.clone(),
        created_at: kombini_storage::now(),
        phase: Phase::Staging,
        ops,
    };
    write_journal(paths, &journal)?;
    let total = journal.ops.len() * 2;

    // Phase 1: stage.
    for (i, op) in journal.ops.iter().enumerate() {
        let stage = stage_path(paths, &id, i);
        let result = match op {
            Op::Move { from, .. } | Op::Remove { from } => fsops::rename(from, &stage),
            Op::Place { blob, .. } => blobs
                .copy_to(blob, &stage)
                .map(|_| ())
                .map_err(AppError::from),
        };
        if let Err(err) = result {
            rollback(paths, &journal, &mut report);
            cleanup(paths, &id);
            return Err(err);
        }
        progress(i + 1, total);
    }
    journal.phase = Phase::Placing;
    if let Err(err) = write_journal(paths, &journal).and_then(|_| before_phase2()) {
        rollback(paths, &journal, &mut report);
        cleanup(paths, &id);
        return Err(err);
    }

    // Phase 2: place.
    let n = journal.ops.len();
    for (i, op) in journal.ops.iter().enumerate() {
        let stage = stage_path(paths, &id, i);
        let result = match op {
            Op::Move { to, .. } | Op::Place { to, .. } => fsops::rename(&stage, to),
            Op::Remove { .. } => fsops::rename(&stage, &trash_path(paths, &id, i)),
        };
        if let Err(err) = result {
            rollback(paths, &journal, &mut report);
            cleanup(paths, &id);
            return Err(err);
        }
        progress(n + i + 1, total);
    }

    if let Err(err) = commit(&id) {
        tracing::error!(error = %err, "state commit failed; rolling back files");
        rollback(paths, &journal, &mut report);
        cleanup(paths, &id);
        return Err(err);
    }
    for op in &journal.ops {
        match op {
            Op::Move { .. } => report.moved += 1,
            Op::Place { .. } => report.placed += 1,
            Op::Remove { .. } => report.removed += 1,
        }
    }
    cleanup(paths, &id);
    Ok(report)
}

fn park(paths: &GamePaths, id: &str, file: &Path, report: &mut TxnReport) {
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    let target = paths
        .recovered_dir()
        .join(id)
        .join(format!("{}_{name}", kombini_storage::random_hex(2)));
    match fsops::rename(file, &target) {
        Ok(()) => report.recovered.push(target.display().to_string()),
        Err(err) => tracing::error!(error = %err, file = %file.display(), "could not park file"),
    }
}

/// Restores every source to its original location. Never deletes a file that
/// exists in two places; the extra copy is parked in `_kombini/recovered`.
fn rollback(paths: &GamePaths, journal: &Journal, report: &mut TxnReport) {
    let id = &journal.id;
    if journal.phase == Phase::Placing {
        for (i, op) in journal.ops.iter().enumerate().rev() {
            let stage = stage_path(paths, id, i);
            if stage.exists() {
                continue;
            }
            match op {
                Op::Move { to, .. } if to.exists() => {
                    if let Err(e) = fsops::rename(to, &stage) {
                        tracing::error!(error = %e, "rollback: unplace failed");
                    }
                }
                Op::Place { to, .. } if to.exists() => {
                    if let Err(e) = fsops::remove_file(to) {
                        tracing::error!(error = %e, "rollback: remove placed copy failed");
                    }
                }
                Op::Remove { .. } => {
                    let trash = trash_path(paths, id, i);
                    if trash.exists()
                        && let Err(e) = fsops::rename(&trash, &stage)
                    {
                        tracing::error!(error = %e, "rollback: untrash failed");
                    }
                }
                _ => {}
            }
        }
    }
    for (i, op) in journal.ops.iter().enumerate().rev() {
        let stage = stage_path(paths, id, i);
        if !stage.exists() {
            continue;
        }
        match op {
            Op::Move { from, .. } | Op::Remove { from } => {
                if from.exists() {
                    park(paths, id, &stage, report);
                } else if let Err(e) = fsops::rename(&stage, from) {
                    tracing::error!(error = %e, "rollback: unstage failed");
                    park(paths, id, &stage, report);
                }
            }
            Op::Place { .. } => {
                let _ = fsops::remove_file(&stage);
            }
        }
    }
}

fn roll_forward(paths: &GamePaths, journal: &Journal, report: &mut TxnReport) {
    let id = &journal.id;
    for (i, op) in journal.ops.iter().enumerate() {
        let stage = stage_path(paths, id, i);
        if !stage.exists() {
            continue;
        }
        match op {
            Op::Move { to, .. } | Op::Place { to, .. } => {
                if to.exists() {
                    park(paths, id, &stage, report);
                } else if let Err(e) = fsops::rename(&stage, to) {
                    tracing::error!(error = %e, "roll forward failed");
                    park(paths, id, &stage, report);
                }
            }
            Op::Remove { .. } => {
                let _ = fsops::remove_file(&stage);
            }
        }
    }
}

fn cleanup(paths: &GamePaths, id: &str) {
    let _ = std::fs::remove_file(journal_path(paths, id));
    let _ = std::fs::remove_dir_all(paths.txn_dir().join(id));
    let _ = std::fs::remove_dir_all(paths.trash_dir().join(id));
}

/// Recovers every journal left by an interrupted transaction. Returns reports
/// for the UI ("an interrupted change was rolled back").
pub fn recover(paths: &GamePaths, committed_txn: Option<&str>) -> Vec<String> {
    let mut messages = Vec::new();
    let Ok(entries) = std::fs::read_dir(paths.txn_dir()) else {
        return messages;
    };
    let mut journals: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().ends_with(".journal.json"))
                .unwrap_or(false)
        })
        .collect();
    journals.sort();
    for path in journals {
        let journal: Journal = match std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
        {
            Some(j) => j,
            None => {
                messages.push(format!(
                    "Found an unreadable transaction journal at {}; left in place for inspection.",
                    path.display()
                ));
                continue;
            }
        };
        let mut report = TxnReport::default();
        if committed_txn == Some(journal.id.as_str()) {
            roll_forward(paths, &journal, &mut report);
            messages.push("An interrupted mod change was completed.".to_owned());
        } else {
            rollback(paths, &journal, &mut report);
            messages.push("An interrupted mod change was rolled back.".to_owned());
        }
        if !report.recovered.is_empty() {
            messages.push(format!(
                "{} duplicate file(s) were moved to {} instead of being deleted.",
                report.recovered.len(),
                paths.recovered_dir().display()
            ));
        }
        cleanup(paths, &journal.id);
    }
    // Stale trash from completed transactions.
    let _ = std::fs::remove_dir_all(paths.trash_dir());
    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _dir: tempfile::TempDir,
        paths: GamePaths,
        blobs: BlobStore,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Deadlock");
        let paths = GamePaths::new(&root);
        std::fs::create_dir_all(paths.shard(1)).unwrap();
        let blobs = BlobStore::new(dir.path().join("blobs")).unwrap();
        Fixture {
            _dir: dir,
            paths,
            blobs,
        }
    }

    fn put(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn swap_place_remove() {
        let f = fixture();
        let a = f.paths.shard(1).join("pak01_dir.vpk");
        let b = f.paths.shard(1).join("pak02_dir.vpk");
        let gone = f.paths.shard(1).join("gb9_x.vpk");
        let new = f.paths.shard(2).join("pak01_dir.vpk");
        put(&a, "A");
        put(&b, "B");
        put(&gone, "G");
        let (blob, _) = f.blobs.insert_bytes(b"NEW").unwrap();
        let ops = vec![
            Op::Move {
                from: a.clone(),
                to: b.clone(),
            },
            Op::Move {
                from: b.clone(),
                to: a.clone(),
            },
            Op::Place {
                blob,
                to: new.clone(),
            },
            Op::Remove { from: gone.clone() },
        ];
        let mut committed = None;
        let report = execute(
            &f.paths,
            &f.blobs,
            ops,
            &|| Ok(()),
            &mut |id| {
                committed = Some(id.to_owned());
                Ok(())
            },
            &|_, _| {},
        )
        .unwrap();
        assert_eq!(read(&a), "B");
        assert_eq!(read(&b), "A");
        assert_eq!(read(&new), "NEW");
        assert!(!gone.exists());
        assert_eq!((report.moved, report.placed, report.removed), (2, 1, 1));
        assert!(committed.is_some());
        assert_eq!(std::fs::read_dir(f.paths.txn_dir()).unwrap().count(), 0);
    }

    #[test]
    fn foreign_target_rejected() {
        let f = fixture();
        let a = f.paths.shard(1).join("gb1_a.vpk");
        let foreign = f.paths.shard(1).join("pak01_dir.vpk");
        put(&a, "A");
        put(&foreign, "F");
        let ops = vec![Op::Move {
            from: a.clone(),
            to: foreign.clone(),
        }];
        let err = execute(
            &f.paths,
            &f.blobs,
            ops,
            &|| Ok(()),
            &mut |_| Ok(()),
            &|_, _| {},
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::LayoutConflict);
        assert_eq!(read(&a), "A");
        assert_eq!(read(&foreign), "F");
    }

    #[test]
    fn guard_failure_and_commit_failure_roll_back() {
        for fail_commit in [false, true] {
            let f = fixture();
            let a = f.paths.shard(1).join("gb1_a.vpk");
            let target = f.paths.shard(1).join("pak01_dir.vpk");
            put(&a, "A");
            let (blob, _) = f.blobs.insert_bytes(b"P").unwrap();
            let placed = f.paths.shard(3).join("pak01_dir.vpk");
            let ops = vec![
                Op::Move {
                    from: a.clone(),
                    to: target.clone(),
                },
                Op::Place {
                    blob,
                    to: placed.clone(),
                },
            ];
            let guard = |fail: bool| {
                move || -> Result<(), AppError> {
                    if fail {
                        Err(AppError::game_running())
                    } else {
                        Ok(())
                    }
                }
            };
            let result = execute(
                &f.paths,
                &f.blobs,
                ops,
                &guard(!fail_commit),
                &mut |_| {
                    if fail_commit {
                        Err(AppError::storage("disk full"))
                    } else {
                        Ok(())
                    }
                },
                &|_, _| {},
            );
            assert!(result.is_err());
            assert_eq!(read(&a), "A");
            assert!(!target.exists());
            assert!(!placed.exists());
        }
    }

    /// Simulates a crash after phase 2 (journal left behind) for both a
    /// committed and an uncommitted transaction.
    #[test]
    fn recovery_forward_and_back() {
        for committed in [false, true] {
            let f = fixture();
            let a = f.paths.shard(1).join("gb1_a.vpk");
            let b = f.paths.shard(1).join("pak01_dir.vpk");
            put(&a, "A");
            let journal = Journal {
                id: "tcrash".into(),
                created_at: 0,
                phase: Phase::Placing,
                ops: vec![Op::Move {
                    from: a.clone(),
                    to: b.clone(),
                }],
            };
            write_journal(&f.paths, &journal).unwrap();
            // Phase 1 done, crash before phase 2.
            fsops::rename(&a, &stage_path(&f.paths, "tcrash", 0)).unwrap();
            let messages = recover(&f.paths, committed.then_some("tcrash"));
            assert_eq!(messages.len(), 1);
            if committed {
                assert_eq!(read(&b), "A");
                assert!(!a.exists());
            } else {
                assert_eq!(read(&a), "A");
                assert!(!b.exists());
            }
            assert!(!journal_path(&f.paths, "tcrash").exists());
        }
    }

    #[test]
    fn recovery_never_deletes_duplicates() {
        let f = fixture();
        let a = f.paths.shard(1).join("gb1_a.vpk");
        let journal = Journal {
            id: "tdup".into(),
            created_at: 0,
            phase: Phase::Staging,
            ops: vec![Op::Move {
                from: a.clone(),
                to: f.paths.shard(1).join("pak01_dir.vpk"),
            }],
        };
        write_journal(&f.paths, &journal).unwrap();
        put(&stage_path(&f.paths, "tdup", 0), "STAGED");
        put(&a, "ORIGINAL");
        let messages = recover(&f.paths, None);
        assert!(messages.iter().any(|m| m.contains("duplicate")));
        assert_eq!(read(&a), "ORIGINAL");
        let recovered: Vec<_> = walk(&f.paths.recovered_dir());
        assert_eq!(recovered.len(), 1);
        assert_eq!(read(&recovered[0]), "STAGED");
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                if e.path().is_dir() {
                    out.extend(walk(&e.path()));
                } else {
                    out.push(e.path());
                }
            }
        }
        out
    }
}
