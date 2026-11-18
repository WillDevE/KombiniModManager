//! Details pane "Versions": the author's changelog from GameBanana and the
//! previous versions kept for rollback.

use crate::app::AppView;
use crate::modal::Modal;
use crate::theme::*;
use crate::widgets::*;
use gpui_kit::component::button::*;
use gpui_kit::component::{Disableable, IconName, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kombini_core::ModId;
use kombini_core::freshness::format_date;
use kombini_gamebanana::GbUpdate;

/// Changelog posts fetched per mod.
const CHANGELOG_COUNT: u32 = 6;

pub enum Changelog {
    Loading,
    Ready(Vec<GbUpdate>),
    Failed,
}

fn version_label(v: Option<&str>) -> Option<String> {
    v.map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| format!("v{}", v.trim_start_matches(['v', 'V'])))
}

impl AppView {
    fn load_changelog(&mut self, remote: u64, cx: &mut Context<Self>) {
        self.changelogs.insert(remote, Changelog::Loading);
        let gb = self.gb.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { gb.updates(remote, CHANGELOG_COUNT) })
                .await;
            this.update(cx, |this, cx| {
                let state = match result {
                    Ok(list) => Changelog::Ready(list),
                    Err(e) => {
                        tracing::warn!(remote, error = %e, "changelog fetch failed");
                        Changelog::Failed
                    }
                };
                this.changelogs.insert(remote, state);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn confirm_restore_version(&mut self, id: ModId, index: usize, cx: &mut Context<Self>) {
        let Some(m) = self.lib.mods.get(&id) else {
            return;
        };
        let Some(v) = m.history.get(index) else {
            return;
        };
        let label = version_label(v.version.as_deref())
            .or_else(|| v.file_name.clone())
            .unwrap_or_else(|| "the previous version".to_owned());
        self.modal = Some(Modal::confirm(
            format!("Go back to {label}?"),
            format!(
                "{} switches to {label}. The version you have now is kept, so you can switch back. Updates will still be offered; use Skip this version to stop that.",
                m.title
            ),
            "Restore",
            false,
            move |this, cx| {
                let id = id.clone();
                this.run(
                    format!("restore-version:{id}"),
                    "Restoring version",
                    cx,
                    move |e, _| e.restore_version(&id, index),
                    |this, report, cx| {
                        this.success(report.summary);
                        this.scan_issues_quietly(cx);
                    },
                );
            },
        ));
        cx.notify();
    }

    pub fn confirm_keep_version(&mut self, id: ModId, index: usize, cx: &mut Context<Self>) {
        let Some(m) = self.lib.mods.get(&id) else {
            return;
        };
        let Some(v) = m.history.get(index) else {
            return;
        };
        let label = v
            .file_name
            .clone()
            .or_else(|| version_label(v.version.as_deref()))
            .unwrap_or_else(|| "this earlier file".to_owned());
        self.modal = Some(Modal::confirm(
            format!("Keep {label} as its own mod?"),
            format!(
                "{label} becomes a separate mod next to {}, enabled wherever it is. Use this when it was a different file that got replaced, not an older version of the same one. Needs no download.",
                m.title
            ),
            "Keep as mod",
            false,
            move |this, cx| {
                let id = id.clone();
                this.run(
                    format!("restore-version:{id}"),
                    "Adding mod",
                    cx,
                    move |e, _| e.keep_version_as_mod(&id, index),
                    |this, report, cx| {
                        this.success(report.summary);
                        this.scan_issues_quietly(cx);
                    },
                );
            },
        ));
        cx.notify();
    }

    /// The "Versions" section, or `None` when there is nothing to show.
    pub fn render_versions_section(
        &mut self,
        id: &ModId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let m = self.lib.mods.get(id)?.clone();
        if let Some(remote) = m.remote_id
            && !self.changelogs.contains_key(&remote)
        {
            self.load_changelog(remote, cx);
        }
        let changelog = m.remote_id.and_then(|r| self.changelogs.get(&r));
        let installed_at = m.remote_updated_at.unwrap_or(0);
        let busy = self.launch.phase.busy() || self.is_pending(&format!("restore-version:{id}"));

        let posts: Vec<AnyElement> = match changelog {
            Some(Changelog::Ready(list)) => list
                .iter()
                .enumerate()
                .map(|(i, u)| {
                    let newer = u.date_added.is_some_and(|d| d > installed_at + 60);
                    let body = u
                        .text
                        .as_deref()
                        .map(kombini_gamebanana::html::to_text)
                        .map(|t| t.trim().to_owned())
                        .filter(|t| !t.is_empty());
                    div()
                        .id(("changelog", i))
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .py(px(6.))
                        .when(i > 0, |d| d.border_t_1().border_color(c(BORDER)))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .children(
                                    version_label(u.version.as_deref()).map(|v| chip(v, MUTED)),
                                )
                                .child(div().flex_1().min_w_0().text_sm().truncate().child(
                                    if u.name.trim().is_empty() {
                                        "Update".to_owned()
                                    } else {
                                        u.name.clone()
                                    },
                                ))
                                .when(newer, |d| d.child(chip("Newer than yours", ACCENT)))
                                .children(u.date_added.map(|d| faint(format_date(d)))),
                        )
                        .children(u.changes.iter().map(|ch| {
                            div()
                                .flex()
                                .gap(px(6.))
                                .text_xs()
                                .when(!ch.cat.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .flex_none()
                                            .text_color(c(FAINT))
                                            .child(ch.cat.clone()),
                                    )
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_color(c(TEXT))
                                        .child(ch.text.clone()),
                                )
                        }))
                        .children(body.map(|b| div().text_xs().text_color(c(MUTED)).child(b)))
                        .into_any_element()
                })
                .collect(),
            _ => Vec::new(),
        };
        let changelog_state: Option<AnyElement> = match changelog {
            Some(Changelog::Loading) => Some(faint("Loading the changelog…").into_any_element()),
            Some(Changelog::Failed) => {
                Some(faint("Could not load the changelog.").into_any_element())
            }
            Some(Changelog::Ready(list)) if list.is_empty() => {
                Some(faint("The author has not posted any updates.").into_any_element())
            }
            _ => None,
        };
        let history: Vec<AnyElement> = m
            .history
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let restore_id = id.clone();
                let keep_id = id.clone();
                let label = version_label(v.version.as_deref())
                    .or_else(|| v.file_name.clone())
                    .unwrap_or_else(|| "Earlier version".to_owned());
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .py(px(4.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_sm().truncate().child(label))
                            .child(faint(
                                [
                                    v.remote_updated_at
                                        .map(|d| format!("uploaded {}", format_date(d))),
                                    Some(format!("replaced {}", format_date(v.replaced_at))),
                                    v.variant_desc.clone(),
                                ]
                                .into_iter()
                                .flatten()
                                .collect::<Vec<_>>()
                                .join(" · "),
                            )),
                    )
                    .child(
                        Button::new(("keep-version", i))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Plus)
                            .label("Keep as mod")
                            .tooltip("Add it as a separate mod instead of swapping")
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.confirm_keep_version(keep_id.clone(), i, cx)
                            })),
                    )
                    .child(
                        Button::new(("restore-version", i))
                            .xsmall()
                            .icon(IconName::Undo2)
                            .label("Restore")
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.confirm_restore_version(restore_id.clone(), i, cx)
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        if posts.is_empty() && changelog_state.is_none() && history.is_empty() {
            return None;
        }
        let page = m.page_url.clone();
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(section_title("Versions"))
                .when(!history.is_empty(), |d| {
                    d.child(
                        card()
                            .px(px(12.))
                            .py(px(6.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(c(FAINT))
                                    .child("Earlier versions"),
                            )
                            .children(history),
                    )
                })
                .when(!posts.is_empty() || changelog_state.is_some(), |d| {
                    d.child(
                        card()
                            .px(px(12.))
                            .py(px(6.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_xs()
                                            .text_color(c(FAINT))
                                            .child("Changelog"),
                                    )
                                    .when_some(page, |d, url| {
                                        d.child(
                                            Button::new("changelog-open")
                                                .xsmall()
                                                .ghost()
                                                .icon(IconName::ExternalLink)
                                                .label("All updates")
                                                .on_click(move |_, _, cx| {
                                                    cx.open_url(
                                                        &kombini_gamebanana::links::updates_page_url(&url),
                                                    )
                                                }),
                                        )
                                    }),
                            )
                            .children(changelog_state)
                            .children(posts),
                    )
                })
                .into_any_element(),
        )
    }
}
