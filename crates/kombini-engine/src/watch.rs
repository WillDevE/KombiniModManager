//! Debounced watcher for the addon folders (§13.3).
//!
//! Fires `on_change` at most once per 500 ms burst of `.vpk` changes directly
//! inside an `addons*` folder, ignoring `_kombini/` and the engine's own writes.

use crate::paths::GamePaths;
use kombini_core::naming::parse_shard_dir;
use notify::{RecursiveMode, Watcher};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

pub const DEBOUNCE: Duration = Duration::from_millis(500);

/// Keeps the watcher alive; dropping it stops watching.
pub struct AddonsWatcher {
    _watcher: notify::RecommendedWatcher,
    _thread: std::thread::JoinHandle<()>,
}

/// True when `path` is a `.vpk` directly inside `citadel/addons*`.
pub fn is_relevant(citadel: &Path, path: &Path) -> bool {
    let is_vpk = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("vpk"));
    let parent_is_shard = path
        .parent()
        .and_then(|p| (p.parent() == Some(citadel)).then(|| p.file_name())?)
        .and_then(|n| parse_shard_dir(&n.to_string_lossy()))
        .is_some();
    is_vpk && parent_is_shard
}

impl AddonsWatcher {
    /// `is_quiet` returns true while the engine is writing (events are ours).
    pub fn start(
        paths: &GamePaths,
        is_quiet: impl Fn() -> bool + Send + 'static,
        on_change: impl Fn() + Send + 'static,
    ) -> notify::Result<Self> {
        let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
        let mut watcher = notify::recommended_watcher(tx)?;
        watcher.watch(&paths.citadel, RecursiveMode::Recursive)?;
        let citadel = paths.citadel.clone();
        let thread = std::thread::Builder::new()
            .name("addons-watcher".into())
            .spawn(move || {
                let relevant =
                    |event: &notify::Event| event.paths.iter().any(|p| is_relevant(&citadel, p));
                while let Ok(event) = rx.recv() {
                    let Ok(event) = event else { continue };
                    if !relevant(&event) {
                        continue;
                    }
                    // Drain the burst.
                    loop {
                        match rx.recv_timeout(DEBOUNCE) {
                            Ok(_) => continue,
                            Err(mpsc::RecvTimeoutError::Timeout) => break,
                            Err(mpsc::RecvTimeoutError::Disconnected) => return,
                        }
                    }
                    if !is_quiet() {
                        on_change();
                    }
                }
            })
            .map_err(|e| notify::Error::generic(&e.to_string()))?;
        Ok(Self {
            _watcher: watcher,
            _thread: thread,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relevance() {
        let citadel = Path::new("/g/game/citadel");
        assert!(is_relevant(
            citadel,
            &citadel.join("addons").join("pak01_dir.vpk")
        ));
        assert!(is_relevant(citadel, &citadel.join("addons7").join("x.VPK")));
        assert!(!is_relevant(
            citadel,
            &citadel.join("addons").join("_kombini").join("a.vpk")
        ));
        assert!(!is_relevant(citadel, &citadel.join("pak01_dir.vpk")));
        assert!(!is_relevant(
            citadel,
            &citadel.join("addons").join("readme.txt")
        ));
        assert!(!is_relevant(citadel, &citadel.join("maps").join("a.vpk")));
    }
}
