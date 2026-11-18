//! Game folder layout and the write-scope guard (§13.2.3).

use kombini_core::naming::shard_dir_name;
use kombini_core::{AppError, ErrorCode, MAX_SHARDS};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct GamePaths {
    pub root: PathBuf,
    pub citadel: PathBuf,
}

impl GamePaths {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            citadel: root.join("game").join("citadel"),
        }
    }

    pub fn shard(&self, shard: u8) -> PathBuf {
        self.citadel.join(shard_dir_name(shard))
    }

    pub fn shards(&self) -> impl Iterator<Item = (u8, PathBuf)> + '_ {
        (1..=MAX_SHARDS).map(|s| (s, self.shard(s)))
    }

    pub fn gameinfo(&self) -> PathBuf {
        self.citadel.join("gameinfo.gi")
    }

    pub fn autoexec(&self) -> PathBuf {
        self.citadel.join("cfg").join("autoexec.cfg")
    }

    pub fn rpt(&self) -> PathBuf {
        self.citadel.join("rpt")
    }

    pub fn console_log(&self) -> PathBuf {
        self.citadel.join("console.log")
    }

    /// `addons/_kombini` — the manager's private working area inside the game.
    pub fn work(&self) -> PathBuf {
        self.shard(1).join("_kombini")
    }

    pub fn txn_dir(&self) -> PathBuf {
        self.work().join("txn")
    }

    pub fn trash_dir(&self) -> PathBuf {
        self.work().join("trash")
    }

    pub fn recovered_dir(&self) -> PathBuf {
        self.work().join("recovered")
    }

    pub fn base_vpks(&self) -> Vec<PathBuf> {
        vec![
            self.citadel.join("pak01_dir.vpk"),
            self.root.join("game").join("core").join("pak01_dir.vpk"),
        ]
    }

    /// Ensures `path` is inside `game/citadel` after resolving links of its
    /// nearest existing ancestor (junction/symlink escapes are rejected).
    pub fn check_scope(&self, path: &Path) -> Result<(), AppError> {
        let citadel = canonical_or_self(&self.citadel);
        let mut probe = path.to_path_buf();
        let mut suffix = Vec::new();
        while !probe.exists() {
            match (probe.file_name(), probe.parent()) {
                (Some(name), Some(parent)) => {
                    suffix.push(name.to_os_string());
                    probe = parent.to_path_buf();
                }
                _ => break,
            }
        }
        let mut resolved = canonical_or_self(&probe);
        for part in suffix.iter().rev() {
            resolved.push(part);
        }
        if resolved.starts_with(&citadel) {
            Ok(())
        } else {
            Err(AppError::new(
                ErrorCode::PermissionDenied,
                "Refusing to write outside the Deadlock game/citadel folder",
            )
            .with_detail(path.display().to_string()))
        }
    }
}

fn canonical_or_self(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// App data/cache directories.
#[derive(Debug, Clone)]
pub struct AppDirs {
    pub data: PathBuf,
    pub cache: PathBuf,
}

impl AppDirs {
    /// `KOMBINI_DATA_DIR` / `KOMBINI_CACHE_DIR` override the OS defaults
    /// (`%LOCALAPPDATA%\Kombini\ModManager\data` on Windows).
    pub fn from_env() -> Result<Self, AppError> {
        let env = |key: &str| {
            std::env::var_os(key)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let project = directories::ProjectDirs::from("com", "Kombini", "ModManager");
        let data = env("KOMBINI_DATA_DIR")
            .or_else(|| project.as_ref().map(|p| p.data_local_dir().to_path_buf()))
            .ok_or_else(|| AppError::internal("Could not determine the app data folder"))?;
        let cache = env("KOMBINI_CACHE_DIR")
            .or_else(|| project.as_ref().map(|p| p.cache_dir().to_path_buf()))
            .unwrap_or_else(|| data.join("cache"));
        Ok(Self { data, cache })
    }

    pub fn db(&self) -> PathBuf {
        self.data.join("kombini.db")
    }
    pub fn blobs(&self) -> PathBuf {
        self.data.join("blobs")
    }
    pub fn downloads(&self) -> PathBuf {
        self.data.join("downloads")
    }
    pub fn logs(&self) -> PathBuf {
        self.data.join("logs")
    }
    pub fn extract_tmp(&self) -> PathBuf {
        self.cache.join("extract")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope() {
        let dir = tempfile::tempdir().unwrap();
        let paths = GamePaths::new(dir.path());
        std::fs::create_dir_all(paths.shard(1)).unwrap();
        assert!(
            paths
                .check_scope(&paths.shard(1).join("pak01_dir.vpk"))
                .is_ok()
        );
        assert!(
            paths
                .check_scope(&paths.shard(5).join("x").join("y.vpk"))
                .is_ok()
        );
        assert!(paths.check_scope(&dir.path().join("other.txt")).is_err());
        assert!(
            paths
                .check_scope(&paths.shard(1).join("..").join("..").join("x"))
                .is_err()
        );
    }
}
