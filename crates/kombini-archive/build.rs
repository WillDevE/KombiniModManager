//! The vendored UnRAR sources call Windows registry, token and crypto APIs (advapi32) but
//! `unrar_sys` does not emit the link directive for them, which breaks linking of test and
//! final binaries on MSVC. Link advapi32 explicitly on Windows.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        println!("cargo:rustc-link-lib=advapi32");
    }
}
