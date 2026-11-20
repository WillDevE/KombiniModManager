//! Sound previews: play a mod's `.vsnd_c` files, and the game's original of
//! the same sound for comparison. Used by the details pane and the file
//! inspector.

use crate::app::AppView;
use crate::theme::*;
use crate::widgets::*;
use gpui_kit::component::button::*;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{IconName, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kombini_core::{AppError, BlobHash, ErrorCode, ModId};
use kombini_source2::sound::{DecodedSound, decode_sound};
use std::time::Duration;

/// Sounds listed in the details pane before "and N more".
const DETAILS_SOUNDS: usize = 8;

/// Where a previewed sound is read from.
#[derive(Clone)]
pub enum SoundOrigin {
    Mod(BlobHash),
    Game,
}

#[derive(Clone)]
pub struct SoundEntry {
    pub blob: BlobHash,
    pub path: String,
    /// The base game has a sound at the same path (the mod replaces it).
    pub in_game: bool,
}

pub enum ModSounds {
    Loading,
    Ready(Vec<SoundEntry>),
    Failed,
}

pub fn is_sound(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".vsnd_c")
}

fn vpk_error(e: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::InvalidVpk, e.to_string())
}

fn sound_name(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.strip_suffix(".vsnd_c").unwrap_or(file).to_owned()
}

