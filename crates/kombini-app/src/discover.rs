//! Discover: GameBanana browse/search grid, mod detail with file picker,
//! install queueing and update checks.

use crate::app::{AppView, Thumb};
use crate::ops::OpProgress;
use crate::smooth_scroll::Smooth as _;
use crate::theme::*;
use crate::widgets::*;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{Disableable, Icon, IconName, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kombini_core::{AppError, ModSource};
use kombini_engine::install::InstallMeta;
use kombini_engine::{Engine, NoProgress, Progress};
use kombini_gamebanana::updates::{Installed, find_update};
use kombini_gamebanana::{BrowsePage, BrowseQuery, Category, Client, GbFile, ModProfile, Sort};
use std::sync::Arc;
use std::time::Duration;

pub const PER_PAGE: u32 = 24;
/// Detail pane width.
const DETAIL_W: f32 = 400.;
/// Horizontal padding of the card grid.
const GRID_PAD: f32 = 16.;
/// Gap between cards.
const CARD_GAP: f32 = 12.;
/// Narrowest a card gets before the grid drops a column.
const CARD_MIN: f32 = 220.;
/// Height of the text panel under the visible part of a Discover thumbnail.
const CARD_INFO_H: f32 = 96.;

pub struct Discover {
    pub input: Entity<InputState>,
    pub query: BrowseQuery,
    /// Every result loaded so far (pages are appended as you scroll).
    pub page: Option<Arc<BrowsePage>>,
    /// Loading the first page of a new search/filter/sort.
    pub loading: bool,
    /// Loading the next page at the bottom of the list.
    pub loading_more: bool,
    pub more_error: Option<String>,
    /// The last page has been loaded.
    pub exhausted: bool,
    pub scroll: ScrollHandle,
    /// Cards play their entrance animation until then (a new result set).
    pub animate_until: Option<std::time::Instant>,
    pub error: Option<String>,
    pub seq: u64,
    /// Bumped when a new page of results arrives; keys the card entrance
    /// animation so it plays once per result set, not on every request.
    pub page_seq: u64,
    pub selected: Option<u64>,
    pub profile: Option<Arc<ModProfile>>,
    pub profile_loading: bool,
    pub profile_error: Option<String>,
    pub categories: Vec<Category>,
    /// Top-level category chosen in the first chip row.
    pub parent: Option<u64>,
    /// Child categories by parent (Skins → heroes).
    pub subcats: std::collections::HashMap<u64, Vec<Category>>,
    /// Hide mods last updated before the last major game update.
    pub hide_outdated: bool,
    /// Screenshot shown in the detail pane.
    pub gallery: usize,
    pub debounce: Option<Task<()>>,
}

impl Discover {
    pub fn new(input: Entity<InputState>) -> Self {
        Self {
            input,
            query: BrowseQuery {
                page: 1,
                per_page: PER_PAGE,
                sort: Sort::MostDownloaded,
                search: String::new(),
                category: None,
            },
            page: None,
            loading: false,
            loading_more: false,
            more_error: None,
            exhausted: false,
            scroll: ScrollHandle::new(),
            animate_until: None,
            error: None,
            seq: 0,
            page_seq: 0,
            selected: None,
            profile: None,
            profile_loading: false,
            profile_error: None,
            categories: Vec::new(),
            parent: None,
            subcats: std::collections::HashMap::new(),
            hide_outdated: false,
            gallery: 0,
            debounce: None,
        }
    }
}

/// Whether `page` is the last one of its search, with `loaded` results in
/// hand (GameBanana's page size is not always the one asked for).
fn is_last_page(page: &BrowsePage, loaded: usize) -> bool {
    let count = page.metadata.record_count;
    page.metadata.complete || page.records.is_empty() || (count > 0 && loaded as u64 >= count)
}

fn gb_err(e: kombini_gamebanana::GbError) -> AppError {
    e.into()
}

/// Checks every GameBanana mod for updates; returns (found, failed).
pub fn check_updates(
    engine: &Engine,
    gb: &Client,
    progress: &OpProgress,
) -> Result<(usize, usize), AppError> {
    let mods: Vec<(kombini_core::ModId, u64, Installed)> = engine.with_state(|s| {
        s.mods
            .values()
            .filter_map(|m| {
                Some((
                    m.id.clone(),
                    m.remote_id?,
                    Installed {
                        file_id: m.file_id,
                        md5: m.md5.clone(),
                        version: m.version.clone(),
                        remote_updated_at: m.remote_updated_at,
                        variant_desc: m.variant_desc.clone(),
                        file_name: m.file_name.clone(),
                        skipped_file_id: m.update.skipped_file_id,
                    },
                ))
            })
            .collect()
    });
    progress.stage("Checking GameBanana");
    let total = mods.len() as u64;
    let mut results = Vec::new();
    let mut failed = 0;
    for (i, (id, remote, installed)) in mods.into_iter().enumerate() {
        if progress.cancelled() {
            return Err(AppError::cancelled());
        }
        progress.progress(i as u64, total);
        match gb.profile(remote, true) {
            Ok(profile) => results.push((id, find_update(&installed, &profile))),
            Err(e) => {
                tracing::warn!(remote, error = %e, "update check failed");
                failed += 1;
            }
        }
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let owned: std::collections::BTreeSet<(u64, u64)> = engine.with_state(|s| {
        s.mods
            .values()
            .filter_map(|m| Some((m.remote_id?, m.file_id?)))
            .collect()
    });
    let remotes: std::collections::BTreeMap<kombini_core::ModId, u64> = engine.with_state(|s| {
        s.mods
            .values()
            .filter_map(|m| Some((m.id.clone(), m.remote_id?)))
            .collect()
    });
    for (id, candidate) in &mut results {
        if let (Some(c), Some(remote)) = (candidate.as_ref(), remotes.get(id))
            && owned.contains(&(*remote, c.file.id))
        {
            *candidate = None;
        }
    }
    let found = results.iter().filter(|(_, c)| c.is_some()).count();
    engine.mutate("check_updates", &NoProgress, move |s| {
        for (id, candidate) in results {
            if let Some(m) = s.mods.get_mut(&id) {
                m.update.checked_at = Some(now);
                match candidate {
                    Some(c) => {
                        m.update.available = true;
                        m.update.file_id = Some(c.file.id);
                        m.update.latest_version = c.version;
                    }
                    None => {
                        m.update.available = false;
                        m.update.file_id = None;
                    }
                }
            }
        }
        Ok(format!("{found} update(s) found"))
    })?;
    Ok((found, failed))
}

pub fn install_meta(profile: &ModProfile, file: &GbFile, enable: Option<bool>) -> InstallMeta {
    let images = profile.images();
    InstallMeta {
        title: Some(profile.name.clone()),
        source: Some(ModSource::Gamebanana),
        remote_id: Some(profile.id),
        author: profile.submitter.as_ref().map(|s| s.name.clone()),
        version: profile.version.clone(),
        category: profile.category.as_ref().map(|c| c.name.clone()),
        page_url: profile.profile_url.clone(),
        description: Some(profile.description_text()).filter(|d| !d.is_empty()),
        thumbnail_url: images.first().cloned(),
        gallery: images,
        file_id: Some(file.id),
        file_name: Some(file.file_name.clone()),
        md5: file.md5.clone(),
        variant_desc: file.description.clone().filter(|d| !d.trim().is_empty()),
        remote_updated_at: file.date_added.or(profile.updated()),
        requirements: profile.requirements(),
        enable,
        ..InstallMeta::default()
    }
}

impl AppView {
    pub fn discover_search_changed(&mut self, text: String, cx: &mut Context<Self>) {
        if text.trim() == self.discover.query.search.trim() {
            return;
        }
        self.discover.query.search = text;
        self.discover.query.page = 1;
        // Debounce: replacing the task cancels the previous timer.
        self.discover.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(350))
                .await;
            this.update(cx, |this, cx| this.discover_fetch(cx)).ok();
        }));
    }

    /// Chooses a top-level category (or All) and loads its children.
    fn discover_pick_parent(&mut self, id: Option<u64>, cx: &mut Context<Self>) {
        self.discover.parent = id;
        self.discover.query.category = id;
        self.discover.query.page = 1;
        self.discover_fetch(cx);
        let Some(parent) = id else {
            return;
        };
        if self.discover.subcats.contains_key(&parent) {
            return;
        }
        let gb = self.gb.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { gb.subcategories(parent) })
                .await;
            this.update(cx, |this, cx| {
                let list = result
                    .map(|l| l.iter().filter(|c| !c.obsolete).cloned().collect())
                    .unwrap_or_default();
                this.discover.subcats.insert(parent, list);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Loads the first page of the current search, filters and sort, and
    /// returns the list to the top.
    pub fn discover_fetch(&mut self, cx: &mut Context<Self>) {
        self.discover.seq += 1;
        let seq = self.discover.seq;
        self.discover.query.page = 1;
        self.discover.loading = true;
        self.discover.loading_more = false;
        self.discover.more_error = None;
        self.discover.exhausted = false;
        self.discover.error = None;
        self.discover.scroll.set_offset(point(px(0.), px(0.)));
        let gb = self.gb.clone();
        let query = self.discover.query.clone();
        let need_categories = self.discover.categories.is_empty();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let page = gb.browse(&query).map_err(gb_err);
                    let cats = if need_categories {
                        gb.categories().ok()
                    } else {
                        None
                    };
                    (page, cats)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.discover.seq != seq {
                    return; // a newer request superseded this one
                }
                this.discover.loading = false;
                let (page, cats) = result;
                if let Some(cats) = cats {
                    this.discover.categories =
                        cats.iter().filter(|c| !c.obsolete).cloned().collect();
                }
                match page {
                    Ok(page) => {
                        this.discover.exhausted = is_last_page(&page, page.records.len());
                        this.discover.page = Some(page);
                        this.discover.page_seq += 1;
                        this.discover.animate_until =
                            Some(std::time::Instant::now() + Duration::from_millis(900));
                        this.discover.scroll.set_offset(point(px(0.), px(0.)));
                    }
                    Err(e) => {
                        let hint = crate::app::hint(e.code).unwrap_or("");
                        this.discover.error = Some(format!("{} {hint}", e.message));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Appends the next page of results (infinite scroll).
    pub fn discover_load_more(&mut self, cx: &mut Context<Self>) {
        let d = &self.discover;
        if d.loading || d.loading_more || d.exhausted || d.page.is_none() {
            return;
        }
        let seq = d.seq;
        let next = d.query.page.max(1) + 1;
        let mut query = d.query.clone();
        query.page = next;
        self.discover.loading_more = true;
        self.discover.more_error = None;
        let gb = self.gb.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { gb.browse(&query).map_err(gb_err) })
                .await;
            this.update(cx, |this, cx| {
                if this.discover.seq != seq {
                    return; // the search changed meanwhile
                }
                this.discover.loading_more = false;
                match result {
                    Ok(more) => {
                        this.discover.query.page = next;
                        if let Some(current) = &this.discover.page {
                            let mut merged = BrowsePage::clone(current);
                            let seen: std::collections::HashSet<u64> =
                                merged.records.iter().map(|r| r.id).collect();
                            let fresh: Vec<_> = more
                                .records
                                .iter()
                                .filter(|r| !seen.contains(&r.id))
                                .cloned()
                                .collect();
                            if fresh.is_empty() {
                                this.discover.exhausted = true;
                            }
                            merged.records.extend(fresh);
                            if is_last_page(&more, merged.records.len()) {
                                this.discover.exhausted = true;
                            }
                            this.discover.page = Some(Arc::new(merged));
                        }
                    }
                    Err(e) => this.discover.more_error = Some(e.message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn discover_open(&mut self, id: u64, force: bool, cx: &mut Context<Self>) {
        self.discover.selected = Some(id);
        self.discover.gallery = 0;
        self.discover.profile = None;
        self.discover.profile_loading = true;
        self.discover.profile_error = None;
        let gb = self.gb.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { gb.profile(id, force).map_err(gb_err) })
                .await;
            this.update(cx, |this, cx| {
                if this.discover.selected != Some(id) {
                    return;
                }
                this.discover.profile_loading = false;
                match result {
                    Ok(p) => this.discover.profile = Some(p),
                    Err(e) => this.discover.profile_error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn thumb(&mut self, url: &str, cx: &mut Context<Self>) -> Option<std::path::PathBuf> {
        match self.thumbs.get(url) {
            Some(Thumb::Ready(p)) => return Some(p.clone()),
            Some(_) => return None,
            None => {}
        }
        self.thumbs.insert(url.to_owned(), Thumb::Loading);
        let images = self.images.clone();
        let url = url.to_owned();
        cx.spawn(async move |this, cx| {
            let u = url.clone();
            let result = cx
                .background_executor()
                .spawn(async move { images.get(&u) })
                .await;
            this.update(cx, |this, cx| {
                this.thumbs.insert(
                    url,
                    match result {
                        Ok(p) => Thumb::Ready(p),
                        Err(_) => Thumb::Failed,
                    },
                );
                cx.notify();
            })
            .ok();
        })
        .detach();
        None
    }

    pub fn render_discover(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Grid width: window minus the detail pane and the grid's padding.
        let detail = if self.discover.selected.is_some() {
            DETAIL_W + 1.
        } else {
            0.
        };
        let grid_w =
            (f32::from(window.viewport_size().width) - detail - 2. * GRID_PAD).max(CARD_MIN);
        div()
            .size_full()
            .flex()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(self.render_discover_toolbar(cx))
                    .child(self.render_discover_grid(grid_w, cx)),
            )
            .when(self.discover.selected.is_some(), |d| {
                d.child(self.render_discover_detail(cx))
            })
    }

    fn render_discover_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let sort = self.discover.query.sort;
        let category = self.discover.query.category;
        let parent = self.discover.parent;
        let subs: Vec<Category> = parent
            .and_then(|p| self.discover.subcats.get(&p).cloned())
            .unwrap_or_default();
        let hide_outdated = self.discover.hide_outdated;
        let cats: Vec<(Option<u64>, String)> = std::iter::once((None, "All".to_owned()))
            .chain(
                self.discover
                    .categories
                    .iter()
                    .take(10)
                    .map(|c| (Some(c.id), c.name.clone())),
            )
            .collect();
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(8.))
            .px(px(16.))
            .py(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(320.))
                            .child(
                                Input::new(&self.discover.input)
                                    .h(px(BAR_H))
                                    .cleanable(true)
                                    .prefix(
                                        gpui_kit::component::Icon::new(IconName::Search)
                                            .size(px(14.))
                                            .text_color(c(FAINT)),
                                    ),
                            ),
                    )
                    .child(segments().children(Sort::ALL.into_iter().map(|s| {
                        segment(format!("sort-{}", s.label()), s.label(), s == sort).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.discover.query.sort = s;
                                this.discover.query.page = 1;
                                this.discover_fetch(cx);
                            }),
                        )
                    })))
                    .child(div().flex_1())
                    .child(
                        Button::new("add-from-link")
                            .small()
                            .ghost()
                            .h(px(BAR_H))
                            .icon(crate::icons::Lucide::Link)
                            .text_label("Add from link")
                            .tooltip("Install a GameBanana mod or collection, or a share code")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_prompt(
                                    "Add from link",
                                    "Paste a GameBanana mod or collection link, a kombini:// link, or a profile share code.",
                                    "",
                                    "Continue",
                                    |this, text, cx| {
                                        if kombini_gamebanana::links::parse(&text).is_none() {
                                            this.toast(Notification::info(
                                                "That is not a GameBanana link or share code.",
                                            ));
                                            return;
                                        }
                                        this.handle_external(&text, cx);
                                    },
                                    window,
                                    cx,
                                )
                            })),
                    )
                    .child(
                        Button::new("hide-outdated")
                            .small()
                            .ghost()
                            .h(px(BAR_H))
                            .icon(if hide_outdated { IconName::Check } else { IconName::EyeOff })
                            .when(hide_outdated, |b| gpui_kit::component::Selectable::selected(b, true))
                            .text_label("Hide may be outdated")
                            .tooltip("Hide mods flagged as possibly broken by a game update")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.discover.hide_outdated = !this.discover.hide_outdated;
                                cx.notify();
                            })),
                    )
                    .when(self.discover.loading, |d| {
                        d.child(gpui_kit::component::spinner::Spinner::new().small())
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .children(cats.into_iter().map(|(id, name)| {
                        choice_chip(format!("cat-{name}"), name, id == parent).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.discover_pick_parent(id, cx);
                            }),
                        )
                    })),
            )
            .when(!subs.is_empty(), |d| {
                let parent_id = parent;
                d.child(
                    div().flex().flex_wrap().gap(px(2.)).children(
                        std::iter::once((parent_id, "All".to_owned()))
                            .chain(subs.into_iter().map(|c| (Some(c.id), c.name)))
                            .map(|(id, name)| {
                                // A wrapping row has no height to fill.
                                segment(format!("subcat-{name}"), name, id == category)
                                    .h(px(26.))
                                    .on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.discover.query.category = id;
                                        this.discover.query.page = 1;
                                        this.discover_fetch(cx);
                                    }),
                                )
                            }),
                    ),
                )
            })
    }

    fn render_discover_grid(&mut self, grid_w: f32, cx: &mut Context<Self>) -> AnyElement {
        // As many columns as fit at CARD_MIN, then stretch cards to fill the row.
        let cols = ((grid_w + CARD_GAP) / (CARD_MIN + CARD_GAP))
            .floor()
            .max(1.);
        let card_w = ((grid_w - CARD_GAP * (cols - 1.)) / cols).floor();
        let thumb_h = (card_w * 0.5625).round();
        // The thumbnail fills the whole card; the text sits on a translucent
        // panel over its bottom part, so any image aspect ratio looks right.
        let card_h = thumb_h + CARD_INFO_H;
        if let Some(err) = self.discover.error.clone() {
            return div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(12.))
                .child(heading("Could not reach GameBanana"))
                .child(muted(err))
                .child(
                    Button::new("discover-retry")
                        .primary()
                        .text_label("Retry")
                        .on_click(cx.listener(|this, _, _, cx| this.discover_fetch(cx))),
                )
                .into_any_element();
        }
        let Some(page) = self.discover.page.clone() else {
            // Skeleton grid while the first page loads.
            return div()
                .flex_1()
                .p(px(16.))
                .flex()
                .flex_wrap()
                .gap(px(12.))
                .children((0..12usize).map(|i| {
                    crate::anim::breathe(
                        div()
                            .w(px(CARD_MIN))
                            .h(px((CARD_MIN * 0.5625).round() + CARD_INFO_H))
                            .rounded(px(10.))
                            .bg(c(RAISED)),
                        ("skeleton", i),
                        0.45,
                        Duration::from_millis(1600),
                    )
                }))
                .into_any_element();
        };
        if page.records.is_empty() {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(muted("No mods found. Try another search or category."))
                .into_any_element();
        }
        let installed: std::collections::HashMap<u64, bool> = self
            .lib
            .mods
            .values()
            .filter_map(|m| Some((m.remote_id?, m.update.available)))
            .collect();
        let selected = self.discover.selected;
        let seq = self.discover.page_seq;
        let hide_nsfw = self.settings.hide_nsfw;
        let hidden_nsfw = if hide_nsfw {
            page.records
                .iter()
                .filter(|r| !r.obsolete && r.is_nsfw())
                .count()
        } else {
            0
        };
        let hide_outdated = self.discover.hide_outdated;
        let visible: Vec<&kombini_gamebanana::ModSummary> = page
            .records
            .iter()
            .filter(|r| !r.obsolete && !(hide_nsfw && r.is_nsfw()))
            .collect();
        let hidden_outdated = if hide_outdated {
            visible
                .iter()
                .filter(|r| self.date_maybe_outdated(r.updated()))
                .count()
        } else {
            0
        };
        let visible: Vec<&kombini_gamebanana::ModSummary> = visible
            .into_iter()
            .filter(|r| !(hide_outdated && self.date_maybe_outdated(r.updated())))
            .collect();
        // Only the rows near the viewport are built; spacers stand in for the
        // rest, so a long list scrolls as fast as a short one.
        let cols_n = cols as usize;
        let row_h = card_h + CARD_GAP;
        let rows = visible.len().div_ceil(cols_n);
        let scroll = self.discover.scroll.clone();
        let view_h = f32::from(scroll.bounds().size.height);
        let top = (-f32::from(scroll.offset().y)).max(0.);
        let est_view = if view_h > 0. { view_h } else { 900. };
        let first = ((top / row_h).floor() as usize).saturating_sub(2).min(rows);
        let last = (((top + est_view) / row_h).ceil() as usize + 2).min(rows);
        // Near the end (within a screen): fetch the next page.
        let content_h = rows as f32 * row_h;
        if view_h > 0. && top + view_h * 2. >= content_h && self.discover.more_error.is_none() {
            self.discover_load_more(cx);
        }
        let animate = self
            .discover
            .animate_until
            .is_some_and(|t| std::time::Instant::now() < t);
        let mut row_elems: Vec<AnyElement> = Vec::new();
        for row in first..last {
            let start = row * cols_n;
            let end = (start + cols_n).min(visible.len());
            let mut cards: Vec<AnyElement> = Vec::new();
            for (n, r) in visible[start..end].iter().enumerate() {
                let n = start + n;
                let id = r.id;
                let thumb = r.thumbnail(220).and_then(|u| self.thumb(&u, cx));
                let author = r
                    .submitter
                    .as_ref()
                    .map(|s| s.name.clone())
                    .unwrap_or_default();
                let state = installed.get(&id).copied();
                let busy = self.is_pending(&format!("gb:{id}"));
                let outdated = self.date_maybe_outdated(r.updated());
                let card = picture_card(
                    ("gb-card", id as usize),
                    card_w,
                    card_h,
                    thumb,
                    if selected == Some(id) { ACCENT } else { BORDER },
                )
                .cursor_pointer()
                .hover(|s| s.border_color(c(FAINT)))
                .when(outdated, |d| d.child(crate::age::outdated_badge()))
                .child(
                    card_info()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .child(r.name.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .flex_1()
                                        .truncate()
                                        .text_xs()
                                        .text_color(c(FAINT))
                                        .child(author),
                                )
                                .child(faint(format!("♥ {}", format_count(r.likes)))),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .children(
                                    r.root_category
                                        .as_ref()
                                        .map(|c| chip(c.name.clone(), MUTED)),
                                )
                                .child(div().flex_1())
                                .child(match state {
                                    Some(true) => chip("Update", ACCENT),
                                    Some(false) => chip("Installed", FAINT),
                                    None => Button::new(("gb-install", id as usize))
                                        .small()
                                        .primary()
                                        .text_label("Install")
                                        .loading(busy)
                                        .disabled(busy)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.queue_gamebanana_install(id, None, cx)
                                        }))
                                        .into_any_element()
                                        .into_div(),
                                }),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.discover_open(id, false, cx)));
                // Entrance animation only right after a new result set
                // arrives; cards scrolled back into view just appear.
                cards.push(if animate {
                    crate::anim::rise_in_delayed(
                        card,
                        SharedString::from(format!("gb-in-{seq}-{id}")),
                        10.,
                        Duration::from_millis(25 * n.min(12) as u64),
                        crate::anim::MEDIUM,
                    )
                    .into_any_element()
                } else {
                    card.into_any_element()
                });
            }
            row_elems.push(
                div()
                    .flex()
                    .gap(px(CARD_GAP))
                    .pb(px(CARD_GAP))
                    .children(cards)
                    .into_any_element(),
            );
        }
        let footer = if self.discover.loading_more {
            div()
                .flex()
                .justify_center()
                .py(px(12.))
                .child(gpui_kit::component::spinner::Spinner::new())
                .into_any_element()
        } else if let Some(err) = self.discover.more_error.clone() {
            div()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .py(px(12.))
                .child(muted(format!("Could not load more: {err}")))
                .child(
                    Button::new("discover-more-retry")
                        .small()
                        .text_label("Retry")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.discover.more_error = None;
                            this.discover_load_more(cx);
                        })),
                )
                .into_any_element()
        } else if self.discover.exhausted {
            div()
                .flex()
                .justify_center()
                .py(px(12.))
                .child(faint(format!(
                    "That's all {} result(s).",
                    page.metadata.record_count.max(page.records.len() as u64)
                )))
                .into_any_element()
        } else {
            div().h(px(44.)).into_any_element()
        };
        // Past about a screen down, offer a jump back to the top.
        let show_top = top > est_view;
        let list = div()
            .id("discover-scroll")
            .flex_1()
            .min_h_0()
            // Old results stay put (dimmed) until the new ones arrive.
            .when(self.discover.loading, |d| d.opacity(0.5))
            .overflow_y_scroll()
            .smooth(&self.smooth.for_handle("discover", &scroll), cx)
            // Re-render while scrolling so rows and paging follow the view.
            .on_scroll(cx.listener(|_, _, _, cx| cx.notify()))
            .px(px(16.))
            .pb(px(16.))
            .when(hidden_nsfw > 0, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .pb(px(10.))
                        .text_xs()
                        .text_color(c(FAINT))
                        .child(format!("{hidden_nsfw} NSFW mod(s) hidden."))
                        .child(
                            div()
                                .id("nsfw-settings")
                                .text_color(c(INFO))
                                .cursor_pointer()
                                .hover(|s| s.underline())
                                .child("Change in Settings")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.set_tab(crate::app::Tab::Settings, cx)
                                })),
                        ),
                )
            })
            .when(hidden_outdated > 0, |d| {
                d.child(
                    div()
                        .pb(px(10.))
                        .text_xs()
                        .text_color(c(FAINT))
                        .child(format!(
                            "{hidden_outdated} mod(s) last updated before the last major game update are hidden."
                        )),
                )
            })
            .child(div().h(px(first as f32 * row_h)))
            .children(row_elems)
            .child(div().h(px((rows - last) as f32 * row_h)))
            .child(footer)
            ;
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(list)
            // On the frame, not in the content, so it stays put while scrolling.
            .vertical_scrollbar(&scroll)
            .when(show_top, |d| {
                d.child(
                    div()
                        .id("discover-top")
                        .absolute()
                        .bottom(px(24.))
                        .right(px(32.))
                        // Clicks stop here instead of reaching the cards below.
                        .occlude()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(18.))
                        .py(px(11.))
                        .rounded_full()
                        .bg(c(RAISED))
                        .border_2()
                        .border_color(c(ACCENT))
                        .shadow_lg()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(c(TEXT))
                        .cursor_pointer()
                        .hover(|s| s.bg(c(PANEL)).border_color(c(TEXT)))
                        .child(Icon::new(IconName::ArrowUp).size_4().text_color(c(ACCENT)))
                        .child(ctl_text("Back to top"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.discover.scroll.set_offset(point(px(0.), px(0.)));
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }

    fn render_discover_detail(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let shell = div()
            .id("gb-detail")
            .w(px(DETAIL_W))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .smooth(&self.smooth.get("discover-detail"), cx)
            .border_l_1()
            .border_color(c(BORDER))
            .bg(c(PANEL))
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(12.));
        let close = Button::new("gb-close")
            .small()
            .ghost()
            .icon(gpui_kit::component::IconName::Close)
            .tooltip("Close")
            .on_click(cx.listener(|this, _, _, cx| {
                this.discover.selected = None;
                cx.notify();
            }));
        if self.discover.profile_loading {
            return shell
                .child(div().flex().justify_end().child(close))
                .child(div().h(px(200.)).rounded(px(RADIUS)).bg(c(RAISED)))
                .child(
                    div()
                        .h(px(20.))
                        .w(relative(0.6))
                        .rounded(px(4.))
                        .bg(c(RAISED)),
                )
                .child(
                    div()
                        .h(px(14.))
                        .w(relative(0.4))
                        .rounded(px(4.))
                        .bg(c(RAISED)),
                )
                .into_any_element();
        }
        if let Some(err) = self.discover.profile_error.clone() {
            let id = self.discover.selected.unwrap_or_default();
            return shell
                .child(div().flex().justify_end().child(close))
                .child(muted(err))
                .child(
                    Button::new("gb-retry")
                        .small()
                        .text_label("Retry")
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.discover_open(id, true, cx)),
                        ),
                )
                .into_any_element();
        }
        let Some(p) = self.discover.profile.clone() else {
            return shell.child(close).into_any_element();
        };
        let gallery = self.render_gallery(&p, cx);
        let installed = self.page_mods(p.id);
        let enabled = self.lib.profile().enabled.clone();
        let files: Vec<GbFile> = crate::page_files::installable(&p);
        let archived = p.archived_files.len();
        let desc: String = p.description_text().chars().take(1500).collect();
        let requirements = p.requirements();
        let url = p.profile_url.clone();
        let remote = p.id;
        shell
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .child(div().flex_1().min_w_0().pt(px(2.)).child(heading(p.name.clone())))
                    .child(div().flex_none().child(close)),
            )
            .children(gallery)
            .child(muted(
                [
                    p.submitter.as_ref().map(|s| format!("by {}", s.name)),
                    p.version.clone().map(|v| format!("v{}", v.trim_start_matches(['v', 'V']))),
                    Some(format!("{} downloads", format_count(p.downloads))),
                    Some(format!("♥ {}", format_count(p.likes))),
                    p.updated().map(|t| format!("updated {}", kombini_core::freshness::format_date(t))),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · "),
            ))
            .children(self.discover_age_note(p.updated()))
            .when(!installed.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .p(px(10.))
                        .rounded(px(RADIUS))
                        .bg(ca(ACCENT, 0.10))
                        .children(installed.iter().enumerate().map(|(i, m)| {
                            let on = enabled.contains(&m.id);
                            let id = m.id.clone();
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(dot(if on { ACCENT } else { FAINT }))
                                .child(div().flex_1().min_w_0().text_sm().truncate().child(
                                    if installed.len() > 1 || files.len() > 1 {
                                        format!(
                                            "Installed: {}{}",
                                            crate::page_files::file_label(m),
                                            if on { "" } else { " (off)" }
                                        )
                                    } else {
                                        format!("Installed{}", if on { "" } else { " (off)" })
                                    },
                                ))
                                .child(
                                    Button::new(("gb-show-installed", i))
                                        .small()
                                        .ghost()
                                        .text_label("Show")
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.show_mod(id.clone(), window, cx);
                                        })),
                                )
                        })),
                )
            })
            .child({
                let busy = self.is_pending(&format!("gb:{remote}"));
                let usable = files.iter().filter(|f| !f.flagged()).count();
                let single = files.len() == 1 && usable == 1;
                let only = files.first().map(|f| f.id);
                let have_all = !installed.is_empty()
                    && files
                        .iter()
                        .all(|f| installed.iter().any(|m| m.file_id == Some(f.id)));
                let label = if single {
                    if have_all { "Reinstall".to_owned() } else { "Install".to_owned() }
                } else if installed.is_empty() {
                    format!("Install ({} options)", files.len())
                } else {
                    format!("Install other files ({} options)", files.len())
                };
                let profile = p.clone();
                Button::new("gb-install-main")
                    .primary()
                    .w_full()
                    .icon(IconName::ArrowDown)
                    .text_label(label)
                    .loading(busy)
                    .disabled(busy || usable == 0)
                    .on_click(cx.listener(move |this, _, _, cx| match (single, only) {
                        (true, Some(fid)) => {
                            let switch = this.settings.switch_variants;
                            this.queue_gamebanana_files(remote, vec![fid], switch, cx)
                        }
                        _ => this.open_file_picker(remote, Some(profile.clone()), cx),
                    }))
            })
            .when(files.len() > 1, |d| {
                d.child(section_title(format!("Files ({})", files.len())))
                    .child(
                        card().px(px(4.)).py(px(4.)).flex().flex_col().children(
                            files.iter().map(|f| {
                                let have = installed.iter().find(|m| m.file_id == Some(f.id));
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .px(px(8.))
                                    .py(px(5.))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .flex_col()
                                            .child(div().text_sm().truncate().child(f.file_name.clone()))
                                            .children(
                                                f.description
                                                    .clone()
                                                    .filter(|d| !d.trim().is_empty())
                                                    .map(|d| div().text_xs().text_color(c(MUTED)).truncate().child(d)),
                                            ),
                                    )
                                    .when(f.flagged(), |d| d.child(chip("Flagged", CORAL)))
                                    .when_some(have, |d, m| {
                                        d.child(chip(
                                            "Installed",
                                            if enabled.contains(&m.id) { ACCENT } else { MUTED },
                                        ))
                                    })
                                    .child(faint(format_bytes(f.size)))
                            }),
                        ),
                    )
            })
            .when(archived > 0 && !files.iter().any(|f| f.archived), |d| {
                d.child(faint(format!(
                    "{archived} older archived file(s) are hidden; they are never installed automatically."
                )))
            })
            .when(!requirements.is_empty(), |d| {
                d.child(section_title("Requirements")).children(requirements.into_iter().map(|r| {
                    let have = r
                        .submission_id
                        .is_some_and(|sid| self.lib.mods.values().any(|m| m.remote_id == Some(sid)));
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(dot(if have { ACCENT } else { AMBER }))
                        .child(div().flex_1().text_sm().child(r.name.clone()))
                        .when_some(r.submission_id.filter(|_| !have), |d, sid| {
                            d.child(
                                Button::new(("gb-req", sid as usize))
                                    .small()
                                    .text_label("Install")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.queue_gamebanana_install(sid, None, cx)
                                    })),
                            )
                        })
                }))
            })
            .when(!desc.is_empty(), |d| {
                d.child(section_title("About"))
                    .child(div().text_sm().text_color(c(MUTED)).child(desc))
            })
            .when_some(url, |d, url| {
                d.child(
                    Button::new("gb-open")
                        .small()
                        .ghost()
                        .text_label("Open on GameBanana")
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                )
            })
            .into_any_element()
    }
}

trait IntoDiv {
    fn into_div(self) -> Div;
}

impl IntoDiv for AnyElement {
    fn into_div(self) -> Div {
        div().child(self)
    }
}
