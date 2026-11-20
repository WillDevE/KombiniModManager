//! Voice packs that ship plain WAV files (loose, zipped or in a Quake-style
//! `.pak`) instead of a VPK. Each WAV named after a game clip
//! (`nano_desperation_power1_01.wav` for `sounds/vo/nano/...vsnd_c`) is
//! encoded as that clip, using the game's own file as the template, and the
//! results are packed into one VPK the normal install path takes from there.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use kombini_core::{AppError, ErrorCode};

use crate::Result;
use kombini_vpk::{EntrySource, VpkArchive};

/// What [`build_sound_pack`] made.
pub(crate) struct SoundPack {
    /// Number of clips encoded.
    pub converted: usize,
    /// WAV files that match no game clip, or could not be read.
    pub skipped: Vec<String>,
}

fn wav_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            wav_files(&path, out);
        } else if kind.is_file()
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("wav"))
        {
            out.push(path);
        }
    }
}

/// True when `dir` holds WAV files but no VPK.
pub(crate) fn is_sound_pack(dir: &Path) -> bool {
    if !kombini_archive::discover_vpks(dir).is_empty() {
        return false;
    }
    let mut wavs = Vec::new();
    wav_files(dir, &mut wavs);
    !wavs.is_empty()
}

/// Game clips by lowercase file stem (`nano_desperation_power1_01`).
fn clips_by_stem(game: &VpkArchive) -> BTreeMap<String, Vec<String>> {
    let mut by_stem: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in game.file_paths() {
        let lower = path.to_ascii_lowercase();
        if !lower.starts_with("sounds/") {
            continue;
        }
        let Some(stem) = lower.strip_suffix(".vsnd_c") else {
            continue;
        };
        let stem = stem.rsplit('/').next().unwrap_or(stem).to_owned();
        by_stem.entry(stem).or_default().push(path.to_owned());
    }
    by_stem
}

/// The game clip a WAV replaces: the clip at the same path when the WAV sits
/// in a `sounds/...` folder, otherwise the only clip with its name (a voice
/// line before any other kind of sound when names repeat).
fn target_for(wav: &Path, dir: &Path, by_stem: &BTreeMap<String, Vec<String>>) -> Option<String> {
    let stem = wav.file_stem()?.to_string_lossy().to_ascii_lowercase();
    let candidates = by_stem.get(&stem)?;
    let rel = wav
        .strip_prefix(dir)
        .ok()?
        .with_extension("vsnd_c")
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    candidates
        .iter()
        .find(|c| rel.ends_with(&c.to_ascii_lowercase()))
        .or_else(|| candidates.iter().find(|c| c.starts_with("sounds/vo/")))
        .or_else(|| candidates.first())
        .cloned()
}

