//! Hero Locker: mods grouped by hero, one active skin per hero, swap prompt
//! and shuffle.
//!
//! A skin is a mod GameBanana files under a hero (or under "Skins" with a
//! hero found in its files, or one the user assigned); see `Mod::is_skin`.
//! The skin files of one GameBanana page form one skin card (its variants).
//! A page file with no hero of its own (a voice pack next to the model)
//! belongs to the page's hero. Other hero mods (sounds, icons) are listed
//! separately and combine freely.

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
/// Height of a skin card's text panel (title, author, files, buttons).
const SKIN_INFO_H: f32 = 104.;
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

/// One skin: the skin files of one GameBanana page (or one local mod).
#[derive(Clone)]
pub struct SkinCard {
    /// The GameBanana page, if the files come from one.
    pub page: Option<u64>,
    /// The card's files, first file of the page first.
    pub ids: Vec<ModId>,
    /// The ones that are on.
    pub on: Vec<ModId>,
}

impl SkinCard {
    /// The file that stands for the card: the first one on, else the first.
    pub fn primary(&self) -> &ModId {
        self.on.first().unwrap_or(&self.ids[0])
    }

    pub fn is_on(&self) -> bool {
        !self.on.is_empty()
    }
}

/// One hero's mods.
#[derive(Clone)]
pub struct HeroEntry {
    pub hero: String,
    pub skins: Vec<SkinCard>,
    pub others: Vec<ModId>,
}

impl HeroEntry {
    pub fn empty(hero: String) -> Self {
        Self {
            hero,
            skins: Vec::new(),
            others: Vec::new(),
        }
    }

    /// Skin cards that are on (more than one means they stack).
    pub fn active_cards(&self) -> Vec<&SkinCard> {
        self.skins.iter().filter(|c| c.is_on()).collect()
    }

    /// Every skin file that is on.
    pub fn active_files(&self) -> Vec<ModId> {
        self.skins
            .iter()
            .flat_map(|c| c.on.iter().cloned())
            .collect()
    }

    pub fn card_of(&self, id: &ModId) -> Option<&SkinCard> {
        self.skins.iter().find(|c| c.ids.contains(id))
    }
}

/// The hero each mod belongs to: its own (override, category, files), or
/// for a page file with none, the hero of its GameBanana page.
pub fn resolve_heroes<'a>(mods: impl IntoIterator<Item = &'a Mod>) -> BTreeMap<ModId, String> {
    let list: Vec<&Mod> = mods.into_iter().collect();
    let mut page_hero: BTreeMap<u64, &str> = BTreeMap::new();
    let mut own: Vec<&Mod> = list.clone();
    own.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
    for m in &own {
        if let (Some(r), Some(h)) = (m.remote_id, m.primary_hero()) {
            page_hero.entry(r).or_insert(h);
        }
    }
    list.iter()
        .filter_map(|m| {
            let hero = m
                .primary_hero()
                .or_else(|| m.remote_id.and_then(|r| page_hero.get(&r).copied()))?;
            Some((m.id.clone(), hero.to_owned()))
        })
        .collect()
}

