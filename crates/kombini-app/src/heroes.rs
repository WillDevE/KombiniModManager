//! Hero Locker: mods grouped by hero, one active skin per hero, swap prompt
//! and shuffle.
//!
//! A skin is a mod GameBanana files under a hero (or a local mod the user
//! assigned to one); see `Mod::is_skin`. Other hero mods (sounds, icons) are
//! listed separately and combine freely.

use crate::app::AppView;
use crate::icons::Lucide;
use crate::smooth_scroll::Smooth as _;
use crate::theme::*;
use crate::widgets::*;
use gpui_kit::component::button::*;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable, Icon, IconName, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kombini_core::{Mod, ModId};
use std::collections::BTreeMap;

const SKIN_CARD_W: f32 = 200.;
/// The game's hero select has nine heroes per row.
const GRID_COLUMNS: f32 = 9.;
const GRID_PAD: f32 = 20.;
/// Hero card portraits are 280 x 380.
const PORTRAIT_ASPECT: f32 = 380. / 280.;
/// Height of the name under a portrait.
const TILE_NAME_H: f32 = 20.;
/// Window height taken by the title bar, tabs, status bar, grid header,
/// legend and padding.
const GRID_CHROME_H: f32 = 285.;
/// A tile's top and bottom border.
const TILE_BORDER_H: f32 = 4.;

/// Amber badge: `n` skins are on for one hero.
fn stack_badge(n: usize) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(2.))
        .px(px(4.))
        .h(px(18.))
        .rounded(px(9.))
        .bg(c(AMBER))
        .text_color(c(WINDOW))
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .child(
            Icon::new(Lucide::Layers)
                .size(px(11.))
                .text_color(c(WINDOW)),
        )
        .child(format!("{n}"))
}

/// One hero's mods.
#[derive(Clone)]
pub struct HeroEntry {
    pub hero: String,
    pub skins: Vec<ModId>,
    pub others: Vec<ModId>,
    pub active: Vec<ModId>,
}

/// Small xorshift RNG seeded from the clock; good enough for picking skins.
pub struct Dice(u64);

impl Dice {
    pub fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Self(seed | 1)
    }

    pub fn below(&mut self, n: usize) -> usize {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        if n == 0 { 0 } else { (x % n as u64) as usize }
    }
}

/// Groups `mods` by hero. `enabled` tells which are on.
pub fn hero_entries<'a>(
    mods: impl IntoIterator<Item = &'a Mod>,
    enabled: impl Fn(&ModId) -> bool,
) -> Vec<HeroEntry> {
    let mut by_hero: BTreeMap<String, HeroEntry> = BTreeMap::new();
    let mut list: Vec<&Mod> = mods.into_iter().collect();
    list.sort_by_key(|m| m.title.to_lowercase());
    for m in list {
        let Some(hero) = m.primary_hero() else {
            continue;
        };
        let e = by_hero
            .entry(hero.to_lowercase())
            .or_insert_with(|| HeroEntry {
                hero: hero.to_owned(),
                skins: Vec::new(),
                others: Vec::new(),
                active: Vec::new(),
            });
        if m.is_skin() {
            e.skins.push(m.id.clone());
            if enabled(&m.id) {
                e.active.push(m.id.clone());
            }
        } else {
            e.others.push(m.id.clone());
        }
    }
    by_hero.into_values().collect()
}

/// Enable/disable sets that give every hero in `entries` with two or more
/// options a random skin (never the same single active one when avoidable).
pub fn shuffle_plan(entries: &[HeroEntry], dice: &mut Dice) -> (Vec<ModId>, Vec<ModId>) {
    let (mut on, mut off) = (Vec::new(), Vec::new());
    for e in entries.iter().filter(|e| e.skins.len() > 1) {
        let mut pool: Vec<&ModId> = e.skins.iter().collect();
        if let [only] = e.active.as_slice() {
            pool.retain(|id| *id != only);
        }
        let pick = pool[dice.below(pool.len())].clone();
        off.extend(e.active.iter().filter(|id| **id != pick).cloned());
        if !e.active.contains(&pick) {
            on.push(pick);
        }
    }
    (on, off)
}

impl AppView {
    pub fn hero_entries(&self) -> Vec<HeroEntry> {
        let profile = self.lib.profile();
        hero_entries(self.lib.mods.values(), |id| profile.is_enabled(id))
    }

