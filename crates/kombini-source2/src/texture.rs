//! Uncompressed compiled textures (`.vtex_c`), enough for UI images such as
//! hero portraits. The `DATA` block holds the header (width, height, depth,
//! format, mip count); the pixels follow the resource, smallest mip first.
//! Block-compressed formats (DXT, BC7) and compressed mip chains are
//! reported as unsupported. Written for Kombini; the layout matches
//! ValveResourceFormat's texture reader.

use crate::resource::Resource;
use crate::DecodeError;

/// `VTexFormat` values handled here.
const RGBA8888: u8 = 4;
const BGRA8888: u8 = 28;

/// An 8-bit RGBA image, rows top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}

/// Decodes the largest mip of an uncompressed 8-bit texture.
pub fn decode_texture(file: &[u8]) -> Result<RgbaImage, DecodeError> {
    let resource = Resource::parse(file)?;
    let header = resource.data_block()?;
    let bad = |what: &str| DecodeError::Texture(what.to_owned());
    let width = u16_at(header, 20).ok_or_else(|| bad("short header"))? as usize;
    let height = u16_at(header, 22).ok_or_else(|| bad("short header"))? as usize;
    let depth = u16_at(header, 24)
        .ok_or_else(|| bad("short header"))?
        .max(1) as usize;
    let format = *header.get(26).ok_or_else(|| bad("short header"))?;
    let mips = (*header.get(27).ok_or_else(|| bad("short header"))?).max(1) as usize;
    if width == 0 || height == 0 || depth != 1 {
        return Err(bad("not a flat 2D texture"));
    }
    if format != RGBA8888 && format != BGRA8888 {
        return Err(DecodeError::Texture(format!(
            "format {format} is not supported"
        )));
    }
    let expected: usize = (0..mips)
        .map(|m| (width >> m).max(1) * (height >> m).max(1) * 4)
        .sum();
    let declared = file
        .get(0..4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
        .unwrap_or(0);
    let data = file.get(declared..).unwrap_or(&[]);
    // A different length means compressed mips or an unknown layout.
    if data.len() != expected {
        return Err(bad("unexpected pixel data size (compressed mips?)"));
    }
    let mut pixels = data[data.len() - width * height * 4..].to_vec();
    if format == BGRA8888 {
        for px in pixels.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
    }
    Ok(RgbaImage {
        width: width as u32,
        height: height as u32,
        pixels,
    })
}

/// The compiled path of a panorama image reference such as
/// `file://{images}/heroes/bebop_card.psd`
/// (`panorama/images/heroes/bebop_card_psd.vtex_c`).
pub fn panorama_image_path(reference: &str) -> Option<String> {
    let rest = reference.strip_prefix("file://{images}/")?;
    let (stem, ext) = rest.rsplit_once('.')?;
    Some(format!("panorama/images/{stem}_{ext}.vtex_c").to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::panorama_image_path;

    #[test]
    fn panorama_paths() {
        assert_eq!(
            panorama_image_path("file://{images}/heroes/bebop_card.psd").as_deref(),
            Some("panorama/images/heroes/bebop_card_psd.vtex_c")
        );
        assert_eq!(panorama_image_path("heroes/x.psd"), None);
    }
}
