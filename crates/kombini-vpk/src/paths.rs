//! File-name helpers for VPK roots, chunks and the game's `pakNN_dir.vpk` slots.

fn ends_with_ci(s: &str, suffix: &str) -> bool {
    s.len() >= suffix.len()
        && s.as_bytes()[s.len() - suffix.len()..].eq_ignore_ascii_case(suffix.as_bytes())
}

/// Parse `<base>_<NNN>.vpk` (case-insensitive) into `(base, index)`.
pub fn parse_chunk_name(file_name: &str) -> Option<(String, u16)> {
    if !ends_with_ci(file_name, ".vpk") {
        return None;
    }
    let stem = file_name.as_bytes().get(..file_name.len() - 4)?;
    // Need at least one base byte plus "_NNN".
    if stem.len() < 5 {
        return None;
    }
    let split = stem.len() - 4;
    let tail = &stem[split..];
    if tail[0] != b'_' || !tail[1..].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let index = tail[1..]
        .iter()
        .fold(0u16, |acc, d| acc * 10 + u16::from(d - b'0'));
    // `split` follows an ASCII '_' boundary only when the preceding bytes are valid UTF-8, which
    // holds because the tail is pure ASCII.
    let base = file_name.get(..split)?;
    Some((base.to_string(), index))
}

/// Strip `.vpk` (case-insensitive) and then a trailing `_dir` (case-insensitive).
pub fn root_base(file_name: &str) -> String {
    let mut base = file_name;
    if ends_with_ci(base, ".vpk") {
        base = &base[..base.len() - 4];
    }
    if ends_with_ci(base, "_dir") {
        base = &base[..base.len() - 4];
    }
    base.to_string()
}

/// Parse `pak<digits>_dir.vpk` (case-insensitive); returns the slot when it is within `1..=99`.
pub fn parse_enabled_root(file_name: &str) -> Option<u8> {
    if file_name.len() < "pak1_dir.vpk".len() || !ends_with_ci(file_name, "_dir.vpk") {
        return None;
    }
    let bytes = file_name.as_bytes();
    if !bytes[..3].eq_ignore_ascii_case(b"pak") {
        return None;
    }
    let digits = &bytes[3..bytes.len() - "_dir.vpk".len()];
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let first_nonzero = digits
        .iter()
        .position(|&d| d != b'0')
        .unwrap_or(digits.len());
    let significant = &digits[first_nonzero..];
    if significant.is_empty() || significant.len() > 2 {
        return None;
    }
    let value = significant.iter().fold(0u8, |acc, d| acc * 10 + (d - b'0'));
    (1..=99).contains(&value).then_some(value)
}

/// True for exactly `pakNN_dir.vpk` with NN in `01..=99` (case-insensitive).
pub fn is_canonical_enabled_root(file_name: &str) -> bool {
    file_name.len() == "pak01_dir.vpk".len()
        && file_name.as_bytes()[3..5].iter().all(u8::is_ascii_digit)
        && parse_enabled_root(file_name).is_some()
}

/// `pak05_dir.vpk` for slot 5.
pub fn enabled_root_name(slot: u8) -> String {
    format!("pak{slot:02}_dir.vpk")
}

/// `pak05_003.vpk` for slot 5, chunk index 3.
pub fn enabled_chunk_name(slot: u8, index: u16) -> String {
    format!("pak{slot:02}_{index:03}.vpk")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_names() {
        assert_eq!(parse_chunk_name("pak01_000.vpk"), Some(("pak01".into(), 0)));
        assert_eq!(
            parse_chunk_name("My_Mod_012.VPK"),
            Some(("My_Mod".into(), 12))
        );
        assert_eq!(parse_chunk_name("pak01_dir.vpk"), None);
        assert_eq!(parse_chunk_name("_000.vpk"), None);
        assert_eq!(parse_chunk_name("a_00.vpk"), None);
        assert_eq!(parse_chunk_name("a_0000.vpk"), None);
        assert_eq!(parse_chunk_name("a_000.txt"), None);
        assert_eq!(parse_chunk_name("é_000.vpk"), Some(("é".into(), 0)));
    }

    #[test]
    fn root_bases() {
        assert_eq!(root_base("pak01_dir.vpk"), "pak01");
        assert_eq!(root_base("PAK01_DIR.VPK"), "PAK01");
        assert_eq!(root_base("cool_skin.vpk"), "cool_skin");
        assert_eq!(root_base("odd"), "odd");
    }

    #[test]
    fn enabled_roots() {
        assert_eq!(parse_enabled_root("pak1_dir.vpk"), Some(1));
        assert_eq!(parse_enabled_root("pak001_dir.vpk"), Some(1));
        assert_eq!(parse_enabled_root("pak00_dir.vpk"), None);
        assert_eq!(parse_enabled_root("pak100_dir.vpk"), None);
        assert_eq!(parse_enabled_root("PAK07_DIR.VPK"), Some(7));
        assert_eq!(parse_enabled_root("pak_dir.vpk"), None);
        assert_eq!(parse_enabled_root("pakx1_dir.vpk"), None);
        assert_eq!(parse_enabled_root("pak99_dir.vpk"), Some(99));
        assert_eq!(parse_enabled_root("pak01_000.vpk"), None);
    }

    #[test]
    fn canonical_roots() {
        assert!(is_canonical_enabled_root("pak01_dir.vpk"));
        assert!(is_canonical_enabled_root("PAK99_DIR.VPK"));
        assert!(!is_canonical_enabled_root("pak1_dir.vpk"));
        assert!(!is_canonical_enabled_root("pak001_dir.vpk"));
        assert!(!is_canonical_enabled_root("pak00_dir.vpk"));
    }

    #[test]
    fn generated_names() {
        assert_eq!(enabled_root_name(5), "pak05_dir.vpk");
        assert_eq!(enabled_chunk_name(5, 3), "pak05_003.vpk");
        assert!(is_canonical_enabled_root(&enabled_root_name(12)));
    }
}
