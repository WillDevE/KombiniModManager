//! Byte <-> text conversion that remembers BOM, encoding and line endings.

use serde::Serialize;

const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

/// Encoding detected for a gameinfo.gi file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Encoding {
    Utf8,
    Latin1,
}

/// Decoded file text plus the facts needed to write it back unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedText {
    pub text: String,
    pub bom: bool,
    pub encoding: Encoding,
    pub crlf: bool,
}

/// Strips a UTF-8 BOM; invalid UTF-8 falls back to Latin-1 (each byte becomes one char).
pub fn decode(bytes: &[u8]) -> DecodedText {
    let (bom, body) = match bytes.strip_prefix(&UTF8_BOM) {
        Some(rest) => (true, rest),
        None => (false, bytes),
    };
    let (text, encoding) = match std::str::from_utf8(body) {
        Ok(s) => (s.to_owned(), Encoding::Utf8),
        Err(_) => (
            body.iter().map(|&b| char::from(b)).collect(),
            Encoding::Latin1,
        ),
    };
    let crlf = text.contains("\r\n");
    DecodedText {
        text,
        bom,
        encoding,
        crlf,
    }
}

/// Re-adds the BOM when `d.bom`; Latin-1 maps chars up to U+00FF back to bytes, others to `?`.
pub fn encode(d: &DecodedText, text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 3);
    if d.bom {
        out.extend_from_slice(&UTF8_BOM);
    }
    match d.encoding {
        Encoding::Utf8 => out.extend_from_slice(text.as_bytes()),
        Encoding::Latin1 => {
            for c in text.chars() {
                out.push(u8::try_from(u32::from(c)).unwrap_or(b'?'));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_bom_lf_round_trip() {
        let bytes = b"\xEF\xBB\xBFabc\ndef\n";
        let d = decode(bytes);
        assert!(d.bom);
        assert!(!d.crlf);
        assert_eq!(d.encoding, Encoding::Utf8);
        assert_eq!(d.text, "abc\ndef\n");
        assert_eq!(encode(&d, &d.text), bytes);
    }

    #[test]
    fn crlf_detected() {
        let d = decode(b"a\r\nb\r\n");
        assert!(d.crlf);
        assert!(!d.bom);
    }

    #[test]
    fn latin1_fallback_round_trip() {
        let bytes = b"// caf\xE9\r\n";
        let d = decode(bytes);
        assert_eq!(d.encoding, Encoding::Latin1);
        assert_eq!(d.text, "// caf\u{e9}\r\n");
        assert_eq!(encode(&d, &d.text), bytes);
    }

    #[test]
    fn latin1_unmappable_becomes_question_mark() {
        let d = decode(b"\xE9");
        assert_eq!(encode(&d, "\u{20AC}x"), b"?x");
    }
}
