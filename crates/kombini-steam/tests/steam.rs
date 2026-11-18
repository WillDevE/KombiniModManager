//! Live, read-only discovery check. Run with `cargo test -p kombini-steam -- --ignored --nocapture`.

#[test]
#[ignore]
fn live_discovery() {
    let dirs = kombini_steam::find_steam_dirs();
    println!("steam dirs: {dirs:?}");
    let installs = kombini_steam::find_deadlock_installs();
    println!("installs: {installs:?}");
    if let Some(first) = installs.first() {
        println!("build: {:?}", kombini_steam::read_build_info(&first.root));
        println!(
            "steam for root: {:?}",
            kombini_steam::steam_dir_for(&first.root)
        );
    }
    let running = kombini_steam::ProcessProbe::new().is_running();
    println!("deadlock running: {running}");
}