/// Skins GameBanana filed under the general "Skins" category whose hero is
/// unknown: they need the user to pick one.
pub fn unassigned_skins<'a>(
    mods: impl IntoIterator<Item = &'a Mod>,
    heroes: &BTreeMap<ModId, String>,
) -> Vec<ModId> {
    let mut out: Vec<&Mod> = mods
        .into_iter()
        .filter(|m| {
            !heroes.contains_key(&m.id)
                && m.category
                    .as_deref()
                    .is_some_and(|c| c.trim().eq_ignore_ascii_case("skins"))
        })
        .collect();
    out.sort_by_key(|m| m.title.to_lowercase());
    out.into_iter().map(|m| m.id.clone()).collect()
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
    let mut list: Vec<&Mod> = mods.into_iter().collect();
    let heroes = resolve_heroes(list.iter().copied());
    list.sort_by(|a, b| {
        (a.title.to_lowercase(), a.id.as_str()).cmp(&(b.title.to_lowercase(), b.id.as_str()))
    });
    let mut by_hero: BTreeMap<String, HeroEntry> = BTreeMap::new();
    for m in list {
        let Some(hero) = heroes.get(&m.id) else {
            continue;
        };
        let e = by_hero
            .entry(hero.to_lowercase())
            .or_insert_with(|| HeroEntry::empty(hero.clone()));
        // Only files with a hero of their own are skins; a voice pack that
        // borrows the page's hero is an add-on.
        if m.primary_hero().is_some() && m.is_skin() {
            let card = m
                .remote_id
                .and_then(|r| e.skins.iter().position(|c| c.page == Some(r)));
            let i = card.unwrap_or_else(|| {
                e.skins.push(SkinCard {
                    page: m.remote_id,
                    ids: Vec::new(),
                    on: Vec::new(),
                });
                e.skins.len() - 1
            });
            e.skins[i].ids.push(m.id.clone());
            if enabled(&m.id) {
                e.skins[i].on.push(m.id.clone());
            }
        } else {
            e.others.push(m.id.clone());
        }
    }
    for e in by_hero.values_mut() {
        for c in &mut e.skins {
            c.ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
            c.on.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        }
    }
    by_hero.into_values().collect()
}