    fn hero_entry(&self, hero: &str) -> Option<HeroEntry> {
        self.hero_entries()
            .into_iter()
            .find(|e| e.hero.eq_ignore_ascii_case(hero))
    }

    /// Other enabled skins for `id`'s hero (the swap prompt's subject).
    pub fn other_enabled_skins(&self, id: &ModId) -> Vec<ModId> {
        let Some(m) = self.lib.mods.get(id) else {
            return Vec::new();
        };
        if !m.is_skin() {
            return Vec::new();
        }
        let Some(hero) = m.primary_hero() else {
            return Vec::new();
        };
        self.hero_entry(hero)
            .map(|e| e.active.into_iter().filter(|a| a != id).collect())
            .unwrap_or_default()
    }

    /// Makes `id` the only active skin of its hero.
    pub fn use_skin(&mut self, id: ModId, cx: &mut Context<Self>) {
        let off = self.other_enabled_skins(&id);
        let title = self
            .lib
            .mods
            .get(&id)
            .map(|m| m.title.clone())
            .unwrap_or_default();
        self.apply_enabled(vec![id], off, format!("Switched to {title}"), cx);
    }

    /// Turns off every skin of `hero` (the game's own look).
    pub fn default_look(&mut self, hero: String, cx: &mut Context<Self>) {
        let Some(e) = self.hero_entry(&hero) else {
            return;
        };
        self.apply_enabled(Vec::new(), e.active, format!("{hero}: default look"), cx);
    }

    /// Random skin for one hero, or for every hero when `hero` is `None`.
    pub fn shuffle_skins(&mut self, hero: Option<String>, cx: &mut Context<Self>) {
        let entries: Vec<HeroEntry> = self
            .hero_entries()
            .into_iter()
            .filter(|e| hero.as_ref().is_none_or(|h| e.hero.eq_ignore_ascii_case(h)))
            .collect();
        let (on, off) = shuffle_plan(&entries, &mut Dice::new());
        if on.is_empty() && off.is_empty() {
            self.toast(gpui_kit::component::notification::Notification::info(
                "Shuffling needs at least two skins for a hero.",
            ));
            return;
        }
        let summary = match &hero {
            Some(h) => format!("Shuffled {h}'s skin"),
            None => format!("Shuffled skins for {} hero(es)", on.len().max(1)),
        };
        self.apply_enabled(on, off, summary, cx);
    }

    /// One transaction enabling `on` and disabling `off`.
    pub fn apply_enabled(
        &mut self,
        on: Vec<ModId>,
        off: Vec<ModId>,
        summary: String,
        cx: &mut Context<Self>,
    ) {
        if on.is_empty() && off.is_empty() {
            return;
        }
        let key = match on.as_slice() {
            [one] => format!("toggle:{one}"),
            _ => "toggle:bulk".to_owned(),
        };
        self.run(
            key,
            summary.clone(),
            cx,
            move |e, _| e.set_enabled_many(&on, &off, &summary),
            |this, report, _| {
                this.success(report.summary);
                for w in report.warnings.iter().take(2) {
                    this.toast(gpui_kit::component::notification::Notification::warning(
                        w.clone(),
                    ));
                }
            },
        );
    }

