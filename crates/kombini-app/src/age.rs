//! Game version and the "made for an older game" estimate in the UI
//! (row chips, the details pane, Discover, Settings).

use crate::app::AppView;
use crate::theme::*;
use crate::widgets::*;
use gpui_kit::component::button::*;
use gpui_kit::component::{Icon, IconName, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kombini_core::ModId;
use kombini_core::freshness::{Freshness, format_date};
use std::collections::BTreeSet;

impl AppView {
    /// Offline timeline first (instant), then one with fresh Steam news.
    pub fn load_timeline(&mut self, cx: &mut Context<Self>) {
        self.background(
            cx,
            |e| Ok(e.game_timeline(false)),
            |this, t, cx| {
                this.timeline = Some(t);
                this.recompute_rows(cx);
                this.background(
                    cx,
                    |e| Ok(e.game_timeline(true)),
                    |this, t, cx| {
                        this.timeline = Some(t);
                        this.recompute_rows(cx);
                        cx.notify();
                    },
                );
                cx.notify();
            },
        );
    }

    pub fn mod_freshness(&self, id: &ModId) -> Freshness {
        match (&self.timeline, self.lib.mods.get(id)) {
            (Some(t), Some(m)) => t.assess_mod(m),
            _ => Freshness::Unknown,
        }
    }

    /// Discover: estimate for a GameBanana date.
    pub fn date_maybe_outdated(&self, date: Option<i64>) -> bool {
        self.timeline
            .as_ref()
            .is_some_and(|t| t.assess(date).maybe_outdated())
    }

    /// Mods for the "Outdated" filter: confirmed issues or an older upload.
    pub fn outdated_ids(&self) -> BTreeSet<ModId> {
        self.lib
            .mods
            .keys()
            .filter(|id| {
                self.issues
                    .by_mod
                    .get(*id)
                    .is_some_and(|m| !m.issues.is_empty())
                    || self.mod_freshness(id).maybe_outdated()
            })
            .cloned()
            .collect()
    }

    /// "Version 6861 · built Nov 9, 2026 · Steam build 26546669".
    pub fn game_version_text(&self) -> Option<String> {
        let t = self.timeline.as_ref()?;
        let parts: Vec<String> = [
            t.build
                .client_version
                .as_ref()
                .map(|v| format!("Version {v}")),
            t.build_time.map(|bt| format!("built {}", format_date(bt))),
            t.build
                .build_id
                .as_ref()
                .map(|b| format!("Steam build {b}")),
        ]
        .into_iter()
        .flatten()
        .collect();
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    /// "Last major update: X (Oct 30, 2026)".
    pub fn last_major_text(&self) -> Option<String> {
        let u = self.timeline.as_ref()?.latest_major()?;
        Some(format!(
            "Last major update: {} ({})",
            u.title,
            format_date(u.date)
        ))
    }

    /// Settings → Game: version, last major update and where dates come from.
    pub fn render_game_version(&self) -> Option<Div> {
        let version = self.game_version_text()?;
        let t = self.timeline.as_ref()?;
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_sm().child(version))
                .children(self.last_major_text().map(muted))
                .child(faint(match t.fetched_at {
                    Some(at) => format!("Update dates from Steam news, checked {}.", ago(at)),
                    None => "Update dates from the built-in list (Steam news not reached yet)."
                        .to_owned(),
                })),
        )
    }

    /// Upload date, game version and the estimate for one installed mod.
    pub fn render_age_facts(&self, id: &ModId, cx: &mut Context<Self>) -> Option<Div> {
        let t = self.timeline.as_ref()?;
        let m = self.lib.mods.get(id)?;
        let upload = kombini_engine::timeline::mod_upload_date(m);
        let freshness = t.assess_mod(m);
        let row = |k: &'static str, v: String| {
            div()
                .flex()
                .gap(px(8.))
                .text_xs()
                .child(div().w(px(76.)).flex_none().text_color(c(FAINT)).child(k))
                .child(div().flex_1().min_w_0().text_color(c(TEXT)).child(v))
        };
        let (icon, color, verdict, link) = match &freshness {
            Freshness::Unknown => (
                IconName::Info,
                FAINT,
                "Local mod: no upload date, so Kombini can't estimate its age. The file checks below still apply.".to_owned(),
                None,
            ),
            Freshness::Current => (
                IconName::CircleCheck,
                ACCENT,
                "Uploaded after the latest game update.".to_owned(),
                None,
            ),
            Freshness::MinorBehind { count, latest } => (
                IconName::Info,
                MUTED,
                format!(
                    "{count} minor update{} since (latest {}, {}). Minor updates rarely break mods.",
                    if *count == 1 { "" } else { "s" },
                    latest.title,
                    format_date(latest.date)
                ),
                latest.url.clone(),
            ),
            Freshness::BeforeMajor {
                first,
                majors,
                total,
            } => (
                IconName::TriangleAlert,
                AMBER,
                format!(
                    "Uploaded before {} ({}), a major game update{}. It may not work until the author updates it.",
                    first.title,
                    format_date(first.date),
                    if *majors > 1 {
                        format!("; {majors} major and {} minor updates since", total - majors)
                    } else {
                        String::new()
                    }
                ),
                first.url.clone(),
            ),
        };
        Some(
            card()
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(row(
                    "Mod file",
                    upload
                        .map(|d| format!("Uploaded {} ({})", format_date(d), ago(d)))
                        .unwrap_or_else(|| format!("Added {}", format_date(m.installed_at))),
                ))
                .children(self.game_version_text().map(|v| row("Game", v)))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(8.))
                        .pt(px(2.))
                        .child(Icon::new(icon).size(px(14.)).text_color(c(color)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .text_color(c(color))
                                .child(verdict),
                        )
                        .when_some(link, |d, url| {
                            d.child(
                                Button::new("age-patch-notes")
                                    .small()
                                    .ghost()
                                    .icon(IconName::ExternalLink)
                                    .text_label("Patch notes")
                                    .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url))),
                            )
                        }),
                ),
        )
    }

    /// Discover detail: one line about the mod's last update.
    pub fn discover_age_note(&self, date: Option<i64>) -> Option<Div> {
        let t = self.timeline.as_ref()?;
        match t.assess(date) {
            Freshness::BeforeMajor { first, .. } => Some(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .p(px(10.))
                    .rounded(px(RADIUS))
                    .bg(ca(AMBER, 0.10))
                    .child(Icon::new(IconName::TriangleAlert).size(px(14.)).text_color(c(AMBER)))
                    .child(div().flex_1().min_w_0().text_xs().text_color(c(AMBER)).child(format!(
                        "Last updated before {} ({}), a major game update. It may not work; check the comments or wait for an update.",
                        first.title,
                        format_date(first.date)
                    ))),
            ),
            _ => None,
        }
    }
}

/// Small badge drawn over Discover thumbnails.
pub fn outdated_badge() -> Div {
    div()
        .absolute()
        .top(px(8.))
        .left(px(8.))
        .flex()
        .items_center()
        .h(px(20.))
        .px(px(8.))
        .rounded(px(999.))
        .bg(ca(WINDOW, 0.85))
        .text_xs()
        .text_color(c(AMBER))
        .child("May be outdated")
}