/// Enable/disable sets that give every hero in `entries` with two or more
/// skins a random one (never the same single active one when avoidable).
pub fn shuffle_plan(entries: &[HeroEntry], dice: &mut Dice) -> (Vec<ModId>, Vec<ModId>) {
    let (mut on, mut off) = (Vec::new(), Vec::new());
    for e in entries.iter().filter(|e| e.skins.len() > 1) {
        let mut pool: Vec<&SkinCard> = e.skins.iter().collect();
        if let [only] = e.active_cards().as_slice() {
            pool.retain(|c| c.ids != only.ids);
        }
        let pick = pool[dice.below(pool.len())];
        off.extend(
            e.active_files()
                .into_iter()
                .filter(|id| !pick.ids.contains(id)),
        );
        if !pick.is_on() {
            on.push(pick.primary().clone());
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

    /// The hero `id` belongs to (see [`resolve_heroes`]).
    pub fn hero_of(&self, id: &ModId) -> Option<String> {
        self.heroes_by_mod.get(id).cloned()
    }

    /// Skins of `id`'s hero that are on, other than `id`'s own skin card
    /// (the swap prompt's subject). Empty when `id` is not a skin.
    pub fn other_enabled_skins(&self, id: &ModId) -> Vec<ModId> {
        let Some(e) = self.hero_of(id).and_then(|h| self.hero_entry(&h)) else {
            return Vec::new();
        };
        let Some(card) = e.card_of(id) else {
            return Vec::new();
        };
        e.active_files()
            .into_iter()
            .filter(|a| !card.ids.contains(a))
            .collect()
    }

    /// Lets the user pick the hero of a mod (skins GameBanana filed under
    /// the general "Skins" category, or a wrong guess).
    pub fn set_mod_hero(&mut self, id: ModId, hero: Option<String>, cx: &mut Context<Self>) {
        self.modal = None;
        self.run(
            "hero",
            "Setting the hero",
            cx,
            move |e, _| {
                e.update_mod(
                    &id,
                    kombini_engine::library::ModPatch {
                        hero_override: Some(hero),
                        ..Default::default()
                    },
                )
            },
            |_, _, _| {},
        );
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
        self.apply_enabled(
            Vec::new(),
            e.active_files(),
            format!("{hero}: default look"),
            cx,
        );
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
                let current = e.hero.eq_ignore_ascii_case(&selected);
                let hero = e.hero.clone();
                let active = e.active_cards();
                let sub = match active.as_slice() {
                    [] if e.skins.is_empty() => format!("{} other mod(s)", e.others.len()),
                    [] => "Default look".to_owned(),
                    [one] => self
                        .lib
                        .mods
                        .get(one.primary())
                        .map(|m| m.title.clone())
                        .unwrap_or_default(),
                    many => format!("{} skins on", many.len()),
                };
                let stacked = active.len() > 1;
                div()
                    .id(("hero-row", i))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(current, |d| d.bg(c(RAISED)))
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
            .unwrap_or_else(|| HeroEntry::empty(selected.clone()));
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
                let active = entry.as_ref().map_or(0, |e| e.active_cards().len());
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
            .children(self.render_unassigned(grid_w, cx))
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
        let stacked = e.active_cards().len() > 1;
        let mut skins: Vec<AnyElement> = Vec::new();
        for (i, card) in e.skins.iter().enumerate() {
            skins.push(self.render_skin_card(card, i, stacked, busy, cx));
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
                                .tooltip("Show in My Mods")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.show_mod(show_id.clone(), window, cx);
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
        let active_files = e.active_files();
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
                                n => format!("{n} skin(s) · {} on", e.active_cards().len()),
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
                                .disabled(busy || active_files.is_empty())
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
        card: &SkinCard,
        i: usize,
        stacked: bool,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = card.primary();
        let active = card.is_on();
        let Some(m) = self.lib.mods.get(id).cloned() else {
            return div().into_any_element();
        };
        let parts = (card.ids.len() > 1).then(|| {
            let ids = card.ids.clone();
            div()
                .id(("skin-parts", i))
                .flex()
                .items_center()
                .gap(px(2.))
                .text_xs()
                .text_color(c(MUTED))
                .cursor_pointer()
                .hover(|s| s.text_color(c(TEXT)))
                .child(format!("{} parts · {} on", card.ids.len(), card.on.len()))
                .child(Icon::new(IconName::ChevronRight).size(px(12.)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.modal = Some(crate::modal::Modal::SkinParts(ids.clone()));
                    cx.notify();
                }))
        });
        let thumb = m.thumbnail_url.as_ref().and_then(|u| self.thumb(u, cx));
        let card_h = (SKIN_CARD_W * 0.5625).round() + SKIN_INFO_H;
        let chip_el = self.issue_chip(id);
        let pending = self.is_pending(&format!("toggle:{id}"));
        let use_id = id.clone();
        let show_id = id.clone();
        picture_card(
            ("skin-card", i),
            SKIN_CARD_W,
            card_h,
            thumb,
            if active { ACCENT } else { BORDER },
        )
        // Status sits on the picture so every card has the same layout.
        .children(chip_el.map(|chip| {
            div()
                .absolute()
                .top(px(6.))
                .left(px(6.))
                .rounded(px(10.))
                .bg(ca(WINDOW, 0.85))
                .child(chip)
        }))
        .child(
            card_info()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .truncate()
                        .child(m.title.clone()),
                )
                .child(
                    faint(m.author.clone().unwrap_or_else(|| "Unknown author".into())).truncate(),
                )
                .children(parts)
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
                                .tooltip("Show in My Mods")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.show_mod(show_id.clone(), window, cx);
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

    /// Skins with no known hero (GameBanana's general "Skins" category and
    /// nothing in the files): each can be given a hero.
    fn render_unassigned(&mut self, width: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ids = unassigned_skins(self.lib.mods.values(), &self.heroes_by_mod);
        if ids.is_empty() {
            return None;
        }
        let rows: Vec<AnyElement> = ids
            .iter()
            .enumerate()
            .filter_map(|(i, id)| {
                let m = self.lib.mods.get(id)?;
                let pick = id.clone();
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
                                .text_sm()
                                .truncate()
                                .child(m.title.clone()),
                        )
                        .child(
                            Button::new(("unassigned-hero", i))
                                .small()
                                .text_label("Set hero…")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.modal = Some(crate::modal::Modal::PickHero(pick.clone()));
                                    cx.notify();
                                })),
                        )
                        .into_any_element(),
                )
            })
            .collect();
        Some(
            div()
                .w(px(width))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(section_title(format!("Skins without a hero ({})", rows.len())))
                .child(muted(
                    "GameBanana lists these under Skins without a hero, and none was found in their files.",
                ))
                .children(rows)
                .into_any_element(),
        )
    }

    /// Body of the hero picker: every hero, plus "No hero".
    pub fn render_pick_hero(&mut self, id: ModId, cx: &mut Context<Self>) -> AnyElement {
        let current = self.hero_of(&id);
        let mut names: Vec<String> = self
            .roster
            .as_ref()
            .map(|r| r.heroes.iter().map(|h| h.name.clone()).collect())
            .filter(|v: &Vec<String>| !v.is_empty())
            .unwrap_or_else(|| kombini_core::heroes::HeroTable::builtin().names().to_vec());
        names.sort_by_key(|n| n.to_lowercase());
        let clear_id = id.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(div().flex().flex_wrap().gap(px(6.)).children(
                names.into_iter().enumerate().map(|(i, name)| {
                    let id = id.clone();
                    let is = current
                        .as_deref()
                        .is_some_and(|c| c.eq_ignore_ascii_case(&name));
                    let hero = name.clone();
                    Button::new(("pick-hero", i))
                        .small()
                        .when(is, |b| b.primary())
                        .when(!is, |b| b.outline())
                        .text_label(name)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_mod_hero(id.clone(), Some(hero.clone()), cx)
                        }))
                }),
            ))
            .child(
                div().flex().justify_end().child(
                    Button::new("pick-hero-none")
                        .ghost()
                        .text_label("Use the detected hero")
                        .tooltip("Clear your choice")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_mod_hero(clear_id.clone(), None, cx)
                        })),
                ),
            )
            .into_any_element()
    }

    /// The files of one skin page, each with its own switch.
    pub fn render_skin_parts(&mut self, ids: Vec<ModId>, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.launch.phase.busy() || self.is_pending("toggle:bulk");
        let profile = self.lib.profile();
        let on: Vec<ModId> = ids
            .iter()
            .filter(|id| profile.is_enabled(id))
            .cloned()
            .collect();
        let rows: Vec<AnyElement> = ids
            .iter()
            .enumerate()
            .filter_map(|(i, id)| {
                let m = self.lib.mods.get(id)?;
                let enabled = on.contains(id);
                let pending = self.is_pending(&format!("toggle:{id}"));
                let toggle_id = id.clone();
                let show_id = id.clone();
                let label = crate::page_files::file_label(m);
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
                                .child(div().text_sm().truncate().child(label))
                                .child(faint(format_bytes(m.total_size()))),
                        )
                        .child(
                            Button::new(("part-show", i))
                                .small()
                                .ghost()
                                .text_label("Details")
                                .tooltip("Show in My Mods")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.modal = None;
                                    this.show_mod(show_id.clone(), window, cx);
                                })),
                        )
                        .child(
                            Switch::new(("part-toggle", i))
                                .checked(if pending { !enabled } else { enabled })
                                .disabled(busy || pending)
                                .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                    this.set_enabled_now(vec![toggle_id.clone()], *checked, cx);
                                })),
                        )
                        .into_any_element(),
                )
            })
            .collect();
        let all_on = ids.clone();
        let all_off = on.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(muted(
                "This GameBanana page has more than one file. Turn on the ones you want; \
                 pages often split a skin from its extras (sounds, effects, alternatives).",
            ))
            .child(div().flex().flex_col().gap(px(6.)).children(rows))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        Button::new("parts-off")
                            .ghost()
                            .text_label("All off")
                            .disabled(busy || all_off.is_empty())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_enabled_now(all_off.clone(), false, cx);
                            })),
                    )
                    .child(
                        Button::new("parts-on")
                            .primary()
                            .text_label("All on")
                            .disabled(busy || on.len() == ids.len())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_enabled(all_on.clone(), true, cx);
                            })),
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
        let hero = self.hero_of(&id).unwrap_or_default();
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
    use super::{Dice, hero_entries, resolve_heroes, shuffle_plan, unassigned_skins};
    use kombini_core::{Mod, ModId, ModSource};

    fn skin(id: &str, hero: &str) -> Mod {
        let mut m = Mod::new(ModId::new(id), id, ModSource::Gamebanana, 0);
        m.category = Some(hero.to_owned());
        m
    }

    fn page(id: &str, remote: u64, heroes: &[&str]) -> Mod {
        let mut m = Mod::new(
            ModId::new(id),
            "Aemeath as Apollo",
            ModSource::Gamebanana,
            0,
        );
        m.category = Some("Skins".into());
        m.remote_id = Some(remote);
        m.heroes = heroes.iter().map(|h| (*h).to_owned()).collect();
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
        assert_eq!(haze.active_files(), vec![ModId::new("a")]);
    }

    #[test]
    fn a_pages_skin_files_are_one_card_and_its_voice_pack_an_add_on() {
        let mods = [
            skin("eishin", "Apollo"),
            page("gb1", 1, &["Apollo"]),
            page("gb1_2", 1, &["Apollo"]),
            page("gb1_3", 1, &[]),
        ];
        let entries = hero_entries(mods.iter(), |id| id.as_str() == "gb1_2");
        let apollo = entries.iter().find(|e| e.hero == "Apollo").unwrap();
        assert_eq!(apollo.skins.len(), 2);
        let card = apollo.card_of(&ModId::new("gb1")).unwrap();
        assert_eq!(card.ids, vec![ModId::new("gb1"), ModId::new("gb1_2")]);
        assert_eq!(card.primary(), &ModId::new("gb1_2"));
        assert_eq!(apollo.others, vec![ModId::new("gb1_3")]);
        let heroes = resolve_heroes(mods.iter());
        assert_eq!(heroes[&ModId::new("gb1_3")], "Apollo");
    }

    #[test]
    fn general_skins_without_a_hero_are_unassigned() {
        let mods = [page("gb9", 9, &[]), skin("a", "Haze")];
        let heroes = resolve_heroes(mods.iter());
        assert_eq!(
            unassigned_skins(mods.iter(), &heroes),
            vec![ModId::new("gb9")]
        );
        let mut picked = page("gb9", 9, &[]);
        picked.hero_override = Some("Haze".into());
        let mods = [picked];
        let heroes = resolve_heroes(mods.iter());
        assert!(unassigned_skins(mods.iter(), &heroes).is_empty());
        let entries = hero_entries(mods.iter(), |_| false);
        assert_eq!(entries[0].skins.len(), 1);
    }

    #[test]
    fn shuffle_changes_the_active_skin() {
        let mods = [skin("a", "Haze"), skin("b", "Haze"), skin("c", "Lash")];
        let entries = hero_entries(mods.iter(), |id| id.as_str() == "a");
        let (on, off) = shuffle_plan(&entries, &mut Dice::new());
        assert_eq!(on, vec![ModId::new("b")]);
        assert_eq!(off, vec![ModId::new("a")]);
    }

    #[test]
    fn shuffle_turns_off_every_file_of_the_old_card() {
        let mods = [
            page("gb1", 1, &["Apollo"]),
            page("gb1_2", 1, &["Apollo"]),
            skin("eishin", "Apollo"),
        ];
        let entries = hero_entries(mods.iter(), |id| id.as_str().starts_with("gb1"));
        let (on, mut off) = shuffle_plan(&entries, &mut Dice::new());
        off.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        assert_eq!(on, vec![ModId::new("eishin")]);
        assert_eq!(off, vec![ModId::new("gb1"), ModId::new("gb1_2")]);
    }
}
