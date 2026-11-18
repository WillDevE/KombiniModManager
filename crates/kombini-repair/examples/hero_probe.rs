//! Decodes a few hero portraits from the game's `pak01_dir.vpk`:
//! `cargo run -p kombini-repair --example hero_probe -- <path to pak01_dir.vpk>`
fn main() {
    let pak = std::env::args()
        .nth(1)
        .expect("usage: hero_probe <path to game/citadel/pak01_dir.vpk>");
    let a = kombini_vpk::VpkArchive::open(std::path::Path::new(&pak)).unwrap();
    for (n, out) in [
        ("panorama/images/heroes/bebop_sm_psd.vtex_c", "sm"),
        ("panorama/images/heroes/bebop_card_psd.vtex_c", "card"),
        ("panorama/images/heroes/bebop_vertical_psd.vtex_c", "vert"),
    ] {
        let b = a.read_file(n).unwrap();
        let r = kombini_source2::resource::Resource::parse(&b).unwrap();
        let d = r.find_block(*b"DATA").unwrap();
        let w = u16::from_le_bytes([d[20], d[21]]) as usize;
        let h = u16::from_le_bytes([d[22], d[23]]) as usize;
        println!(
            "{out} {w}x{h} fmt {} mips {} extra {:?}",
            d[26],
            d[27],
            &d[32..52]
        );
        let fs = u32::from_le_bytes(b[0..4].try_into().unwrap()) as usize;
        let px = &b[b.len() - w * h * 4..];
        // BGRA -> PPM (drop alpha) for a quick look
        let mut ppm = format!("P6 {w} {h} 255\n").into_bytes();
        for c in px.chunks(4) {
            ppm.extend_from_slice(&[c[2], c[1], c[0]]);
        }
        let _ = fs;
        std::fs::write(format!("target/tmp/{out}.ppm"), ppm).unwrap();
    }
}