/// Encodes every WAV under `dir` that names a game clip and writes the clips
/// to `out` (a `pak01_dir.vpk`). `game_pak` is the game's `pak01_dir.vpk`,
/// read for the templates only.
pub(crate) fn build_sound_pack(
    dir: &Path,
    game_pak: &Path,
    out: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<SoundPack> {
    let mut wavs = Vec::new();
    wav_files(dir, &mut wavs);
    wavs.sort();
    let game = VpkArchive::open(game_pak).map_err(|e| {
        AppError::new(
            ErrorCode::InvalidVpk,
            format!("Cannot read the game's sounds: {e}"),
        )
    })?;
    let by_stem = clips_by_stem(&game);
    let mut entries: Vec<(String, EntrySource)> = Vec::new();
    let mut skipped = Vec::new();
    for wav in &wavs {
        if cancelled() {
            return Err(AppError::cancelled());
        }
        let name = wav
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(target) = target_for(wav, dir, &by_stem) else {
            skipped.push(name);
            continue;
        };
        if entries.iter().any(|(p, _)| *p == target) {
            skipped.push(name);
            continue;
        }
        let encoded = std::fs::read(wav)
            .map_err(|e| AppError::from_io("Reading a sound", wav, &e))
            .and_then(|bytes| {
                let pcm = kombini_source2::sound_encode::read_wav(&bytes)
                    .map_err(|e| AppError::invalid(format!("{name}: {e}")))?;
                let template = game
                    .read_file(&target)
                    .map_err(|e| AppError::invalid(format!("{target}: {e}")))?;
                kombini_source2::sound_encode::encode_pcm16(&template, &pcm)
                    .map_err(|e| AppError::invalid(format!("{name}: {e}")))
            });
        match encoded {
            Ok(bytes) => entries.push((target, EntrySource::Bytes(bytes))),
            Err(e) => {
                tracing::warn!("sound pack: {name}: {e}");
                skipped.push(name);
            }
        }
    }
    if entries.is_empty() {
        return Err(AppError::new(
            ErrorCode::NoVpkInArchive,
            "None of the WAV files in this download are named after a Deadlock sound, \
             so there is nothing to replace.",
        ));
    }
    let converted = entries.len();
    kombini_vpk::write_vpk(out, entries, cancelled).map_err(|e| {
        AppError::new(
            ErrorCode::Io,
            format!("Writing the converted sounds failed: {e}"),
        )
    })?;
    Ok(SoundPack { converted, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_target_by_path_then_voice_line() {
        let mut by_stem = BTreeMap::new();
        by_stem.insert(
            "clip".to_owned(),
            vec![
                "sounds/ui/clip.vsnd_c".to_owned(),
                "sounds/vo/nano/clip.vsnd_c".to_owned(),
            ],
        );
        let dir = Path::new("/x");
        assert_eq!(
            target_for(Path::new("/x/clip.wav"), dir, &by_stem).as_deref(),
            Some("sounds/vo/nano/clip.vsnd_c")
        );
        assert_eq!(
            target_for(Path::new("/x/sounds/ui/Clip.WAV"), dir, &by_stem).as_deref(),
            Some("sounds/ui/clip.vsnd_c")
        );
        assert_eq!(target_for(Path::new("/x/other.wav"), dir, &by_stem), None);
    }

    /// Encodes one WAV as a live Calico voice line and decodes it back.
    #[test]
    fn live_voice_line_round_trip() {
        let Ok(pak) = std::env::var("DEADLOCK_PAK") else {
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        // One second of a 440 Hz stereo tone.
        let rate = 44_100u32;
        let mut data = Vec::new();
        for i in 0..rate {
            let s = ((i as f32 * 440. * std::f32::consts::TAU / rate as f32).sin() * 8000.) as i16;
            data.extend_from_slice(&s.to_le_bytes());
            data.extend_from_slice(&s.to_le_bytes());
        }
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 4).to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
        wav.extend_from_slice(&data);
        std::fs::write(src.join("nano_desperation_power1_01.wav"), &wav).unwrap();
        std::fs::write(src.join("not_a_game_sound.wav"), &wav).unwrap();
        assert!(is_sound_pack(&src));

        let out = tmp.path().join("pak01_dir.vpk");
        let pack = build_sound_pack(&src, Path::new(&pak), &out, &|| false).unwrap();
        assert_eq!(pack.converted, 1);
        assert_eq!(pack.skipped, vec!["not_a_game_sound.wav".to_owned()]);

        let vpk = VpkArchive::open(&out).unwrap();
        let bytes = vpk
            .read_file("sounds/vo/nano/nano_desperation_power1_01.vsnd_c")
            .unwrap();
        let decoded = kombini_source2::sound::decode_sound(&bytes).unwrap();
        assert_eq!(
            decoded.info.format,
            kombini_source2::sound::SoundFormat::Pcm16
        );
        assert_eq!(decoded.info.rate, rate);
        assert_eq!(decoded.info.channels, 1);
        assert!((decoded.info.duration - 1.0).abs() < 1e-6);
        // WAV header + one second of mono 16-bit audio.
        assert_eq!(decoded.bytes.len(), 44 + rate as usize * 2);
    }
}
