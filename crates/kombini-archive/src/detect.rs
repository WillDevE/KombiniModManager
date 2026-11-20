//! Magic-byte archive format detection. File extensions are never consulted.

use std::fs::File;
use std::io::Read;
use std::path::Path;

/// The container format of a file, decided from its first bytes only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveKind {
    /// ZIP archive.
    Zip,
    /// 7-Zip archive.
    SevenZ,
    /// RAR archive (RAR4 or RAR5).
    Rar,
    /// A bare Valve package file (not an archive container, copied as-is).
    Vpk,
    /// A Quake-style `PACK` file (some voice packs ship their WAVs this way).
    Pak,
    /// Anything else.
    Unknown,
}

/// Number of leading bytes needed to recognise every supported format.
const HEAD_LEN: u64 = 8;

/// Detects the format of `head`, the first bytes of a file.
///
/// Magic bytes: ZIP `PK\x03\x04` / `PK\x05\x06` / `PK\x07\x08`; 7z `37 7A BC AF 27 1C`;
/// RAR4 `Rar!\x1A\x07\x00`; RAR5 `Rar!\x1A\x07\x01\x00`; VPK `34 12 AA 55`; PAK `PACK`.
pub fn detect_bytes(head: &[u8]) -> ArchiveKind {
    if head.starts_with(b"PK\x03\x04")
        || head.starts_with(b"PK\x05\x06")
        || head.starts_with(b"PK\x07\x08")
    {
        ArchiveKind::Zip
    } else if head.starts_with(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]) {
        ArchiveKind::SevenZ
    } else if head.starts_with(b"Rar!\x1A\x07\x01\x00") || head.starts_with(b"Rar!\x1A\x07\x00") {
        ArchiveKind::Rar
    } else if head.starts_with(&[0x34, 0x12, 0xAA, 0x55]) {
        ArchiveKind::Vpk
    } else if head.starts_with(b"PACK") {
        ArchiveKind::Pak
    } else {
        ArchiveKind::Unknown
    }
}

/// Reads the first bytes of `path` and detects its format. The extension is ignored.
pub fn detect(path: &Path) -> std::io::Result<ArchiveKind> {
    let file = File::open(path)?;
    let mut head = Vec::with_capacity(HEAD_LEN as usize);
    file.take(HEAD_LEN).read_to_end(&mut head)?;
    Ok(detect_bytes(&head))
}