impl AppView {
    /// Starts (or, for the sound already playing, stops) a preview.
    pub fn play_sound(
        &mut self,
        key: String,
        origin: SoundOrigin,
        path: String,
        cx: &mut Context<Self>,
    ) {
        if self.playing.as_ref().is_some_and(|(k, _)| *k == key) {
            self.audio.stop();
            self.playing = None;
            cx.notify();
            return;
        }
        self.background(
            cx,
            move |e| {
                let archive = match origin {
                    SoundOrigin::Mod(blob) => e.blobs().path_for(&blob),
                    SoundOrigin::Game => e.paths().citadel.join("pak01_dir.vpk"),
                };
                let result: Result<DecodedSound, String> = kombini_vpk::VpkArchive::open(&archive)
                    .and_then(|a| a.read_file(&path))
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| decode_sound(&bytes).map_err(|e| e.to_string()));
                Ok(result)
            },
            move |this, result, cx| {
                match result {
                    Ok(sound) => {
                        this.audio.play(sound.bytes);
                        this.play_token += 1;
                        let token = this.play_token;
                        this.playing = Some((key, token));
                        let length =
                            Duration::from_secs_f64(sound.info.duration.clamp(0.1, 600.) + 0.3);
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(length).await;
                            this.update(cx, |this, cx| {
                                if this.playing.as_ref().is_some_and(|(_, t)| *t == token) {
                                    this.playing = None;
                                    cx.notify();
                                }
                            })
                            .ok();
                        })
                        .detach();
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "sound preview failed");
                        this.toast(Notification::info("This sound could not be played."));
                    }
                }
                cx.notify();
            },
        );
    }

    pub fn is_playing(&self, key: &str) -> bool {
        self.playing.as_ref().is_some_and(|(k, _)| k == key)
    }

    /// A small play/stop button.
    pub fn play_button(
        &self,
        id: impl Into<ElementId>,
        key: String,
        origin: SoundOrigin,
        path: String,
        label: Option<&'static str>,
        cx: &mut Context<Self>,
    ) -> Button {
        let playing = self.is_playing(&key);
        Button::new(id)
            .small()
            .ghost()
            .icon(if playing {
                IconName::Square
            } else {
                IconName::Play
            })
            .when_some(label, |b, l| b.text_label(l))
            .tooltip(if playing { "Stop" } else { "Play" })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.play_sound(key.clone(), origin.clone(), path.clone(), cx)
            }))
    }

    /// The archives previewed for a mod: its selected variants, else all.
    fn sound_blobs(&self, id: &ModId) -> Vec<BlobHash> {
        let Some(m) = self.lib.mods.get(id) else {
            return Vec::new();
        };
        let selected: Vec<BlobHash> = m
            .roots()
            .filter(|f| f.selected)
            .map(|f| f.blob.clone())
            .collect();
        if selected.is_empty() {
            m.roots().map(|f| f.blob.clone()).collect()
        } else {
            selected
        }
    }

    fn load_mod_sounds(&mut self, id: ModId, blobs: Vec<BlobHash>, cx: &mut Context<Self>) {
        self.mod_sounds
            .insert(id.clone(), (blobs.clone(), ModSounds::Loading));
        let key = id.clone();
        cx.spawn(async move |this, cx| {
            let engine = this
                .update(cx, |this, _| this.engine.clone())
                .ok()
                .flatten();
            let Some(engine) = engine else { return };
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut list = Vec::new();
                    for blob in blobs {
                        let a = kombini_vpk::VpkArchive::open(&engine.blobs().path_for(&blob))
                            .map_err(vpk_error)?;
                        list.extend(a.entries().iter().filter(|e| is_sound(&e.path)).map(|e| {
                            SoundEntry {
                                blob: blob.clone(),
                                path: e.path.clone(),
                                in_game: false,
                            }
                        }));
                    }
                    list.sort_by(|a, b| a.path.cmp(&b.path));
                    list.dedup_by(|a, b| a.path == b.path);
                    if !list.is_empty()
                        && let Ok(game) = kombini_vpk::VpkArchive::open(
                            &engine.paths().citadel.join("pak01_dir.vpk"),
                        )
                    {
                        for s in &mut list {
                            s.in_game = game.contains(&s.path);
                        }
                    }
                    Ok::<_, AppError>(list)
                })
                .await;
            this.update(cx, |this, cx| {
                let state = match result {
                    Ok(list) => ModSounds::Ready(list),
                    Err(e) => {
                        tracing::warn!(error = %e, "listing mod sounds failed");
                        ModSounds::Failed
                    }
                };
                if let Some(slot) = this.mod_sounds.get_mut(&key) {
                    slot.1 = state;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Details pane "Sounds", or `None` for mods without sounds.
    pub fn render_sounds_section(
        &mut self,
        id: &ModId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // Reload when the mod's files change (update, variant switch).
        let blobs = self.sound_blobs(id);
        if self.mod_sounds.get(id).is_none_or(|(b, _)| *b != blobs) {
            self.load_mod_sounds(id.clone(), blobs, cx);
        }
        let (_, ModSounds::Ready(list)) = self.mod_sounds.get(id)? else {
            return None;
        };
        if list.is_empty() {
            return None;
        }
        let total = list.len();
        let shown: Vec<SoundEntry> = list.iter().take(DETAILS_SOUNDS).cloned().collect();
        let rows: Vec<AnyElement> = shown
            .into_iter()
            .enumerate()
            .map(|(i, s)| {
                let mod_key = format!("mod:{}:{}", s.blob, s.path);
                let game_key = format!("game:{}", s.path);
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .py(px(2.))
                    .child(self.play_button(
                        ("snd-mod", i),
                        mod_key,
                        SoundOrigin::Mod(s.blob.clone()),
                        s.path.clone(),
                        None,
                        cx,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_sm().truncate().child(sound_name(&s.path)))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(c(FAINT))
                                    .truncate()
                                    .child(s.path.clone()),
                            ),
                    )
                    .when(s.in_game, |d| {
                        d.child(self.play_button(
                            ("snd-game", i),
                            game_key,
                            SoundOrigin::Game,
                            s.path.clone(),
                            Some("Original"),
                            cx,
                        ))
                    })
                    .into_any_element()
            })
            .collect();
        let inspect_id = id.clone();
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(section_title("Sounds"))
                .child(
                    card()
                        .px(px(12.))
                        .py(px(6.))
                        .flex()
                        .flex_col()
                        .children(rows)
                        .when(total > DETAILS_SOUNDS, |d| {
                            d.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .pt(px(4.))
                                    .child(div().flex_1().child(faint(format!(
                                        "and {} more",
                                        total - DETAILS_SOUNDS
                                    ))))
                                    .child(
                                        Button::new("snd-all")
                                            .small()
                                            .ghost()
                                            .text_label("All files")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.open_inspector(inspect_id.clone(), cx)
                                            })),
                                    ),
                            )
                        }),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use kombini_source2::sound::decode_sound;

    /// Reads real game sounds; runs only with DEADLOCK_PAK set.
    #[test]
    fn game_sounds_open_in_the_player() {
        let Some(pak) = std::env::var_os("DEADLOCK_PAK") else {
            return;
        };
        let archive = kombini_vpk::VpkArchive::open(std::path::Path::new(&pak)).unwrap();
        for path in [
            "sounds/abilities/abrams/abrams_a1_siphonlife_cast.vsnd_c",
            "models/heroes/kelvin/maya/animation/jamaalb_working/ref/kelvin_tutorial_lane_info.vsnd_c",
        ] {
            let sound = decode_sound(&archive.read_file(path).unwrap()).unwrap();
            let decoder = rodio::Decoder::new(std::io::Cursor::new(sound.bytes));
            assert!(decoder.is_ok(), "{path}: {:?}", decoder.err());
        }
    }
}
