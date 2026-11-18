//! Compiled sounds (`.vsnd_c`): the `CTRL` block describes the sound
//! (`m_vSound`: rate, format, channels, streaming size) and the audio itself
//! follows the resource, starting at the file size declared in the header.
//! Written for Kombini (not part of the vendored morphic code); the layout
//! matches ValveResourceFormat's sound reader.

use crate::kv3::Value;
use crate::resource::Resource;
use crate::DecodeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundFormat {
    Pcm16,
    Pcm8,
    Mp3,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SoundInfo {
    pub format: SoundFormat,
    pub rate: u32,
    pub channels: u16,
    pub duration: f64,
}

/// A sound ready for a player: MP3 frames as stored, or PCM wrapped in WAV.
#[derive(Debug, Clone)]
pub struct DecodedSound {
    pub info: SoundInfo,
    pub bytes: Vec<u8>,
}

fn int(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    v.as_int()
        .or_else(|| v.as_uint().and_then(|u| i64::try_from(u).ok()))
}

/// Decodes a `.vsnd_c` file into something a player can open.
pub fn decode_sound(file: &[u8]) -> Result<DecodedSound, DecodeError> {
    let resource = Resource::parse(file)?;
    let ctrl = resource
        .find_block(*b"CTRL")
        .ok_or_else(|| DecodeError::Sound("sound without a CTRL block".into()))?;
    let ctrl = crate::kv3::decode(ctrl)?;
    let sound = ctrl
        .get("m_vSound")
        .ok_or_else(|| DecodeError::Sound("CTRL block has no m_vSound".into()))?;
    let format = match sound.get("m_nFormat").and_then(Value::as_str) {
        Some("PCM16") => SoundFormat::Pcm16,
        Some("PCM8") => SoundFormat::Pcm8,
        Some("MP3") => SoundFormat::Mp3,
        other => {
            return Err(DecodeError::Sound(format!(
                "sound format {}",
                other.unwrap_or("unknown")
            )));
        }
    };
    let rate = int(sound.get("m_nRate"))
        .unwrap_or(44_100)
        .clamp(1, 384_000) as u32;
    let channels = int(sound.get("m_nChannels")).unwrap_or(1).clamp(1, 8) as u16;
    let duration = sound
        .get("m_flDuration")
        .and_then(Value::as_f64)
        .unwrap_or(0.);
    let header_size = file
        .get(0..4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
        .unwrap_or(0);
    let start = header_size.min(file.len());
    let declared = int(sound.get("m_nStreamingSize"))
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(usize::MAX);
    let end = start.saturating_add(declared).min(file.len());
    let audio = &file[start..end];
    if audio.is_empty() {
        return Err(DecodeError::Sound("sound has no audio data".into()));
    }
    let info = SoundInfo {
        format,
        rate,
        channels,
        duration,
    };
    let bytes = match format {
        SoundFormat::Mp3 => audio.to_vec(),
        SoundFormat::Pcm16 => wav(audio, rate, channels, 16),
        SoundFormat::Pcm8 => wav(audio, rate, channels, 8),
    };
    Ok(DecodedSound { info, bytes })
}

/// A minimal RIFF/WAVE file around interleaved PCM.
fn wav(pcm: &[u8], rate: u32, channels: u16, bits: u16) -> Vec<u8> {
    let block_align = channels * bits / 8;
    // Whole frames only.
    let len = pcm.len() - pcm.len() % block_align.max(1) as usize;
    let pcm = &pcm[..len];
    let mut out = Vec::with_capacity(44 + len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + len as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * block_align as u32).to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(len as u32).to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

#[cfg(test)]
mod tests {
    use super::wav;

    #[test]
    fn wav_header() {
        let w = wav(&[1, 2, 3, 4, 5], 44_100, 2, 16);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        // 5 bytes trimmed to one whole 4-byte frame.
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 4);
        assert_eq!(w.len(), 48);
        assert_eq!(u16::from_le_bytes(w[32..34].try_into().unwrap()), 4);
    }
}