    /// Loads the game's hero roster (names, order, portraits) once.
    fn ensure_roster(&mut self, cx: &mut Context<Self>) {
        if self.roster.is_some() || self.roster_loading || self.engine.is_none() {
            return;
        }
        self.roster_loading = true;
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let Some(engine) = engine else { return };
            let result = cx
                .background_executor()
                .spawn(async move { engine.hero_roster() })
                .await;
            this.update(cx, |this, cx| {
                this.roster_loading = false;
                match result {
                    Ok(r) => this.roster = Some(std::sync::Arc::new(r)),
                    Err(e) => {
                        tracing::warn!(error = %e, "hero roster unavailable");
                        this.roster = Some(std::sync::Arc::new(Default::default()));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The Heroes tab: the hero-select grid, or one hero's skins.
    pub fn render_heroes(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_roster(cx);
        let entries = self.hero_entries();
        let have_roster = self.roster.as_ref().is_some_and(|r| !r.heroes.is_empty());
        if self.locker_hero.is_none() && (have_roster || self.roster_loading) {
            let size = window.viewport_size();
            return self.render_hero_grid(
                entries,
                f32::from(size.width),
                f32::from(size.height),
                cx,
            );
        }
        if entries.is_empty() && self.locker_hero.is_none() {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .child(Icon::new(Lucide::Swords).size(px(28.)).text_color(c(FAINT)))
                .child(muted("No hero mods yet. Skins you install from Discover show up here, grouped by hero."))
                .into_any_element();
        }
        let selected = self
            .locker_hero
            .clone()
            .or_else(|| entries.first().map(|e| e.hero.clone()))
            .unwrap_or_default();
        let busy = self.launch.phase.busy() || self.is_pending("toggle:bulk");
        let list = div()
            .id("hero-list")
            .w(px(240.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .smooth(&self.smooth.get("hero-list"), cx)
            .border_r_1()
            .border_color(c(BORDER))
            .bg(c(PANEL))
            .p(px(8.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(8.))
                    .pb(px(6.))
                    .when(have_roster, |d| {
                        d.child(
                            Button::new("all-heroes")
                                .small()
                                .ghost()
                                .icon(IconName::ChevronLeft)
                                .text_label("All heroes")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.locker_hero = None;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .when(!have_roster, |d| d.child(section_title("Heroes"))),
                    )
                    .child(
                        Button::new("shuffle-all")
                            .small()
                            .ghost()
                            .icon(Lucide::Shuffle)
                            .text_label("Shuffle all")
                            .tooltip("A random skin for every hero with two or more")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.shuffle_skins(None, cx))),
                    ),
            )
            .children(entries.iter().enumerate().map(|(i, e)| {
                let active = e.hero.eq_ignore_ascii_case(&selected);
                let hero = e.hero.clone();
                let sub = match e.active.as_slice() {
                    [] if e.skins.is_empty() => format!("{} other mod(s)", e.others.len()),
                    [] => "Default look".to_owned(),
                    [one] => self
                        .lib
                        .mods
                        .get(one)
                        .map(|m| m.title.clone())
                        .unwrap_or_default(),
                    many => format!("{} skins on", many.len()),
                };
                let stacked = e.active.len() > 1;
                div()
                    .id(("hero-row", i))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(active, |d| d.bg(c(RAISED)))
                    .hover(|s| s.bg(c(HOVER)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_sm().truncate().child(e.hero.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .truncate()
                                    .text_color(c(if stacked { AMBER } else { FAINT }))
                                    .child(sub),
                            ),
                    )
                    .child(faint(format!("{}", e.skins.len() + e.others.len())))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.locker_hero = Some(hero.clone());
                        cx.notify();
                    }))
            }));
        // A hero picked in the grid may have no mods yet.
        let entry = entries
            .iter()
            .find(|e| e.hero.eq_ignore_ascii_case(&selected))
            .cloned()
            .unwrap_or_else(|| HeroEntry {
                hero: selected.clone(),
                skins: Vec::new(),
                others: Vec::new(),
                active: Vec::new(),
            });
        let detail = self.render_hero_detail(entry, busy, cx);
        div()
            .size_full()
            .flex()
            .child(list)
            .child(
                div()
                    .id("hero-detail")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .smooth(&self.smooth.get("hero-detail"), cx)
                    .p(px(16.))
                    .child(detail),
            )
            .into_any_element()
    }

    /// The hero-select grid: the game's roster in its own order, each tile
    /// showing how many skins the hero has and whether more than one is on.
    fn render_hero_grid(
        &mut self,
        entries: Vec<HeroEntry>,
        width: f32,
        height: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let busy = self.launch.phase.busy() || self.is_pending("toggle:bulk");
        let roster = self.roster.clone().unwrap_or_default();
        let find = |name: &str| {
            entries
                .iter()
                .find(|e| e.hero.eq_ignore_ascii_case(name))
                .cloned()
        };
        // Roster heroes in picker order, then heroes with mods the game did not list.
        let mut tiles: Vec<(String, Option<std::path::PathBuf>, Option<HeroEntry>)> = roster
            .heroes
            .iter()
            .map(|h| (h.name.clone(), h.portrait.clone(), find(&h.name)))
            .collect();
        for e in &entries {
            if !tiles
                .iter()
                .any(|(n, _, _)| n.eq_ignore_ascii_case(&e.hero))
            {
                tiles.push((e.hero.clone(), None, Some(e.clone())));
            }
        }
        let gap = 8.;
        // Fit the whole roster on screen: the tile size follows both the
        // window width (nine columns) and its height (all rows visible).
        let rows = (tiles.len() as f32 / GRID_COLUMNS).ceil().max(1.);
        let avail_w = (width - 2. * GRID_PAD).max(300.);
        let avail_h = (height - GRID_CHROME_H).max(200.);
        let by_width = (avail_w - gap * (GRID_COLUMNS - 1.)) / GRID_COLUMNS;
        let by_height =
            ((avail_h - gap * (rows - 1.)) / rows - TILE_NAME_H - TILE_BORDER_H) / PORTRAIT_ASPECT;
        let tile_w = by_width.min(by_height).clamp(48., 120.).floor();
        let img_h = (tile_w * PORTRAIT_ASPECT).round();
        let grid_w = tile_w * GRID_COLUMNS + gap * (GRID_COLUMNS - 1.);
        let with_mods = tiles.iter().filter(|(_, _, e)| e.is_some()).count();
        let header = div()
            .w(px(grid_w))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(heading("Heroes"))
                    .child(muted(format!("{with_mods} with mods"))),
            )
            .child(
                Button::new("shuffle-all")
                    .small()
                    .ghost()
                    .icon(Lucide::Shuffle)
                    .text_label("Shuffle all")
                    .tooltip("A random skin for every hero with two or more")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.shuffle_skins(None, cx))),
            );
        let legend = div()
            .w(px(grid_w))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(14.))
            .text_xs()
            .text_color(c(FAINT))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .size(px(10.))
                            .rounded(px(3.))
                            .border_2()
                            .border_color(c(ACCENT)),
                    )
                    .child("A skin is on"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(stack_badge(2))
                    .child("More than one skin is on; only one shows in game. Open the hero to keep one."),
            );
        let tiles: Vec<AnyElement> = tiles
            .into_iter()
            .enumerate()
            .map(|(i, (name, portrait, entry))| {
                let skins = entry.as_ref().map_or(0, |e| e.skins.len());
                let mods = entry.as_ref().map_or(0, |e| e.skins.len() + e.others.len());
                let active = entry.as_ref().map_or(0, |e| e.active.len());
                let open = name.clone();
                let initial = name.chars().next().map(String::from).unwrap_or_default();
                let picture = match portrait {
                    Some(path) => img(path)
                        .size_full()
                        .object_fit(ObjectFit::Cover)
                        .when(mods == 0, |i| i.opacity(0.4))
                        .into_any_element(),
                    None => div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_xl()
                        .text_color(c(FAINT))
                        .child(initial)
                        .into_any_element(),
                };
                div()
                    .id(("hero-tile", i))
                    .w(px(tile_w))
                    .flex()
                    .flex_col()
                    .rounded(px(8.))
                    .overflow_hidden()
                    .bg(c(PANEL))
                    .border_2()
                    .border_color(c(if active > 0 { ACCENT } else { BORDER }))
                    .cursor_pointer()
                    .hover(|s| s.border_color(c(TEXT)))
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .h(px(img_h))
                            .bg(c(RAISED))
                            .child(picture)
                            .when(active > 1, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top(px(4.))
                                        .right(px(4.))
                                        .child(stack_badge(active)),
                                )
                            })
                            .when(skins > 0 && active <= 1, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top(px(4.))
                                        .right(px(4.))
                                        .px(px(5.))
                                        .rounded(px(6.))
                                        .bg(ca(WINDOW, 0.75))
                                        .text_xs()
                                        .text_color(c(TEXT))
                                        .child(format!("{skins}")),
                                )
                            }),
                    )
                    .child(
                        div()
                            .h(px(TILE_NAME_H))
                            .px(px(4.))
                            .pt(px(2.))
                            .text_xs()
                            .text_center()
                            .truncate()
                            .text_color(c(if mods > 0 { TEXT } else { FAINT }))
                            .child(name),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.locker_hero = Some(open.clone());
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        div()
            .id("hero-grid")
            .size_full()
            .overflow_y_scroll()
            .smooth(&self.smooth.get("hero-grid"), cx)
            .p(px(GRID_PAD))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(12.))
            .child(header)
            .child(legend)
            .when(self.roster_loading && roster.heroes.is_empty(), |d| {
                d.child(muted("Reading the hero list from the game…"))
            })
            .child(
                div()
                    .w(px(grid_w))
                    .flex()
                    .flex_wrap()
                    .gap(px(gap))
                    .children(tiles),
            )
            .into_any_element()
    }

    fn render_hero_detail(
        &mut self,
        e: HeroEntry,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hero = e.hero.clone();
        let hero2 = e.hero.clone();
        let mut skins: Vec<AnyElement> = Vec::new();
        for (i, id) in e.skins.iter().enumerate() {
            skins.push(self.render_skin_card(
                id,
                i,
                e.active.contains(id),
                e.active.len() > 1,
                busy,
                cx,
            ));
        }
        let others: Vec<AnyElement> = e
            .others
            .iter()
            .enumerate()
            .filter_map(|(i, id)| {
                let m = self.lib.mods.get(id)?;
                let on = self.lib.profile().is_enabled(id);
                let toggle_id = id.clone();
                let show_id = id.clone();
                Some(
                    card()
                        .px(px(12.))
                        .py(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().text_sm().truncate().child(m.title.clone()))
                                .children(m.category.clone().map(faint)),
                        )
                        .child(
                            Button::new(("hero-other-show", i))
                                .small()
                                .ghost()
                                .text_label("Details")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.selected = Some(show_id.clone());
                                    this.set_tab(crate::app::Tab::Mods, cx);
                                })),
                        )
                        .child(
                            Switch::new(("hero-other", i))
                                .checked(on)
                                .disabled(busy)
                                .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                    this.set_enabled(vec![toggle_id.clone()], *checked, cx);
                                })),
                        )
                        .into_any_element(),
                )
            })
            .collect();
        let stacked = e.active.len() > 1;
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(heading(e.hero.clone()))
                            .child(muted(match e.skins.len() {
                                0 => "No skins installed.".to_owned(),
                                n => format!("{n} skin(s) · {} on", e.active.len()),
                            })),
                    )
                    .when(e.skins.len() > 1, |d| {
                        d.child(
                            Button::new("shuffle-hero")
                                .small()
                                .ghost()
                                .icon(Lucide::Shuffle)
                                .text_label("Shuffle")
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.shuffle_skins(Some(hero.clone()), cx)
                                })),
                        )
                    })
                    .when(!e.skins.is_empty(), |d| {
                        d.child(
                            Button::new("default-look")
                                .small()
                                .text_label("Default look")
                                .tooltip("Turn off this hero's skins")
                                .disabled(busy || e.active.is_empty())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.default_look(hero2.clone(), cx)
                                })),
                        )
                    }),
            )
            .when(stacked, |d| {
                d.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .p(px(10.))
                        .rounded(px(RADIUS))
                        .bg(ca(AMBER, 0.10))
                        .text_xs()
                        .text_color(c(AMBER))
                        .child("More than one skin is on for this hero. The one highest in the load order wins; the others are loaded for nothing. Pick the one to keep with Keep only this."),
                )
            })
            .when(!skins.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(12.))
                        .children(skins),
                )
            })
            .when(!others.is_empty(), |d| {
                d.child(section_title(format!("Other {} mods", e.hero)))
                    .child(div().flex().flex_col().gap(px(6.)).children(others))
            })
            .into_any_element()
    }

    fn render_skin_card(
        &mut self,
        id: &ModId,
        i: usize,
        active: bool,
        stacked: bool,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(m) = self.lib.mods.get(id).cloned() else {
            return div().into_any_element();
        };
        let thumb = m.thumbnail_url.as_ref().and_then(|u| self.thumb(u, cx));
        let thumb_h = (SKIN_CARD_W * 0.5625).round();
        let chip_el = self.issue_chip(id);
        let pending = self.is_pending(&format!("toggle:{id}"));
        let use_id = id.clone();
        let show_id = id.clone();
        div()
            .id(("skin-card", i))
            .w(px(SKIN_CARD_W))
            .flex()
            .flex_col()
            .rounded(px(10.))
            .overflow_hidden()
            .bg(c(PANEL))
            .border_1()
            .border_color(if active { c(ACCENT) } else { c(BORDER) })
            .child(
                div()
                    .relative()
                    .w(px(SKIN_CARD_W))
                    .h(px(thumb_h))
                    .bg(c(RAISED))
                    .when_some(thumb, |d, path| {
                        d.child(img(path).size_full().object_fit(ObjectFit::Cover))
                    })
                    // Status sits on the picture so every card has the same layout.
                    .children(chip_el.map(|chip| {
                        div()
                            .absolute()
                            .top(px(6.))
                            .left(px(6.))
                            .rounded(px(10.))
                            .bg(ca(WINDOW, 0.85))
                            .child(chip)
                    })),
            )
            .child(
                div()
                    .p(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(m.title.clone()),
                    )
                    .child(
                        faint(m.author.clone().unwrap_or_else(|| "Unknown author".into()))
                            .truncate(),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(div().flex_1())
                            .child(
                                Button::new(("skin-details", i))
                                    .small()
                                    .ghost()
                                    .text_label("Details")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.selected = Some(show_id.clone());
                                        this.set_tab(crate::app::Tab::Mods, cx);
                                    })),
                            )
                            .child(if active && stacked {
                                // Several skins are on: this one stays, the rest go off.
                                Button::new(("skin-keep", i))
                                    .small()
                                    .primary()
                                    .text_label("Keep only this")
                                    .disabled(busy)
                                    .on_click(cx.listener({
                                        let keep = use_id.clone();
                                        move |this, _, _, cx| this.use_skin(keep.clone(), cx)
                                    }))
                                    .into_any_element()
                            } else if active {
                                chip("Active", ACCENT).into_any_element()
                            } else {
                                Button::new(("skin-use", i))
                                    .small()
                                    .primary()
                                    .text_label("Use")
                                    .loading(pending)
                                    .disabled(busy || pending)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.use_skin(use_id.clone(), cx)
                                    }))
                                    .into_any_element()
                            }),
                    ),
            )
            .into_any_element()
    }

    /// Swap prompt shown when enabling a second skin for a hero.
    pub fn render_hero_swap(
        &mut self,
        id: ModId,
        others: Vec<ModId>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let title = |this: &Self, id: &ModId| {
            this.lib
                .mods
                .get(id)
                .map(|m| m.title.clone())
                .unwrap_or_default()
        };
        let new_title = title(self, &id);
        let old: Vec<String> = others.iter().map(|o| title(self, o)).collect();
        let hero = self
            .lib
            .mods
            .get(&id)
            .and_then(|m| m.primary_hero().map(str::to_owned))
            .unwrap_or_default();
        let (swap_id, keep_id) = (id.clone(), id);
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(muted(format!(
                "{} is already using {}. Only one skin per hero shows in game; the one highest in the load order wins.",
                hero,
                old.join(", ")
            )))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        Button::new("swap-cancel")
                            .ghost()
                            .text_label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.modal = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("swap-keep")
                            .text_label("Keep both")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.modal = None;
                                this.set_enabled_now(vec![keep_id.clone()], true, cx);
                            })),
                    )
                    .child(
                        Button::new("swap-ok")
                            .primary()
                            .text_label(format!("Swap to {new_title}"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.modal = None;
                                this.use_skin(swap_id.clone(), cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Dice, hero_entries, shuffle_plan};
    use kombini_core::{Mod, ModId, ModSource};

    fn skin(id: &str, hero: &str) -> Mod {
        let mut m = Mod::new(ModId::new(id), id, ModSource::Gamebanana, 0);
        m.category = Some(hero.to_owned());
        m
    }

    #[test]
    fn groups_by_hero_and_splits_skins() {
        let mut sound = Mod::new(ModId::new("snd"), "Haze voice", ModSource::Gamebanana, 0);
        sound.category = Some("Voice".into());
        sound.heroes = vec!["Haze".into()];
        let mods = [
            skin("a", "Haze"),
            skin("b", "Haze"),
            skin("c", "Lady Geist"),
            sound,
        ];
        let on = |id: &ModId| id.as_str() == "a";
        let entries = hero_entries(mods.iter(), on);
        assert_eq!(entries.len(), 2);
        let haze = entries.iter().find(|e| e.hero == "Haze").unwrap();
        assert_eq!(haze.skins.len(), 2);
        assert_eq!(haze.others.len(), 1);
        assert_eq!(haze.active, vec![ModId::new("a")]);
    }

    #[test]
    fn shuffle_changes_the_active_skin() {
        let mods = [skin("a", "Haze"), skin("b", "Haze"), skin("c", "Lash")];
        let entries = hero_entries(mods.iter(), |id| id.as_str() == "a");
        for _ in 0..20 {
            let (on, off) = shuffle_plan(&entries, &mut Dice::new());
            assert_eq!(on, vec![ModId::new("b")]);
            assert_eq!(off, vec![ModId::new("a")]);
        }
    }
}
