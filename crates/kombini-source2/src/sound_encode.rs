//! Building `.vsnd_c` files from WAV audio, so voice packs that ship plain
//! WAVs can replace the game's clips. Written for Kombini.
//!
//! The game clip being replaced is the template: its `CTRL` block is patched
//! byte-faithfully to describe 16-bit PCM (format, rate, channels, sample
//! count, duration, streaming size), every other block is kept, and the PCM is
//! appended after the resource, where [`crate::sound::decode_sound`] and the
//! engine read the audio from.

use crate::kv3::{self, node, Node, Val, Value};
use crate::resource::Resource;
use crate::DecodeError;

/// Interleaved 16-bit PCM read from a WAV file.
#[derive(Debug, Clone, PartialEq)]
pub struct Pcm {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels.max(1))
    }

    /// The same audio with `channels` channels: averaged down to mono, or the
    /// first channel repeated.
    pub fn with_channels(&self, channels: u16) -> Pcm {
        let from = usize::from(self.channels.max(1));
        let to = usize::from(channels.max(1));
        if from == to {
            return self.clone();
        }
        let mut samples = Vec::with_capacity(self.frames() * to);
        for frame in self.samples.chunks_exact(from) {
            if to == 1 {
                let sum: i32 = frame.iter().map(|&s| i32::from(s)).sum();
                samples.push((sum / from as i32) as i16);
            } else {
                for c in 0..to {
                    samples.push(frame[c.min(from - 1)]);
                }
            }
        }
        Pcm {
            rate: self.rate,
            channels: to as u16,
            samples,
        }
    }
}

fn err(msg: &str) -> DecodeError {
    DecodeError::Sound(msg.to_owned())
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Reads a RIFF/WAVE file: integer PCM (8, 16, 24 or 32 bit) or 32-bit float,
/// converted to 16-bit samples.
pub fn read_wav(bytes: &[u8]) -> Result<Pcm, DecodeError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(err("not a WAV file"));
    }
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut data: Option<&[u8]> = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32_at(bytes, at + 4) as usize;
        let body = &bytes[at + 8..(at + 8).saturating_add(size).min(bytes.len())];
        if id == b"fmt " && body.len() >= 16 {
            let mut tag = u16_at(body, 0);
            if tag == 0xFFFE && body.len() >= 26 {
                // WAVE_FORMAT_EXTENSIBLE: the real format leads the sub-format GUID.
                tag = u16_at(body, 24);
            }
            fmt = Some((tag, u16_at(body, 2), u32_at(body, 4), u16_at(body, 14)));
        } else if id == b"data" {
            data = Some(body);
        }
        at = (at + 8).saturating_add(size).saturating_add(size & 1);
    }
    let (tag, channels, rate, bits) = fmt.ok_or_else(|| err("WAV file has no fmt chunk"))?;
    let data = data.ok_or_else(|| err("WAV file has no data chunk"))?;
    if channels == 0 || channels > 8 || rate == 0 {
        return Err(err("WAV file has a bad channel count or rate"));
    }
    let samples: Vec<i16> = match (tag, bits) {
        (1, 8) => data.iter().map(|&b| (i16::from(b) - 128) << 8).collect(),
        (1, 16) => data
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect(),
        (1, 24) => data
            .chunks_exact(3)
            .map(|c| i16::from_le_bytes([c[1], c[2]]))
            .collect(),
        (1, 32) => data
            .chunks_exact(4)
            .map(|c| i16::from_le_bytes([c[2], c[3]]))
            .collect(),
        (3, 32) => data
            .chunks_exact(4)
            .map(|c| {
                let f = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                (f.clamp(-1., 1.) * 32767.) as i16
            })
            .collect(),
        _ => {
            return Err(DecodeError::Sound(format!(
                "WAV encoding {tag} with {bits}-bit samples is not supported"
            )));
        }
    };
    let whole = samples.len() - samples.len() % usize::from(channels);
    let mut samples = samples;
    samples.truncate(whole);
    if samples.is_empty() {
        return Err(err("WAV file has no audio"));
    }
    Ok(Pcm {
        rate,
        channels,
        samples,
    })
}

