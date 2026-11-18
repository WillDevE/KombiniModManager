//! Domain types and pure algorithms for the Kombini Deadlock mod manager.
//!
//! This crate has no IO and no knowledge of SQLite or Tauri. Everything here is
//! deterministic and unit-tested; the engine composes it with the filesystem.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod conflicts;
pub mod error;
pub mod freshness;
pub mod heroes;
pub mod ids;
pub mod layout;
pub mod model;
pub mod naming;
pub mod order;
pub mod safety;

pub use conflicts::{Conflict, find_conflicts};
pub use error::{AppError, ErrorCode};
pub use ids::{BlobHash, GameId, GroupId, ModId, ProfileId};
pub use layout::{Layout, LayoutError, assign_layout};
pub use model::*;
pub use order::effective_order;

/// Maximum number of addon folders ("shards") the manager uses.
pub const MAX_SHARDS: u8 = 10;
/// Maximum mounted `pakNN_dir.vpk` roots per addon folder.
pub const SLOTS_PER_SHARD: u8 = 99;
/// Deadlock's Steam app id.
pub const APP_ID: u32 = 1_422_450;
