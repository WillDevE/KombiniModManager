//! Synthetic game folders and engines for tests.

use crate::{AppDirs, Engine, EngineConfig, ProcessCheck};
use kombini_vpk::EntrySource;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const STOCK_GAMEINFO: &str = include_str!("../../kombini-gameinfo/tests/fixtures/stock.gi");

#[derive(Default)]
pub struct FakeProcess(pub AtomicBool);

impl ProcessCheck for FakeProcess {
    fn is_running(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub root: PathBuf,
    pub process: Arc<FakeProcess>,
}

impl Fixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Deadlock");
        let citadel = root.join("game").join("citadel");
        std::fs::create_dir_all(citadel.join("addons")).unwrap();
        std::fs::write(citadel.join("gameinfo.gi"), STOCK_GAMEINFO).unwrap();
        Self {
            dir,
            root,
            process: Arc::new(FakeProcess::default()),
        }
    }

    pub fn dirs(&self) -> AppDirs {
        AppDirs {
            data: self.dir.path().join("data"),
            cache: self.dir.path().join("cache"),
        }
    }

    pub fn open(&self) -> Engine {
        Engine::open(EngineConfig {
            dirs: self.dirs(),
            game_root: self.root.clone(),
            process: self.process.clone(),
        })
        .unwrap()
        .0
    }

    pub fn addons(&self, shard: u8) -> PathBuf {
        self.root
            .join("game")
            .join("citadel")
            .join(kombini_core::naming::shard_dir_name(shard))
    }

    /// Sorted file names directly inside a shard folder.
    pub fn listing(&self, shard: u8) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.addons(shard))
            .map(|r| {
                r.flatten()
                    .filter(|e| e.path().is_file())
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    pub fn gameinfo(&self) -> String {
        std::fs::read_to_string(self.root.join("game/citadel/gameinfo.gi")).unwrap()
    }

    /// A scratch folder outside the game.
    pub fn scratch(&self, name: &str) -> PathBuf {
        let p = self.dir.path().join("scratch").join(name);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}

/// Writes a single-file VPK containing `entries` (path → content).
pub fn make_vpk(path: &Path, entries: &[(&str, &str)]) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let entries = entries
        .iter()
        .map(|(p, c)| ((*p).to_owned(), EntrySource::Bytes(c.as_bytes().to_vec())))
        .collect();
    kombini_vpk::write_vpk(path, entries, &|| false).unwrap();
    path.to_path_buf()
}
