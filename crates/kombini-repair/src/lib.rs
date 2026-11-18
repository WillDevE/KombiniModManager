//! Mod compatibility checks and repairs for Deadlock VPK mods.
//!
//! Ported from vpkmerge-core (<https://github.com/Slush97/vpkmerge>, commit
//! df4158673f4e700b9dda4db76ed9f193440b24ba, MIT, Copyright (c) 2026 esoc),
//! the engine behind the Grimoire mod manager's "Mod issues" repairs; see
//! `LICENSE-vpkmerge`. Every check compares a mod's copy of a game file with
//! the installed game's own copy:
//!
//! - [`vdata_check`]: a mod shipping an older `.vdata_c` silently deletes
//!   heroes, abilities or fields Valve added since (detection only).
//! - [`compatibility`]: fills absent `heroes`/`abilities` data fields, missing
//!   localization strings, absent model sound bindings, and swaps in the game's
//!   current skeleton sampling when the bone structure provably still matches.
//!   Values the mod author set are never changed.
//! - [`attachment_heal`]: model swaps built before Valve re-framed the
//!   `root_motion` attachments break the hero camera; restores the game's
//!   camera/aim attachment data.
//!
//! Repairs write a new archive and verify every entry against either the
//! intended replacement or the input; they never touch the input or the game.
//!
//! Local changes from upstream: VPK access goes through [`pak`] (on
//! `kombini-vpk`) instead of `valve_pak`, and the KV3/resource code comes from
//! `kombini-source2` (vendored `morphic`).

#![allow(
    clippy::all,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used
)]

pub mod attachment_heal;
pub mod compatibility;
mod model_metadata;
pub mod pak;
pub mod vdata_check;

pub use attachment_heal::{
    AttachmentHealer, AttachmentReport, AttachmentStatus, HealedModel, STALE_DEGREES,
};
pub use compatibility::{CompatibilityRepairer, CompatibilityReport, RepairStatus};
pub use vdata_check::{check_vdata, VdataChecker, VdataReport, VdataStatus};
