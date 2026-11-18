//! Mod issues: checks and repairs for mods made against an older Deadlock
//! build (camera points, outdated game data, text, skeleton, sound bindings).
//!
//! The approach and the repair code come from Grimoire's mod repairs (its
//! vpkmerge engine, MIT); see `kombini-repair` and the Credits panel in
//! Settings.

use crate::app::AppView;
use crate::icons::Lucide;
use crate::modal::Modal;
use crate::theme::*;
use crate::widgets::*;
use gpui_kit::component::button::*;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{Disableable, Icon, IconName, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kombini_core::ModId;
use kombini_engine::repair::{ModIssue, ModIssues};
use kombini_engine::{NoProgress, Progress};
use std::collections::HashMap;

/// Latest scan result. `by_mod` only holds mods with findings or errors.
#[derive(Default)]
pub struct IssuesState {
    pub scanned: bool,
    pub scanning: bool,
    pub checked: usize,
    pub by_mod: HashMap<ModId, ModIssues>,
}

impl IssuesState {
    pub fn repairable_mods(&self) -> Vec<ModId> {
        let mut v: Vec<&ModIssues> = self
            .by_mod
            .values()
            .filter(|m| m.repairable() > 0)
            .collect();
        v.sort_by_key(|m| m.title.to_lowercase());
        v.into_iter().map(|m| m.mod_id.clone()).collect()
    }

    fn store(&mut self, list: Vec<ModIssues>) {
        self.scanned = true;
        self.checked = list.len();
        self.by_mod = list
            .into_iter()
            .filter(|m| !m.issues.is_empty() || m.error.is_some())
            .map(|m| (m.mod_id.clone(), m))
            .collect();
    }
}

impl AppView {
    /// Silent read-only scan (startup, after installs). Results feed the row
    /// chips, the details pane and the Mod issues window.
    pub fn scan_issues_quietly(&mut self, cx: &mut Context<Self>) {
        if self.issues.scanning || self.engine.is_none() {
            return;
        }
        self.issues.scanning = true;
        self.background(
            cx,
            |e| e.scan_mod_issues(&NoProgress),
            |this, list, cx| {
                this.issues.scanning = false;
                this.issues.store(list);
                this.recompute_rows(cx);
                cx.notify();
            },
        );
    }

    /// Scan with progress in the status bar (the Rescan button).
    pub fn scan_issues(&mut self, cx: &mut Context<Self>) {
        self.run(
            "issues-scan",
            "Checking mods against the game",
            cx,
            |e, p| e.scan_mod_issues(p),
            |this, list, cx| {
                this.issues.store(list);
                this.recompute_rows(cx);
                let n = this.issues.repairable_mods().len();
                this.success(if n == 0 {
                    "No fixable mod issues found.".to_owned()
                } else {
                    format!("{n} mod(s) can be fixed.")
                });
                cx.notify();
            },
        );
    }

    pub fn open_issues(&mut self, cx: &mut Context<Self>) {
        self.modal = Some(Modal::Issues);
        if !self.issues.scanned {
            self.scan_issues(cx);
        }
        cx.notify();
    }

    pub fn repair_mod(&mut self, id: ModId, cx: &mut Context<Self>) {
        let title = self
            .lib
            .mods
            .get(&id)
            .map(|m| m.title.clone())
            .unwrap_or_default();
        self.run_with(
            format!("repair:{id}"),
            format!("Fixing {title}"),
            true,
            cx,
            move |e, p| e.repair_mod(&id, p),
            |this, out, cx| {
                if out.files == 0 {
                    this.toast(Notification::info(format!(
                        "{} had nothing left to fix automatically.",
                        out.title
                    )));
                } else {
                    this.success(format!(
                        "Fixed {}: {} game file(s) updated. A snapshot was taken first.",
                        out.title, out.entries
                    ));
                }
                if !out.remaining.is_empty() {
                    this.toast(
                        Notification::warning(format!(
                            "{} issue(s) in {} can't be fixed automatically. Check for an update of the mod.",
                            out.remaining.len(),
                            out.title
                        ))
                        .title("Partly fixed"),
                    );
                }
                this.apply_remaining(out.mod_id.clone(), out.title.clone(), out.remaining);
                this.recompute_rows(cx);
                cx.notify();
            },
        );
    }

    pub fn repair_all(&mut self, cx: &mut Context<Self>) {
        let ids = self.issues.repairable_mods();
        if ids.is_empty() {
            return;
        }
        let n = ids.len();
        self.run_with(
            "repair:all",
            format!("Fixing {n} mod(s)"),
            true,
            cx,
            move |e, p| {
                let mut done = Vec::new();
                for id in &ids {
                    if p.cancelled() {
                        break;
                    }
                    done.push(e.repair_mod(id, p)?);
                }
                Ok(done)
            },
            |this, outcomes, cx| {
                let fixed = outcomes.iter().filter(|o| o.files > 0).count();
                let left: usize = outcomes.iter().map(|o| o.remaining.len()).sum();
                this.success(if left == 0 {
                    format!("Fixed {fixed} mod(s). Snapshots were taken first.")
                } else {
                    format!("Fixed {fixed} mod(s); {left} issue(s) can't be fixed automatically.")
                });
                for o in outcomes {
                    this.apply_remaining(o.mod_id, o.title, o.remaining);
                }
                this.recompute_rows(cx);
                cx.notify();
            },
        );
    }

    fn apply_remaining(&mut self, id: ModId, title: String, remaining: Vec<ModIssue>) {
        if remaining.is_empty() {
            self.issues.by_mod.remove(&id);
        } else {
            let enabled = self.lib.profile().is_enabled(&id);
            self.issues.by_mod.insert(
                id.clone(),
                ModIssues {
                    mod_id: id,
                    title,
                    enabled,
                    issues: remaining,
                    error: None,
                },
            );
        }
    }

    /// Row chip: confirmed findings first, then the date estimate.
    pub fn issue_chip(&self, id: &ModId) -> Option<Div> {
        if let Some(m) = self.issues.by_mod.get(id) {
            if m.repairable() > 0 {
                return Some(chip("Fix available", AMBER));
            }
            if !m.issues.is_empty() {
                return Some(chip("Needs update", CORAL));
            }
        }
        self.mod_freshness(id)
            .maybe_outdated()
            .then(|| chip("May be outdated", MUTED))
    }

    /// Compatibility section in the details pane.
    pub fn render_issue_section(
        &mut self,
        id: &ModId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let facts = self.render_age_facts(id, cx);
        let found = self
            .issues
            .by_mod
            .get(id)
            .filter(|m| !m.issues.is_empty() || m.error.is_some())
            .cloned();
        if facts.is_none() && found.is_none() {
            return None;
        }
        let card = found.map(|m| self.render_issue_card(&m, 0, cx));
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(section_title("Compatibility"))
                .children(facts)
                .children(card)
                .into_any_element(),
        )
    }

    fn render_issue_card(&mut self, m: &ModIssues, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let pending =
            self.is_pending(&format!("repair:{}", m.mod_id)) || self.is_pending("repair:all");
        let repairable = m.repairable();
        let busy = self.launch.phase.busy();
        let fix_id = m.mod_id.clone();
        card()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(12.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        Icon::new(Lucide::ShieldAlert)
                            .size(px(16.))
                            .text_color(c(if repairable > 0 { AMBER } else { FAINT })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(m.title.clone()),
                    )
                    .when(!m.enabled, |d| d.child(chip("Disabled", FAINT)))
                    .when(repairable > 0, |d| {
                        d.child(
                            Button::new(("issue-fix", i))
                                .small()
                                .warning()
                                .icon(Lucide::Wrench)
                                .label("Fix")
                                .tooltip(
                                    "Repair a copy of this mod's files; a snapshot is taken first",
                                )
                                .loading(pending)
                                .disabled(pending || busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.repair_mod(fix_id.clone(), cx)
                                })),
                        )
                    }),
            )
            .when_some(m.error.clone(), |d, err| {
                d.child(faint(format!("Could not check this mod: {err}")))
            })
            .children(m.issues.iter().map(|issue| {
                div()
                    .flex()
                    .gap(px(8.))
                    .child(chip(
                        issue.kind.label(),
                        if issue.repairable { AMBER } else { MUTED },
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(c(TEXT))
                                    .child(issue.detail.clone()),
                            )
                            .child(faint(issue.entry.clone())),
                    )
            }))
            .into_any_element()
    }

    /// The Mod issues window.
    pub fn render_issues(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let scanning = self.is_pending("issues-scan");
        let fixing_all = self.is_pending("repair:all");
        let fixable = self.issues.repairable_mods().len();
        let mut mods: Vec<ModIssues> = self.issues.by_mod.values().cloned().collect();
        mods.sort_by(|a, b| {
            (b.repairable() > 0)
                .cmp(&(a.repairable() > 0))
                .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        let head = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(div().flex_1().child(muted(
                "Game updates can break older mods: cameras, hero data, text and sounds. Kombini compares each mod with your installed game and fixes what it safely can. Nothing the mod author changed is overwritten.",
            )))
            .child(
                Button::new("issues-rescan")
                    .small()
                    .ghost()
                    .icon(IconName::RefreshCw)
                    .label("Rescan")
                    .loading(scanning)
                    .disabled(scanning || fixing_all)
                    .on_click(cx.listener(|this, _, _, cx| this.scan_issues(cx))),
            )
            .child(
                Button::new("issues-fix-all")
                    .small()
                    .warning()
                    .icon(Lucide::Wrench)
                    .label(format!("Fix all ({fixable})"))
                    .loading(fixing_all)
                    .disabled(fixable == 0 || scanning || fixing_all || self.launch.phase.busy())
                    .on_click(cx.listener(|this, _, _, cx| this.repair_all(cx))),
            );
        let body: AnyElement = if !self.issues.scanned {
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .py(px(24.))
                .justify_center()
                .child(gpui_kit::component::spinner::Spinner::new())
                .child(muted("Comparing your mods with the game…"))
                .into_any_element()
        } else if mods.is_empty() {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.))
                .py(px(24.))
                .child(
                    Icon::new(IconName::CircleCheck)
                        .size(px(28.))
                        .text_color(c(ACCENT)),
                )
                .child(muted(format!(
                    "No issues found in {} mod(s).",
                    self.issues.checked
                )))
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .children(
                    mods.iter()
                        .enumerate()
                        .map(|(i, m)| self.render_issue_card(m, i, cx))
                        .collect::<Vec<_>>(),
                )
                .into_any_element()
        };
        let version = [self.game_version_text(), self.last_major_text()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(head)
            .when(!version.is_empty(), |d| d.child(faint(version)))
            .child(body)
            .child(
                div()
                    .id("issues-credit")
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_xs()
                    .text_color(c(FAINT))
                    .child("Based on the mod repairs in Grimoire (vpkmerge by esoc).")
                    .child(
                        div()
                            .id("issues-credit-link")
                            .text_color(c(INFO))
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .child("Credits")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.modal = None;
                                this.set_tab(crate::app::Tab::Settings, cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}
