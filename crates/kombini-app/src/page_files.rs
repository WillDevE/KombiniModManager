//! Several files on one GameBanana page: the install window (pick one or
//! more files) and the "Files from this page" section of a mod's details.
//!
//! Every installed file is its own mod (`gb<page>`, `gb<page>_<file>`), so a
//! model and its voice-line addon can be on together and ordered like any
//! other mods; the engine keeps a page's files next to each other in the
//! load order. Installing a different file can switch to it (turn the
//! others off, like Grimoire's "switch variants") or stack it.

use crate::app::{AppView, Tab};
use crate::discover::install_meta;
use crate::modal::Modal;
use crate::smooth_scroll::Smooth as _;
use crate::theme::*;
use crate::widgets::*;
use gpui_kit::component::button::*;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable, Icon, IconName, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kombini_core::order::effective_order;
use kombini_core::{AppError, ErrorCode, Mod, ModId};
use kombini_download::DownloadSpec;
use kombini_gamebanana::{GbFile, ModProfile};
use std::collections::BTreeSet;
use std::sync::Arc;

pub struct FilePicker {
    pub remote: u64,
    pub state: PickerLoad,
    /// File ids chosen for install.
    pub selected: BTreeSet<u64>,
    /// Turn off the page's other installed files. `None` follows the
    /// setting for a single file and stays off for several.
    pub switch: Option<bool>,
    pub scroll: crate::smooth_scroll::SmoothScroll,
}

pub enum PickerLoad {
    Loading,
    Ready(Arc<ModProfile>),
    Failed(String),
}

/// Files offered for install: the current ones, or the archived ones when
/// the page has nothing else.
pub fn installable(profile: &ModProfile) -> Vec<GbFile> {
    let current: Vec<GbFile> = profile.current_files().into_iter().cloned().collect();
    if current.is_empty() {
        profile.archived_files.clone()
    } else {
        current
    }
}

/// How a page file is told apart from its siblings: the author's file
/// description, else the file name without extension.
pub fn file_label(m: &Mod) -> String {
    m.variant_desc
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            m.file_name
                .as_deref()
                .map(|n| n.rsplit_once('.').map_or(n, |(stem, _)| stem).to_owned())
        })
        .unwrap_or_else(|| "File".to_owned())
}

fn gb_file_label(f: &GbFile) -> String {
    f.description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| f.file_name.clone())
}

impl AppView {
    /// Installed files of a GameBanana page, in load order (disabled ones
    /// after the enabled, by order position).
    pub fn page_mods(&self, remote: u64) -> Vec<Mod> {
        let profile = self.lib.profile();
        let existing: BTreeSet<ModId> = self.lib.mods.keys().cloned().collect();
        let order = effective_order(profile, &existing);
        let pos = |id: &ModId| {
            order
                .iter()
                .position(|o| o == id)
                .or_else(|| {
                    profile
                        .order
                        .iter()
                        .position(|o| o == id)
                        .map(|p| p + order.len())
                })
                .unwrap_or(usize::MAX)
        };
        let mut mods: Vec<Mod> = self
            .lib
            .mods
            .values()
            .filter(|m| m.remote_id == Some(remote))
            .cloned()
            .collect();
        mods.sort_by_key(|m| pos(&m.id));
        mods
    }

