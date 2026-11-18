//! Source 2 resource container and binary KeyValues3 codec.
//!
//! Vendored from `morphic` 0.11.9 (part of <https://github.com/Slush97/vpkmerge>,
//! commit df4158673f4e700b9dda4db76ed9f193440b24ba, MIT, Copyright (c) 2026 esoc;
//! see `LICENSE-morphic`). morphic itself ports layouts and algorithms from
//! ValveResourceFormat (MIT). Only the `kv3` and `resource` modules and the
//! decode error type are included; texture, model, material and sound code is
//! not. Local changes: the error type is trimmed, tests that need the omitted
//! modules are removed, and lint allowances are added for this workspace.
//! `sound` and `texture` are Kombini's own `.vsnd_c` and `.vtex_c` readers.

#![allow(
    clippy::all,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    dead_code
)]

mod error;
pub mod kv3;
pub mod resource;
pub mod sound;
pub mod texture;

pub use error::DecodeError;

/// Decodes the KV3 `DATA` block of a compiled resource (`.vdata_c`, `.vmdl_c`, ...).
pub fn decode_kv3_resource(file_bytes: &[u8]) -> Result<kv3::Value, DecodeError> {
    let resource = resource::Resource::parse(file_bytes)?;
    let data = resource.data_block()?;
    kv3::decode(data)
}
