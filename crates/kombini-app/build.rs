//! Gives the app's main thread an 8 MiB stack on Windows. The default 1 MiB
//! is too small for GPUI's layout of deep element trees in debug builds,
//! where stack frames are large (switching tabs overflowed it).

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("windows-msvc") {
        println!("cargo:rustc-link-arg-bins=/STACK:8388608");
    } else if target.contains("windows-gnu") {
        println!("cargo:rustc-link-arg-bins=-Wl,--stack,8388608");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
