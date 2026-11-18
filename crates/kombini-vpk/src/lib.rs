//! Valve VPK v1/v2 reader, writer and merger
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod bundle;
pub mod error;
pub mod extract;
pub mod merge;
pub mod paths;
pub mod query;
pub mod read;
pub mod write;

pub use bundle::{VpkBundle, group_bundles};
pub use error::{Result, VpkError};
pub use extract::{ExtractOptions, extract, safe_relpath};
pub use merge::{CollisionPolicy, MergeOptions, MergeReport, merge_vpks};
pub use paths::{
    enabled_chunk_name, enabled_root_name, is_canonical_enabled_root, parse_chunk_name,
    parse_enabled_root, root_base,
};
pub use query::{EntryPage, EntryQuery, EntryRow, FolderNode, SortKey, VpkIndex};
pub use read::{
    EMBEDDED, MAX_ENTRIES, MAX_TREE_SIZE, SIGNATURE, VerifyReport, VpkArchive, VpkEntry, is_vpk,
};
pub use write::{EntrySource, WriteReport, write_vpk};