/// Points the `m_vSound` member `key` at `val`, giving it a tag wide enough
/// for the value (a template's narrow or implied tag may not fit).
fn set_member(sound: &mut Node, key: &str, tag: u8, val: Val) -> Result<(), DecodeError> {
    let Val::Object(members) = &mut sound.val else {
        return Err(err("m_vSound is not an object"));
    };
    let node = members
        .iter_mut()
        .find(|(k, _)| k == key)
        .map(|(_, n)| n)
        .ok_or_else(|| DecodeError::Sound(format!("template sound has no {key}")))?;
    node.tag = tag;
    node.val = val;
    Ok(())
}

/// Builds a `.vsnd_c` holding `pcm` as 16-bit PCM, using the game clip
/// `template` for every block but the sound description. The audio takes the
/// template's channel count, so a stereo WAV replacing a mono voice line is
/// mixed down.
pub fn encode_pcm16(template: &[u8], pcm: &Pcm) -> Result<Vec<u8>, DecodeError> {
    let resource = Resource::parse(template)?;
    let index = resource
        .blocks()
        .iter()
        .position(|b| &b.kind == b"CTRL")
        .ok_or_else(|| err("template sound has no CTRL block"))?;
    let ctrl = resource
        .find_block(*b"CTRL")
        .ok_or_else(|| err("template sound has no CTRL block"))?;
    let mut doc = kv3::decode_lossless(ctrl)?;
    let Val::Object(root) = &mut doc.root.val else {
        return Err(err("template CTRL is not an object"));
    };
    let sound = root
        .iter_mut()
        .find(|(k, _)| k == "m_vSound")
        .map(|(_, n)| n)
        .ok_or_else(|| err("template sound has no m_vSound"))?;
    let channels = match sound.get("m_nChannels").map(|n| n.to_value()) {
        Some(Value::Int(i)) => i,
        Some(Value::UInt(u)) => u as i64,
        _ => 1,
    }
    .clamp(1, 2) as u16;
    let pcm = pcm.with_channels(channels);
    let frames = pcm.frames();
    let audio: Vec<u8> = pcm.samples.iter().flat_map(|s| s.to_le_bytes()).collect();

    set_member(
        sound,
        "m_nFormat",
        node::STRING,
        Val::String("PCM16".into()),
    )?;
    set_member(sound, "m_nRate", node::INT32, Val::Int(i64::from(pcm.rate)))?;
    set_member(
        sound,
        "m_nSampleCount",
        node::UINT32,
        Val::UInt(frames as u64),
    )?;
    set_member(
        sound,
        "m_nStreamingSize",
        node::UINT32,
        Val::UInt(audio.len() as u64),
    )?;
    set_member(
        sound,
        "m_flDuration",
        node::DOUBLE,
        Val::F64(frames as f64 / f64::from(pcm.rate)),
    )?;
    let ctrl = kv3::encode_v5(&doc)?;
    let mut out = resource.rebuild_with_block(index, &ctrl)?;
    out.extend_from_slice(&audio);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav16(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&channels.to_le_bytes());
        w.extend_from_slice(&rate.to_le_bytes());
        w.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
        w.extend_from_slice(&(channels * 2).to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        // An odd-sized chunk before data exercises the pad byte.
        w.extend_from_slice(b"LIST");
        w.extend_from_slice(&3u32.to_le_bytes());
        w.extend_from_slice(&[1, 2, 3, 0]);
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(data.len() as u32).to_le_bytes());
        w.extend_from_slice(&data);
        w
    }

    #[test]
    fn reads_pcm16_wav() {
        let pcm = read_wav(&wav16(22_050, 2, &[1, 2, 3, 4, 5])).unwrap();
        assert_eq!(pcm.rate, 22_050);
        assert_eq!(pcm.channels, 2);
        // The dangling half frame is dropped.
        assert_eq!(pcm.samples, vec![1, 2, 3, 4]);
    }

    #[test]
    fn mixes_channels() {
        let pcm = Pcm {
            rate: 1,
            channels: 2,
            samples: vec![10, 20, -4, 0],
        };
        assert_eq!(pcm.with_channels(1).samples, vec![15, -2]);
        let mono = pcm.with_channels(1);
        assert_eq!(mono.with_channels(2).samples, vec![15, 15, -2, -2]);
    }

    #[test]
    fn rejects_non_wav() {
        assert!(read_wav(b"PACKxxxxxxxx").is_err());
    }
}