    /// Opens the install window for a page. `profile` skips the fetch when
    /// the page is already loaded (the Discover sidebar).
    pub fn open_file_picker(
        &mut self,
        remote: u64,
        profile: Option<Arc<ModProfile>>,
        cx: &mut Context<Self>,
    ) {
        let state = match &profile {
            Some(p) => PickerLoad::Ready(p.clone()),
            None => PickerLoad::Loading,
        };
        let selected = profile
            .as_ref()
            .map(|p| self.picker_defaults(p))
            .unwrap_or_default();
        self.modal = Some(Modal::Files(Box::new(FilePicker {
            remote,
            state,
            selected,
            switch: None,
            scroll: crate::smooth_scroll::SmoothScroll::new(),
        })));
        cx.notify();
        if profile.is_some() {
            return;
        }
        let gb = self.gb.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { gb.profile(remote, false) })
                .await;
            this.update(cx, |this, cx| {
                let defaults = result.as_ref().ok().map(|p| this.picker_defaults(p));
                if let Some(Modal::Files(view)) = &mut this.modal
                    && view.remote == remote
                {
                    match result {
                        Ok(p) => {
                            view.selected = defaults.unwrap_or_default();
                            view.state = PickerLoad::Ready(p);
                        }
                        Err(e) => {
                            tracing::warn!(remote, error = %e, "file picker profile failed");
                            view.state = PickerLoad::Failed(
                                "Could not load this mod's files. Try again in a moment.".into(),
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

    /// Nothing from the page installed: the most downloaded file. Otherwise
    /// nothing, so the user picks what to add.
    fn picker_defaults(&self, profile: &ModProfile) -> BTreeSet<u64> {
        if self
            .lib
            .mods
            .values()
            .any(|m| m.remote_id == Some(profile.id))
        {
            return BTreeSet::new();
        }
        installable(profile)
            .iter()
            .filter(|f| !f.flagged())
            .max_by_key(|f| f.download_count)
            .map(|f| f.id)
            .into_iter()
            .collect()
    }

    fn picker_toggle(&mut self, file_id: u64, cx: &mut Context<Self>) {
        if let Some(Modal::Files(view)) = &mut self.modal
            && !view.selected.remove(&file_id)
        {
            view.selected.insert(file_id);
        }
        cx.notify();
    }

    fn picker_switch(&self, view: &FilePicker) -> bool {
        view.switch
            .unwrap_or(self.settings.switch_variants && view.selected.len() <= 1)
    }

    /// Queues downloads of `file_ids` from one page. They install as
    /// separate mods, later files of the page above earlier ones.
    pub fn queue_gamebanana_files(
        &mut self,
        remote: u64,
        file_ids: Vec<u64>,
        disable_siblings: bool,
        cx: &mut Context<Self>,
    ) {
        let gb = self.gb.clone();
        let Some(dm) = self.downloads.clone() else {
            return;
        };
        self.run(
            format!("gb:{remote}"),
            "Preparing download",
            cx,
            move |_, _| {
                let profile = gb.profile(remote, false).map_err(AppError::from)?;
                let page: Vec<u64> = profile.all_files().map(|f| f.id).collect();
                let mut files: Vec<GbFile> = Vec::new();
                for fid in &file_ids {
                    let file = profile.file(*fid).cloned().ok_or_else(|| {
                        AppError::new(ErrorCode::InvalidInput, "That file is no longer available")
                    })?;
                    if file.flagged() {
                        return Err(AppError::new(
                            ErrorCode::InvalidInput,
                            format!(
                                "GameBanana's virus scan flagged {}, so it was not downloaded.",
                                file.file_name
                            ),
                        ));
                    }
                    files.push(file);
                }
                // Highest priority first: files further down the page
                // (usually addons) load above the main file.
                let mut hint: Vec<u64> = files.iter().map(|f| f.id).collect();
                hint.sort_by_key(|f| std::cmp::Reverse(page.iter().position(|p| p == f)));
                for file in &files {
                    let mut meta = install_meta(&profile, file, Some(true));
                    meta.disable_siblings = disable_siblings;
                    if files.len() > 1 {
                        meta.order_hint = hint.clone();
                    }
                    dm.enqueue(DownloadSpec {
                        id: format!(
                            "gb{}-{}-{}",
                            profile.id,
                            file.id,
                            kombini_storage::random_hex(3)
                        ),
                        url: file.url(),
                        file_name: kombini_download::safe_file_name(&file.file_name),
                        title: if files.len() > 1 {
                            format!("{} ({})", profile.name, gb_file_label(file))
                        } else {
                            profile.name.clone()
                        },
                        expected_md5: file.md5.clone(),
                        expected_size: Some(file.size).filter(|s| *s > 0),
                        meta: serde_json::to_value(&meta).unwrap_or_default(),
                    });
                }
                Ok((profile.name.clone(), files.len()))
            },
            |this, (title, n), cx| {
                this.toast(Notification::info(if n == 1 {
                    format!("Downloading {title}…")
                } else {
                    format!("Downloading {n} files of {title}…")
                }));
                cx.notify();
            },
        );
    }

    /// Queues the update of one installed file into the same mod.
    pub fn queue_gamebanana_update(
        &mut self,
        id: ModId,
        remote: u64,
        file_id: u64,
        cx: &mut Context<Self>,
    ) {
        let gb = self.gb.clone();
        let Some(dm) = self.downloads.clone() else {
            return;
        };
        self.run(
            format!("update:{id}"),
            "Preparing download",
            cx,
            move |_, _| {
                let profile = gb.profile(remote, true).map_err(AppError::from)?;
                let file = profile.file(file_id).cloned().ok_or_else(|| {
                    AppError::new(ErrorCode::InvalidInput, "That file is no longer available")
                })?;
                if file.flagged() {
                    return Err(AppError::new(
                        ErrorCode::InvalidInput,
                        "GameBanana's virus scan flagged this file, so it was not downloaded.",
                    ));
                }
                let mut meta = install_meta(&profile, &file, None);
                meta.id = Some(id);
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
                Ok(profile.name.clone())
            },
            |this, title, cx| {
                this.toast(Notification::info(format!("Downloading {title}…")));
                cx.notify();
            },
        );
    }

    /// Installs from a page: a given file directly, the only file directly,
    /// or the install window (with the page open in Discover) when there
    /// are several.
    pub fn queue_gamebanana_install(
        &mut self,
        remote: u64,
        file_id: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        let switch = self.settings.switch_variants;
        if let Some(fid) = file_id {
            self.queue_gamebanana_files(remote, vec![fid], switch, cx);
            return;
        }
        let gb = self.gb.clone();
        self.run(
            format!("gb:{remote}"),
            "Reading the mod's files",
            cx,
            move |_, _| gb.profile(remote, false).map_err(AppError::from),
            move |this, profile, cx| {
                let files = installable(&profile);
                let usable: Vec<&GbFile> = files.iter().filter(|f| !f.flagged()).collect();
                match usable.as_slice() {
                    [] => {
                        this.error(
                            &format!("Installing {}", profile.name),
                            &AppError::new(
                                ErrorCode::InvalidInput,
                                "This mod has no downloadable files (they may be archived or flagged).",
                            ),
                        );
                    }
                    [one] if files.len() == 1 => {
                        let fid = one.id;
                        this.queue_gamebanana_files(remote, vec![fid], switch, cx);
                    }
                    _ => {
                        if this.discover.selected != Some(remote) {
                            this.discover_open(remote, false, cx);
                        }
                        this.tab = Tab::Discover;
                        this.open_file_picker(remote, Some(profile), cx);
                    }
                }
                cx.notify();
            },
        );
    }

    pub fn render_file_picker(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(Modal::Files(view)) = &self.modal else {
            return div().into_any_element();
        };
        let profile = match &view.state {
            PickerLoad::Loading => {
                return div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(gpui_kit::component::spinner::Spinner::new())
                    .child(muted("Reading the mod's files…"))
                    .into_any_element();
            }
            PickerLoad::Failed(e) => return muted(e.clone()).into_any_element(),
            PickerLoad::Ready(p) => p.clone(),
        };
        let remote = view.remote;
        let selected = view.selected.clone();
        let switch = self.picker_switch(view);
        let scroll = view.scroll.clone();
        let installed = self.page_mods(remote);
        let enabled: BTreeSet<ModId> = self.lib.profile().enabled.clone();
        let busy = self.is_pending(&format!("gb:{remote}"));
        let files = installable(&profile);
        // Installed files of the page that the picked ones would turn off.
        let others_on = installed
            .iter()
            .filter(|m| enabled.contains(&m.id))
            .filter(|m| !m.file_id.is_some_and(|f| selected.contains(&f)))
            .count();
        let ids: Vec<u64> = files
            .iter()
            .map(|f| f.id)
            .filter(|f| selected.contains(f))
            .collect();
        let n = ids.len();
        let label = match n {
            0 => "Install".to_owned(),
            1 => "Install 1 file".to_owned(),
            n => format!("Install {n} files"),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(muted(format!(
                "{} has {} files. Pick one, or several to use together (a model and its voice lines, say). Each installs as its own mod, next to the others in the load order.",
                profile.name,
                files.len()
            )))
            .child(
                div()
                    .relative()
                    .child(
                div()
                    .id("picker-files")
                    .max_h(px(420.))
                    .overflow_y_scroll()
                    .smooth(&scroll, cx)
                    // Room for the scrollbar beside the rows.
                    .pr(px(12.))
                    .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .children(files.into_iter().enumerate().map(|(i, f)| {
                        let fid = f.id;
                        let flagged = f.flagged();
                        let on = selected.contains(&fid);
                        let have = installed.iter().find(|m| m.file_id == Some(fid));
                        let have_on = have.is_some_and(|m| enabled.contains(&m.id));
                        let outdated = self.date_maybe_outdated(f.date_added);
                        let desc = f
                            .description
                            .clone()
                            .filter(|d| !d.trim().is_empty());
                        div()
                            .id(("picker-file", i))
                            .flex()
                            .items_start()
                            .gap(px(10.))
                            .p(px(10.))
                            .rounded(px(RADIUS))
                            .border_1()
                            .border_color(c(if on { ACCENT } else { BORDER }))
                            .bg(if on { ca(ACCENT, 0.06) } else { c(PANEL) })
                            .when(!flagged, |d| {
                                d.cursor_pointer().hover(|s| s.bg(c(HOVER))).on_click(
                                    cx.listener(move |this, _, _, cx| this.picker_toggle(fid, cx)),
                                )
                            })
                            .child(
                                div()
                                    .mt(px(2.))
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
                                    .gap(px(3.))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .text_sm()
                                                    .truncate()
                                                    .child(f.file_name.clone()),
                                            )
                                            .child(faint(format_bytes(f.size))),
                                    )
                                    .children(desc.map(|d| div().text_xs().text_color(c(MUTED)).child(d)))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .items_center()
                                            .gap(px(6.))
                                            .when_some(have.is_some().then_some(have_on), |d, on| {
                                                d.child(chip(
                                                    if on { "Installed · on" } else { "Installed · off" },
                                                    if on { ACCENT } else { MUTED },
                                                ))
                                            })
                                            .when(outdated, |d| d.child(chip("May be outdated", MUTED)))
                                            .when(flagged, |d| d.child(chip("Flagged by virus scan", CORAL)))
                                            .when(f.archived, |d| d.child(chip("Archived", MUTED)))
                                            .child(faint(format!(
                                                "{} downloads{}",
                                                format_count(f.download_count),
                                                f.date_added
                                                    .map(|t| format!(
                                                        " · uploaded {}",
                                                        kombini_core::freshness::format_date(t)
                                                    ))
                                                    .unwrap_or_default()
                                            ))),
                                    ),
                            )
                    })),
                    ),
                    )
                    .vertical_scrollbar(&scroll.handle),
            )
            .when(others_on > 0, |d| {
                d.child(
                    div()
                        .id("picker-switch")
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(Modal::Files(view)) = &mut this.modal {
                                view.switch = Some(!switch);
                            }
                            cx.notify();
                        }))
                        .child(Switch::new("picker-switch-sw").small().checked(switch))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(div().text_sm().child(format!(
                                    "Turn off the {} you have on",
                                    if others_on == 1 {
                                        "file".to_owned()
                                    } else {
                                        format!("{others_on} files")
                                    }
                                )))
                                .child(faint(if switch {
                                    "Switches to the picked files; the others stay installed."
                                } else {
                                    "Keeps them on together with the picked files."
                                })),
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        Button::new("picker-none")
                            .ghost()
                            .small()
                            .text_label("Clear")
                            .disabled(n == 0)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(Modal::Files(view)) = &mut this.modal {
                                    view.selected.clear();
                                }
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("picker-cancel")
                            .ghost()
                            .text_label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.close_modal(cx))),
                    )
                    .child(
                        Button::new("picker-install")
                            .primary()
                            .icon(IconName::ArrowDown)
                            .text_label(label)
                            .loading(busy)
                            .disabled(busy || n == 0)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.modal = None;
                                this.queue_gamebanana_files(remote, ids.clone(), switch, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    /// "Files from this page" in a mod's details: the page's installed
    /// files with on/off switches, and a way to add more. `None` unless
    /// two or more files of the page are installed.
    pub fn render_page_files_section(
        &mut self,
        id: &ModId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let remote = self.lib.mods.get(id)?.remote_id?;
        let mods = self.page_mods(remote);
        if mods.len() < 2 {
            return None;
        }
        let enabled = self.lib.profile().enabled.clone();
        let busy = self.is_pending(&format!("gb:{remote}"));
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(section_title("Files from this page"))
                .child(card().px(px(4.)).py(px(4.)).flex().flex_col().children(
                    mods.iter().enumerate().map(|(i, m)| {
                        let on = enabled.contains(&m.id);
                        let this_one = &m.id == id;
                        let toggle = m.id.clone();
                        let open = m.id.clone();
                        div()
                            .id(("page-file", i))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .px(px(8.))
                            .py(px(6.))
                            .rounded(px(6.))
                            .when(this_one, |d| d.bg(c(RAISED)))
                            .when(!this_one, |d| {
                                d.cursor_pointer()
                                    .hover(|s| s.bg(c(HOVER)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.selected = Some(open.clone());
                                        cx.notify();
                                    }))
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_sm().truncate().child(file_label(m)))
                                    .child(faint(
                                        [
                                            m.file_name.clone(),
                                            Some(format_bytes(m.total_size())),
                                            this_one.then(|| "this one".to_owned()),
                                        ]
                                        .into_iter()
                                        .flatten()
                                        .collect::<Vec<_>>()
                                        .join(" · "),
                                    )),
                            )
                            .child(
                                Switch::new(("page-file-sw", i))
                                    .small()
                                    .checked(on)
                                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                        this.set_enabled(vec![toggle.clone()], *checked, cx);
                                    })),
                            )
                    }),
                ))
                .child(
                    Button::new("page-files-add")
                        .small()
                        .icon(IconName::Plus)
                        .text_label("Install other files…")
                        .loading(busy)
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_file_picker(remote, None, cx)
                        })),
                )
                .into_any_element(),
        )
    }
}
