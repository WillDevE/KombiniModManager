//! App settings (ARCHITECTURE §10), persisted as one JSON document.

use serde::{Deserialize, Serialize};

pub const SETTINGS_KEY: &str = "app_settings";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub game_path: Option<String>,
    pub block_while_running: bool,
    pub auto_reapply_gameinfo: bool,
    pub auto_snapshot: bool,
    pub snapshot_retention: u32,
    pub keep_archives: bool,
    pub max_concurrent_downloads: u32,
    pub check_updates_on_startup: bool,
    pub update_check_interval_minutes: u32,
    pub default_launch_options: String,
    pub url_handlers_registered: bool,
    pub bandwidth_limit_kbps: Option<u32>,
    /// Snapshot before launching modded.
    pub snapshot_before_launch: bool,
    /// Seconds to wait for deadlock.exe after asking Steam to launch.
    pub launch_timeout_seconds: u32,
    /// Hide mods GameBanana marks as mature (content ratings or a
    /// warn/hide initial visibility) from Discover.
    pub hide_nsfw: bool,
    /// Ask before enabling a second skin for the same hero.
    pub hero_swap_prompt: bool,
    /// Give every hero with two or more skins a random one on Launch Modded.
    pub shuffle_on_launch: bool,
    /// Installing another file from a mod's page turns off the files of
    /// that page you already have (switch variants instead of stacking).
    pub switch_variants: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            game_path: None,
            block_while_running: true,
            auto_reapply_gameinfo: true,
            auto_snapshot: true,
            snapshot_retention: 20,
            keep_archives: true,
            max_concurrent_downloads: 2,
            check_updates_on_startup: true,
            update_check_interval_minutes: 60,
            default_launch_options: String::new(),
            url_handlers_registered: false,
            bandwidth_limit_kbps: None,
            snapshot_before_launch: false,
            launch_timeout_seconds: 60,
            hide_nsfw: true,
            hero_swap_prompt: true,
            shuffle_on_launch: false,
            switch_variants: true,
        }
    }
}

impl Settings {
    /// Clamps values into sane ranges.
    pub fn normalized(mut self) -> Self {
        self.snapshot_retention = self.snapshot_retention.clamp(1, 500);
        self.max_concurrent_downloads = self.max_concurrent_downloads.clamp(1, 6);
        self.update_check_interval_minutes = self.update_check_interval_minutes.clamp(10, 1440);
        self.launch_timeout_seconds = self.launch_timeout_seconds.clamp(10, 300);
        self
    }
}
