//! Settings tab.

use crate::app::AppView;
use crate::smooth_scroll::Smooth as _;
use crate::theme::*;
use crate::widgets::*;
use gpui_kit::component::button::*;
use gpui_kit::component::input::Input;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kombini_engine::settings::Settings;

type Toggle = fn(&mut Settings) -> &mut bool;

impl AppView {
    pub fn render_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Load the active profile's launch options into the input once per profile.
        let profile = self.lib.profile().clone();
        if self.launch_input_for.as_ref() != Some(&profile.id) {
            self.launch_input_for = Some(profile.id.clone());
            let value = profile.launch_options.clone().unwrap_or_default();
            self.launch_input
                .update(cx, |input, cx| input.set_value(value, window, cx));
        }
        let s = self.settings.clone();
        let saving = self.is_pending("settings");
        let toggles: [(&str, &str, bool, Toggle); 10] = [
            (
                "Block changes while Deadlock runs",
                "Prevents half-applied changes while the game has files open.",
                s.block_while_running,
                |s| &mut s.block_while_running,
            ),
            (
                "Keep mods in place automatically",
                "Puts back missing mod files and repairs gameinfo.gi (Steam updates remove it) on startup, when Kombini regains focus and after the game closes.",
                s.auto_reapply_gameinfo,
                |s| &mut s.auto_reapply_gameinfo,
            ),
            (
                "Snapshot before risky changes",
                "Uninstalls and restores take an automatic snapshot first.",
                s.auto_snapshot,
                |s| &mut s.auto_snapshot,
            ),
            (
                "Snapshot before launching modded",
                "Lets you roll back to exactly what you played with.",
                s.snapshot_before_launch,
                |s| &mut s.snapshot_before_launch,
            ),
            (
                "Keep downloaded archives",
                "Allows reinstalling without downloading again.",
                s.keep_archives,
                |s| &mut s.keep_archives,
            ),
            (
                "Check for mod updates automatically",
                "Asks GameBanana for newer files of your installed mods, at most every 6 hours.",
                s.check_updates_on_startup,
                |s| &mut s.check_updates_on_startup,
            ),
            (
                "Hide NSFW mods in Discover",
                "Hides mods GameBanana marks as mature (content ratings or hidden by default).",
                s.hide_nsfw,
                |s| &mut s.hide_nsfw,
            ),
            (
                "Ask before stacking skins",
                "When you enable a second skin for a hero, offer to swap instead of loading both.",
                s.hero_swap_prompt,
                |s| &mut s.hero_swap_prompt,
            ),
            (
                "Switch files instead of stacking them",
                "Installing another file from a mod's page turns off the ones you have. Off keeps them all on. The install window lets you choose each time.",
                s.switch_variants,
                |s| &mut s.switch_variants,
            ),
            (
                "Shuffle skins on Launch Modded",
                "Every hero with two or more skins gets a random one each time you launch modded.",
                s.shuffle_on_launch,
                |s| &mut s.shuffle_on_launch,
            ),
        ];
        let game = self.engine.as_ref().map(|e| e.paths().root.clone());
        div()
            .id("settings")
            .size_full()
            .overflow_y_scroll()
            .smooth(&self.smooth.get("settings"), cx)
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(
                panel()
                    .p(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(section_title("Game"))
                    .child(label(
                        game.as_ref()
                            .map(|g| g.display().to_string())
                            .unwrap_or_else(|| "Not set".into()),
                    ))
                    .children(self.render_game_version())
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(
                                Button::new("set-game")
                                    .small()
                                    .text_label("Change folder…")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.pick_game_folder(cx)),
                                    ),
                            )
                            .when_some(game, |d, g| {
                                let addons = g.join("game").join("citadel").join("addons");
                                d.child(
                                    Button::new("open-addons")
                                        .small()
                                        .ghost()
                                        .text_label("Open addons folder")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.open_folder(&addons, cx)
                                        })),
                                )
                            })
                            .child(
                                Button::new("open-data")
                                    .small()
                                    .ghost()
                                    .text_label("Open app data")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let d = this.dirs.data.clone();
                                        this.open_folder(&d, cx)
                                    })),
                            ),
                    ),
            )
            .child(
                panel()
                    .p(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(div().flex_1().child(section_title("Behaviour")))
                            .when(saving, |d| {
                                d.child(gpui_kit::component::spinner::Spinner::new().xsmall())
                            }),
                    )
                    .children(toggles.into_iter().enumerate().map(
                        |(i, (title, desc, value, field))| {
                            div()
                                .flex()
                                .items_center()
                                .gap(px(12.))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .flex_1()
                                        .child(label(title).text_sm())
                                        .child(faint(desc)),
                                )
                                .child(
                                    Switch::new(("set-sw", i))
                                        .checked(value)
                                        .disabled(saving)
                                        .on_click(cx.listener(
                                            move |this, checked: &bool, _, cx| {
                                                let mut next = this.settings.clone();
                                                *field(&mut next) = *checked;
                                                this.save_settings(next, cx);
                                            },
                                        )),
                                )
                        },
                    )),
            )
            .child(
                panel()
                    .p(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(section_title(format!("Launch · profile {}", profile.name)))
                    .child(faint(
                        "Used by Launch Modded for this profile.",
                    ))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .items_center()
                            .child(div().flex_1().child(Input::new(&self.launch_input)))
                            .child(self.launch_save_state()),
                    )
                    .child(
                        div().flex().gap(px(8.)).child(
                            Button::new("edit-autoexec")
                                .small()
                                .text_label("Edit autoexec.cfg")
                                .tooltip("Opens game\\citadel\\cfg\\autoexec.cfg in your text editor")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.run(
                                        "autoexec",
                                        "Opening autoexec.cfg",
                                        cx,
                                        |e, _| {
                                            if e.read_autoexec().is_none() {
                                                e.write_autoexec("// Kombini: console commands run at game start\n")?;
                                            }
                                            Ok(e.paths().autoexec())
                                        },
                                        |_, path, cx| cx.open_with_system(&path),
                                    );
                                })),
                        ),
                    )
                    .child(divider())
                    .child(section_title("Links"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .child(label("Open kombini:// and deadlockmm:// links").text_sm())
                                    .child(faint(
                                        "Lets one-click install buttons on mod sites open in Kombini. Registered for your Windows user only.",
                                    )),
                            )
                            .child(
                                Switch::new("set-links")
                                    .checked(s.url_handlers_registered)
                                    .disabled(saving)
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        let on = *checked;
                                        match crate::instance::set_url_handlers(on) {
                                            Ok(()) => {
                                                let mut next = this.settings.clone();
                                                next.url_handlers_registered = on;
                                                this.save_settings(next, cx);
                                            }
                                            Err(e) => {
                                                this.error(
                                                    "Registering link handlers",
                                                    &kombini_core::AppError::new(
                                                        kombini_core::ErrorCode::PermissionDenied,
                                                        e.to_string(),
                                                    ),
                                                );
                                                cx.notify();
                                            }
                                        }
                                    })),
                            ),
                    )
                    .child(divider())
                    .child(section_title("Maintenance"))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(8.))
                            .child(
                                Button::new("m-updates")
                                    .small()
                                    .text_label("Check for updates")
                                    .loading(self.is_pending("updates"))
                                    .disabled(self.is_pending("updates"))
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.check_updates(true, cx)),
                                    ),
                            )
                            .child(
                                Button::new("m-adopt")
                                    .small()
                                    .text_label("Import existing addons")
                                    .on_click(cx.listener(|this, _, _, cx| this.open_adopt(cx))),
                            )
                            .child(Button::new("m-export").small().text_label("Export mod bundle…").loading(self.is_pending("export")).disabled(self.is_pending("export")).on_click(cx.listener(|this, _, _, cx| this.export_bundle(cx))))
                            .child(Button::new("m-import").small().text_label("Import mod bundle…").on_click(cx.listener(|this, _, _, cx| this.import_bundle_dialog(cx))))
                            .child(
                                Button::new("m-health")
                                    .small()
                                    .text_label("Health check")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.modal = Some(crate::modal::Modal::Health);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("m-gc")
                                    .small()
                                    .ghost()
                                    .text_label("Free unused storage")
                                    .loading(self.is_pending("gc"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.run(
                                            "gc",
                                            "Cleaning storage",
                                            cx,
                                            |e, _| e.gc_blobs(),
                                            |this, r, _| {
                                                this.success(format!(
                                                    "Freed {}.",
                                                    format_bytes(r.freed_bytes)
                                                ))
                                            },
                                        );
                                    })),
                            ),
                    ),
            )
            .child(self.render_credits())
            .child(faint(format!(
                "Kombini Mod Manager {} · MIT",
                env!("CARGO_PKG_VERSION")
            )))
    }
}

