//! Safe editing of Deadlock gameinfo.gi search paths
//!
//! Deadlock only loads addon VPKs when `Game citadel/addons` (and further shard folders) appear
//! above the base `Game citadel` mount. Once that happens the engine would default the `Mod` and
//! `Write` search paths to the addon folder and abort with
//! `Unable to read default keybinding configuration (user_keys_default)`, so the managed block
//! always includes `Mod`/`Write` for `citadel` and `core` as well.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod edit;
pub mod file;
pub mod parse;
pub mod text;

pub use edit::{
    EditError, ensure_text, remove_other_manager_mounts_text, repair_markers_text, unpatch_text,
};
pub use file::{GameInfoError, GameInfoFile, GameInfoStatus, Issue, IssueCode, WriteReport};
pub use parse::{Analysis, MountLine, analyze};
pub use text::{DecodedText, Encoding, decode, encode};

/// First line of the block Kombini manages inside `SearchPaths`.
pub const MANAGED_START: &str = "// >>> Kombini Mod Manager (managed block) >>>";
/// Last line of the block Kombini manages inside `SearchPaths`.
pub const MANAGED_END: &str = "// <<< Kombini Mod Manager <<<";
/// Appended to the "missing Mod/Write" issue message.
pub const KEYBINDING_WARNING: &str = "Deadlock may abort with \"Unable to read default keybinding configuration (user_keys_default)\"";

const MAX_SHARDS: u8 = 10;

/// Addon search paths for shards 1..=n (n clamped to 1..=10): "citadel/addons", "citadel/addons2", ...
pub fn addon_paths(shard_count: u8) -> Vec<String> {
    let n = shard_count.clamp(1, MAX_SHARDS);
    (1..=n)
        .map(|shard| {
            if shard == 1 {
                "citadel/addons".to_string()
            } else {
                format!("citadel/addons{shard}")
            }
        })
        .collect()
}

/// true for "citadel/addons" and "citadel/addons2".."citadel/addons10" (case-insensitive, no trailing slash)
pub fn is_managed_addon_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("citadel/addons") else {
        return false;
    };
    if rest.is_empty() {
        return true;
    }
    if rest.starts_with('0') || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    matches!(rest.parse::<u8>(), Ok(2..=MAX_SHARDS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addon_paths_are_clamped() {
        assert_eq!(addon_paths(0), vec!["citadel/addons"]);
        assert_eq!(addon_paths(1), vec!["citadel/addons"]);
        assert_eq!(
            addon_paths(3),
            vec!["citadel/addons", "citadel/addons2", "citadel/addons3"]
        );
        assert_eq!(addon_paths(10).len(), 10);
        assert_eq!(addon_paths(10)[9], "citadel/addons10");
        assert_eq!(addon_paths(200).len(), 10);
    }

    #[test]
    fn managed_addon_path_matching() {
        for ok in [
            "citadel/addons",
            "Citadel/Addons",
            "citadel/addons2",
            "citadel/addons9",
            "citadel/addons10",
        ] {
            assert!(is_managed_addon_path(ok), "{ok}");
        }
        for bad in [
            "citadel/addons1",
            "citadel/addons11",
            "citadel/addons02",
            "citadel/addons/",
            "citadel/addonsx",
            "citadel",
            "core",
            "",
        ] {
            assert!(!is_managed_addon_path(bad), "{bad}");
        }
    }
}
