//! Imports and exports: adopting unmanaged addons (and DMM manifests),
//! Deadlock Mod Interchange bundles, Grimoire `mod-profile`/`mp1:` codes, and
//! whole libraries from Deadlock Mod Manager and Grimoire.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod interchange;
pub mod managers;
pub mod modprofile;
pub mod unmanaged;

#[cfg(test)]
mod tests;
