//! Installing a GameBanana collection: list its Deadlock mods, let the user
//! pick, and queue the downloads.

use crate::app::{AppView, Tab};
use crate::discover::install_meta;
use crate::modal::Modal;
use crate::theme::*;
use crate::widgets::*;
use gpui_kit::component::button::*;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{Disableable, Icon, IconName, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kombini_core::{AppError, ErrorCode};
use kombini_download::DownloadSpec;
use kombini_gamebanana::{Collection, GbFile};
use std::collections::BTreeSet;
use std::sync::Arc;

pub struct CollectionView {
    pub id: u64,
    pub state: CollectionLoad,
    /// Submission ids chosen for install.
    pub selected: BTreeSet<u64>,
}

pub enum CollectionLoad {
    Loading,
    Ready(Arc<Collection>),
    Failed(String),
}

/// What queuing a collection did.
#[derive(Default)]
pub struct CollectionQueued {
    pub queued: usize,
    /// Mods with several files; the user picks one in Discover.
    pub several: Vec<String>,
    pub failed: Vec<String>,
}

impl AppView {
    fn installed_remote_ids(&self) -> BTreeSet<u64> {
        self.lib.mods.values().filter_map(|m| m.remote_id).collect()
    }

    pub fn open_collection(&mut self, id: u64, cx: &mut Context<Self>) {
        self.modal = Some(Modal::Collection(Box::new(CollectionView {
            id,
            state: CollectionLoad::Loading,
            selected: BTreeSet::new(),
        })));
        cx.notify();
        let gb = self.gb.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { gb.collection(id) })
                .await;
            this.update(cx, |this, cx| {
                let installed = this.installed_remote_ids();
                if let Some(Modal::Collection(view)) = &mut this.modal
                    && view.id == id
                {
                    match result {
                        Ok(c) => {
                            view.selected = c
                                .installable()
                                .map(|m| m.id)
                                .filter(|id| !installed.contains(id))
                                .collect();
                            view.state = CollectionLoad::Ready(Arc::new(c));
                        }
                        Err(e) => {
                            tracing::warn!(id, error = %e, "collection fetch failed");
                            view.state = CollectionLoad::Failed(
                                "Could not load this collection. It may be private or deleted."
                                    .into(),
                            );
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn collection_toggle(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(Modal::Collection(view)) = &mut self.modal
            && !view.selected.remove(&id)
        {
            view.selected.insert(id);
        }
        cx.notify();
    }

    fn collection_select(&mut self, all: bool, cx: &mut Context<Self>) {
        let installed = self.installed_remote_ids();
        if let Some(Modal::Collection(view)) = &mut self.modal
            && let CollectionLoad::Ready(c) = &view.state
        {
            view.selected = if all {
                c.installable()
                    .map(|m| m.id)
                    .filter(|id| !installed.contains(id))
                    .collect()
            } else {
                BTreeSet::new()
            };
        }
        cx.notify();
    }

    /// Queues one download per chosen mod (its only current file).
    fn queue_collection(&mut self, ids: Vec<u64>, cx: &mut Context<Self>) {
        let gb = self.gb.clone();
        let Some(dm) = self.downloads.clone() else {
            return;
        };
        self.run(
            "collection",
            "Queuing collection downloads",
            cx,
            move |_, _| {
                let mut out = CollectionQueued::default();
                for id in ids {
                    let profile = match gb.profile(id, false) {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::warn!(id, error = %e, "collection item profile failed");
                            out.failed.push(format!("#{id}"));
                            continue;
                        }
                    };
                    let files: Vec<&GbFile> = profile
                        .current_files()
                        .into_iter()
                        .filter(|f| !f.flagged())
                        .collect();
                    let file = match files.as_slice() {
                        [one] => (*one).clone(),
                        [] => {
                            out.failed.push(profile.name.clone());
                            continue;
                        }
                        _ => {
                            out.several.push(profile.name.clone());
                            continue;
                        }
                    };
                    let meta = install_meta(&profile, &file, Some(true));
                    dm.enqueue(DownloadSpec {
                        id: format!(
                            "gb{}-{}-{}",
                            profile.id,
                            file.id,
                            kombini_storage::random_hex(3)
                        ),
                        url: file.url(),
                        file_name: kombini_download::safe_file_name(&file.file_name),
                        title: profile.name.clone(),
                        expected_md5: file.md5.clone(),
                        expected_size: Some(file.size).filter(|s| *s > 0),
                        meta: serde_json::to_value(&meta).unwrap_or_default(),
                    });
                    out.queued += 1;
                }
                if out.queued == 0 && out.several.is_empty() && !out.failed.is_empty() {
                    return Err(AppError::new(
                        ErrorCode::Network,
                        "None of the collection's mods could be downloaded.",
                    ));
                }
                Ok(out)
            },
            |this, out, cx| {
                this.modal = None;
                if out.queued > 0 {
                    this.success(format!(
                        "Downloading {} mod(s) from the collection.",
                        out.queued
                    ));
                    this.tab = Tab::Downloads;
                }
                if !out.several.is_empty() {
                    this.toast(Notification::info(format!(
                        "{} mod(s) have several files, so pick one in Discover: {}",
                        out.several.len(),
                        out.several.join(", ")
                    )));
                }
                if !out.failed.is_empty() {
                    this.toast(Notification::warning(format!(
                        "Not available: {}",
                        out.failed.join(", ")
                    )));
                }
                cx.notify();
            },
        );
    }

    pub fn render_collection(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(Modal::Collection(view)) = &self.modal else {
            return div().into_any_element();
        };
        let col = match &view.state {
            CollectionLoad::Loading => {
                return div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(gpui_kit::component::spinner::Spinner::new())
                    .child(muted("Reading the collection…"))
                    .into_any_element();
            }
            CollectionLoad::Failed(e) => return muted(e.clone()).into_any_element(),
            CollectionLoad::Ready(x) => x.clone(),
        };
        let selected = view.selected.clone();
        let installed = self.installed_remote_ids();
        let busy = self.is_pending("collection");
        let installable: Vec<_> = col.installable().cloned().collect();
        let skipped = col.items.len() - installable.len();
        let already = installable
            .iter()
            .filter(|m| installed.contains(&m.id))
            .count();
        let by = col
            .profile
            .submitter
            .as_ref()
            .map(|s| format!(" by {}", s.name))
            .unwrap_or_default();
        let ids: Vec<u64> = selected.iter().copied().collect();
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .text_sm()
                    .text_color(c(TEXT))
                    .child(format!("{}{by}", col.profile.name)),
            )
            .child(faint(
                [
                    Some(format!("{} Deadlock mod(s)", installable.len())),
                    (already > 0).then(|| format!("{already} already installed")),
                    (skipped > 0).then(|| format!("{skipped} other item(s) skipped")),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · "),
            ))
            .child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(
                        Button::new("col-all")
                            .xsmall()
                            .ghost()
                            .label("Select all")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.collection_select(true, cx)),
                            ),
                    )
                    .child(
                        Button::new("col-none")
                            .xsmall()
                            .ghost()
                            .label("Select none")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.collection_select(false, cx)),
                            ),
                    ),
            )
            .child(div().flex().flex_col().gap(px(2.)).children(
                installable.into_iter().enumerate().map(|(i, m)| {
                    let have = installed.contains(&m.id);
                    let on = selected.contains(&m.id);
                    let id = m.id;
                    let category = m
                        .root_category
                        .as_ref()
                        .map(|r| r.name.clone())
                        .unwrap_or_default();
                    div()
                        .id(("col-item", i))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .px(px(8.))
                        .py(px(5.))
                        .rounded(px(6.))
                        .when(!have, |d| {
                            d.cursor_pointer().hover(|s| s.bg(c(RAISED))).on_click(
                                cx.listener(move |this, _, _, cx| this.collection_toggle(id, cx)),
                            )
                        })
                        .child(
                            div()
                                .size(px(16.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.))
                                .border_1()
                                .border_color(c(if on { ACCENT } else { BORDER }))
                                .when(on, |d| {
                                    d.bg(c(ACCENT)).child(
                                        Icon::new(IconName::Check)
                                            .size(px(12.))
                                            .text_color(c(WINDOW)),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().text_sm().truncate().child(m.name.clone()))
                                .child(faint(category)),
                        )
                        .when(have, |d| d.child(chip("Installed", MUTED)))
                        .when(m.is_nsfw(), |d| d.child(chip("Mature", CORAL)))
                }),
            ))
            .child(
                Button::new("col-install")
                    .primary()
                    .label(format!("Install {} mod(s)", ids.len()))
                    .loading(busy)
                    .disabled(busy || ids.is_empty())
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.queue_collection(ids.clone(), cx)),
                    ),
            )
            .into_any_element()
    }
}
