//! Steam library discovery, Deadlock build info, launch and process detection
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod build;
pub mod discover;
pub mod launch;
pub mod news;
pub mod process;
pub mod vdf;

pub use build::{
    AppManifest, BuildInfo, SteamInf, build_info_from, parse_app_manifest, parse_steam_inf,
    read_build_info,
};
pub use discover::{
    DeadlockInstall, find_deadlock_installs, find_steam_dirs, library_folders,
    machine_convars_files, parse_library_folders, steam_dir_for, validate_game_root,
};
pub use launch::{
    LaunchMethod, has_autoexec_options, launch, launch_uri, merge_launch_options,
    split_launch_options, steam_executable,
};
pub use process::{ProcessProbe, is_deadlock_name};
pub use vdf::{KvValue, parse_kv};

#[derive(Debug, thiserror::Error)]
pub enum SteamError {
    #[error("{0} is not a Deadlock folder (game/citadel/gameinfo.gi was not found)")]
    NotDeadlock(std::path::PathBuf),
    #[error("Steam was not found")]
    SteamNotFound,
    #[error("could not start Steam: {0}")]
    Launch(String),
    #[error("I/O error on {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub const APP_ID: u32 = 1422450;
pub const PROCESS_NAME: &str = "deadlock.exe";