impl AppView {
    /// Credits: projects Kombini builds on, and where features came from.
    fn render_credits(&self) -> impl IntoElement {
        let link = |id: &'static str, label: &'static str, url: &'static str| {
            Button::new(id)
                .small()
                .ghost()
                .icon(gpui_kit::component::IconName::ExternalLink)
                .text_label(label)
                .on_click(move |_, _, cx| cx.open_url(url))
        };
        let entry = |title: &'static str, body: &'static str| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(title))
                .child(div().text_xs().text_color(c(MUTED)).child(body))
        };
        panel()
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(section_title("Credits"))
            .child(
                card()
                    .p(px(14.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .border_color(ca(ACCENT, 0.35))
                    .child(entry(
                        "Mod issues and repairs: based on Grimoire",
                        "The Mod issues feature (fixing camera points, outdated hero and ability data, missing text, skeleton layouts and sound bindings after a game update) follows the approach of the Grimoire mod manager by esoc. Its repair engine, vpkmerge, and its Source 2 library, morphic, were ported into Kombini. Grimoire showed how to do it safely: compare a mod's copy of a game file with the installed game's own copy, only add what is missing, never overwrite what the mod author changed, and verify the rebuilt archive before it replaces anything. Used under the MIT License.",
                    ))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(link("credit-grimoire", "Grimoire", "https://github.com/Slush97/grimoire"))
                            .child(link("credit-vpkmerge", "vpkmerge", "https://github.com/Slush97/vpkmerge")),
                    ),
            )
            .child(entry(
                "\"May be outdated\" estimate",
                "Comparing a mod's upload date with the last major game update follows Deadlock Mod Manager's outdated-mod cutoff. Update dates come from Valve's Steam news for Deadlock.",
            ))
            .child(entry(
                "Crosshair designer and autoexec commands: based on Grimoire",
                "The crosshair convars, the game's default values, the preview's layout math and the curated command list come from Grimoire's crosshair editor and command presets (MIT, esoc).",
            ))
            .child(entry(
                "Importing from Deadlock Mod Manager and Grimoire: based on Grimoire",
                "Reading Deadlock Mod Manager's state.json and .dmm.json (every version, profile folders and shards, its download cache as a fallback) follows Grimoire's own DMM migration (MIT, esoc). Grimoire's libraries are read from its mod-metadata.json and profiles.json.",
            ))
            .child(entry(
                "Source 2 formats",
                "Resource and KeyValues3 layouts follow ValveResourceFormat (MIT), by way of morphic.",
            ))
            .child(entry(
                "Interface",
                "GPUI and gpui-component (Apache-2.0); icons from Lucide (ISC).",
            ))
            .child(entry(
                "Mods",
                "Browsing, downloads and update checks use GameBanana. Mods belong to their authors.",
            ))
            .child(faint("Full license texts: THIRD-PARTY-NOTICES.md in the install folder."))
    }

    /// Saves the launch options input when it differs from the profile
    /// (called on blur and Enter; no Save button).
    pub fn save_launch_options(&mut self, cx: &mut Context<Self>) {
        let text = self.launch_input.read(cx).value().trim().to_string();
        let current = self
            .lib
            .profile()
            .launch_options
            .clone()
            .unwrap_or_default();
        if text == current.trim() || self.engine.is_none() {
            return;
        }
        let value = (!text.is_empty()).then_some(text);
        self.run(
            "launch-options",
            "Saving launch options",
            cx,
            move |e, _| e.set_profile_launch_options(value),
            |this, (), cx| {
                this.launch_saved_at = Some(std::time::Instant::now());
                cx.notify();
            },
        );
    }

    /// Inline status next to the launch options input.
    fn launch_save_state(&self) -> AnyElement {
        let row = || {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(6.))
                .text_xs()
        };
        if self.is_pending("launch-options") {
            return row()
                .text_color(c(MUTED))
                .child(gpui_kit::component::spinner::Spinner::new().xsmall())
                .child("Saving…")
                .into_any_element();
        }
        let recent = self
            .launch_saved_at
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(3));
        if recent {
            let key = self
                .launch_saved_at
                .map(|t| format!("{t:?}"))
                .unwrap_or_default();
            return crate::anim::fade_in(
                row()
                    .text_color(c(ACCENT))
                    .child(
                        gpui_kit::component::Icon::new(gpui_kit::component::IconName::Check)
                            .size(px(14.)),
                    )
                    .child("Saved"),
                SharedString::from(format!("launch-saved-{key}")),
                crate::anim::MEDIUM,
            )
            .into_any_element();
        }
        row()
            .text_color(c(FAINT))
            .child("Saves when you press Enter or leave the field")
            .into_any_element()
    }

    /// Exports every mod plus profiles as a Deadlock Mod Interchange folder.
    pub fn export_bundle(&mut self, cx: &mut Context<Self>) {
        let start = dirs_home().unwrap_or_else(|| self.dirs.data.clone());
        let rx = cx.prompt_for_new_path(&start, Some("Kombini mods"));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(dest))) = rx.await {
                this.update(cx, |this, cx| {
                    let shown = dest.display().to_string();
                    this.run_with(
                        "export",
                        "Exporting mod bundle",
                        true,
                        cx,
                        move |e, p| kombini_import::interchange::export(e, &dest, true, p),
                        move |this, _, cx| {
                            this.toast(Notification::success(format!("Exported to {shown}.")));
                            cx.notify();
                        },
                    );
                })
                .ok();
            }
        })
        .detach();
    }

    pub fn import_bundle_dialog(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose a mod bundle folder".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update(cx, |this, cx| {
                    this.handle_external(&path.display().to_string(), cx)
                })
                .ok();
            }
        })
        .detach();
    }
}

pub fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("USERPROFILE").map(|h| std::path::PathBuf::from(h).join("Documents"))
}
