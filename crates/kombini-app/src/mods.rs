//! My Mods: toolbar, virtualized list (drag to reorder the enabled set) and
//! the details pane.

use crate::app::AppView;
use crate::icons::Lucide;
use crate::modal::Modal;
use crate::theme::*;
use crate::widgets::*;
use gpui_kit::component::Icon;
use gpui_kit::component::button::*;
use gpui_kit::component::input::Input;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kombini_core::order::effective_order;
use kombini_core::{FilesState, FixAction, Level, Mod, ModId};
use kombini_storage::LibraryState;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListFilter {
    All,
    Enabled,
    Disabled,
    Favorites,
    Updates,
    Problems,
    /// Confirmed issues or uploaded before a major game update.
    Outdated,
}

impl ListFilter {
    const ALL: [ListFilter; 7] = [
        Self::All,
        Self::Enabled,
        Self::Disabled,
        Self::Favorites,
        Self::Updates,
        Self::Problems,
        Self::Outdated,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Enabled => "Enabled",
            Self::Disabled => "Disabled",
            Self::Favorites => "Favorites",
            Self::Updates => "Updates",
            Self::Problems => "Problems",
            Self::Outdated => "Outdated",
        }
    }
}

fn has_problem(m: &Mod) -> bool {
    m.files_state() != FilesState::Ok || m.safety.iter().any(|f| f.level == Level::Error)
}

/// Enabled mods in effective order (index 0 wins), then disabled mods in
/// profile order, then anything unordered by title.
pub fn ordered_ids(lib: &LibraryState) -> Vec<ModId> {
    let existing: BTreeSet<ModId> = lib.mods.keys().cloned().collect();
    let profile = lib.profile();
    let mut out = effective_order(profile, &existing);
    let mut seen: BTreeSet<ModId> = out.iter().cloned().collect();
    for id in &profile.order {
        if existing.contains(id) && seen.insert(id.clone()) {
            out.push(id.clone());
        }
    }
    let mut rest: Vec<&Mod> = lib
        .mods
        .values()
        .filter(|m| !seen.contains(&m.id))
        .collect();
    rest.sort_by_key(|m| m.title.to_lowercase());
    out.extend(rest.into_iter().map(|m| m.id.clone()));
    out
}

pub fn filtered_rows(
    lib: &LibraryState,
    filter: ListFilter,
    needle: &str,
    outdated: &BTreeSet<ModId>,
) -> Vec<ModId> {
    let profile = lib.profile();
    ordered_ids(lib)
        .into_iter()
        .filter(|id| {
            let Some(m) = lib.mods.get(id) else {
                return false;
            };
            let on = profile.is_enabled(id);
            let keep = match filter {
                ListFilter::All => true,
                ListFilter::Enabled => on,
                ListFilter::Disabled => !on,
                ListFilter::Favorites => m.favorite,
                ListFilter::Updates => m.update.available,
                ListFilter::Problems => has_problem(m),
                ListFilter::Outdated => outdated.contains(id),
            };
            keep && (needle.is_empty() || matches_needle(m, needle))
        })
        .collect()
}

fn matches_needle(m: &Mod, needle: &str) -> bool {
    m.title.to_lowercase().contains(needle)
        || m.author
            .as_deref()
            .is_some_and(|a| a.to_lowercase().contains(needle))
        || m.heroes.iter().any(|h| h.to_lowercase().contains(needle))
        || m.hero_override
            .as_deref()
            .is_some_and(|h| h.to_lowercase().contains(needle))
        || m.category
            .as_deref()
            .is_some_and(|c| c.to_lowercase().contains(needle))
}

pub fn fix_label(fix: &FixAction) -> &'static str {
    match fix {
        FixAction::ReapplyGameinfo => "Reapply gameinfo",
        FixAction::RepairGameinfoMarkers => "Repair markers",
        FixAction::ClearReadOnly => "Clear read-only",
        FixAction::RestoreFiles { .. } => "Restore files",
        FixAction::Reinstall { .. } => "Reinstall",
        FixAction::RemoveMod { .. } => "Remove mod",
        FixAction::OpenConflicts => "View conflicts",
        FixAction::OpenImport => "Import",
        FixAction::CheckUpdates => "Check updates",
    }
}

/// Payload while dragging a row.
#[derive(Clone)]
pub struct DraggedMod {
    pub id: ModId,
    pub title: String,
    pub rank: usize,
    pub hero: Option<String>,
    pub size: String,
    pub thumb: Option<std::path::PathBuf>,
}

/// Floating copy of the row that follows the cursor while dragging.
pub struct DragGhost {
    pub m: DraggedMod,
    pub width: Pixels,
}

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let m = &self.m;
        div()
            .w(self.width)
            .h(px(ROW_H))
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(10.))
            .rounded(px(RADIUS))
            .bg(c(RAISED))
            .border_1()
            .border_color(c(ACCENT))
            .shadow_lg()
            .opacity(0.95)
            .child(
                Icon::new(Lucide::GripVertical)
                    .size(px(16.))
                    .text_color(c(ACCENT)),
            )
            .child(
                div()
                    .w(px(28.))
                    .text_xs()
                    .text_color(c(ACCENT))
                    .child(format!("#{}", m.rank)),
            )
            .child(thumb_tile(m.thumb.clone()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(c(TEXT))
                            .child(m.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(c(FAINT))
                            .child(format!("{} · drop to set load order", m.size)),
                    ),
            )
            .children(m.hero.clone().map(|h| chip(h, MUTED)))
    }
}

/// 32px square: the mod's thumbnail when cached, else a package icon.
pub fn thumb_tile(path: Option<std::path::PathBuf>) -> Div {
    let tile = div()
        .flex_none()
        .size(px(32.))
        .rounded(px(6.))
        .overflow_hidden()
        .bg(c(HOVER))
        .flex()
        .items_center()
        .justify_center();
    match path {
        Some(p) => tile.child(img(p).size_full().object_fit(ObjectFit::Cover)),
        None => tile.child(
            Icon::new(Lucide::Package)
                .size(px(16.))
                .text_color(c(FAINT)),
        ),
    }
}

impl AppView {
    /// Moves `id` to the position of `target` within the enabled order.
    pub fn move_enabled(&mut self, id: ModId, target_index: usize, cx: &mut Context<Self>) {
        let existing: BTreeSet<ModId> = self.lib.mods.keys().cloned().collect();
        let mut order = effective_order(self.lib.profile(), &existing);
        let Some(from) = order.iter().position(|m| m == &id) else {
            return;
        };
        let item = order.remove(from);
        let to = target_index.min(order.len());
        if from == to {
            order.insert(from, item);
            return;
        }
        order.insert(to, item);
        self.run(
            "reorder",
            "Changing load order",
            cx,
            move |e, _| e.reorder_enabled(&order),
            |_, _, _| {},
        );
    }

    pub fn confirm_uninstall(&mut self, ids: Vec<ModId>, cx: &mut Context<Self>) {
        let names: Vec<String> = ids
            .iter()
            .filter_map(|id| self.lib.mods.get(id).map(|m| m.title.clone()))
            .collect();
        let body = if names.len() == 1 {
            format!(
                "{} will be removed from the game and your library. A snapshot is taken first, so you can undo this from Snapshots.",
                names[0]
            )
        } else {
            format!(
                "{} mods will be removed. A snapshot is taken first, so you can undo this from Snapshots.",
                names.len()
            )
        };
        self.modal = Some(Modal::confirm(
            "Uninstall?",
            body,
            "Uninstall",
            true,
            move |this, cx| {
                let ids = ids.clone();
                this.run(
                    "uninstall",
                    "Uninstalling",
                    cx,
                    move |e, _| e.uninstall(&ids),
                    |this, report, _| this.success(report.summary),
                );
            },
        ));
        cx.notify();
    }

    pub fn render_mods(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = window;
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.render_mods_toolbar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_mod_list(cx))
                    .child(self.render_details(cx)),
            )
    }

    fn render_mods_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled_visible: Vec<ModId> = self
            .rows
            .iter()
            .filter(|id| !self.lib.profile().is_enabled(id))
            .cloned()
            .collect();
        let disabled_visible: Vec<ModId> = self
            .rows
            .iter()
            .filter(|id| self.lib.profile().is_enabled(id))
            .cloned()
            .collect();
        let bulk = self.is_pending("toggle:bulk");
        let updatable: Vec<ModId> = self
            .lib
            .mods
            .values()
            .filter(|m| m.update.available)
            .map(|m| m.id.clone())
            .collect();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .px(px(16.))
            .py(px(10.))
            .child(div().w(px(280.)).child(Input::new(&self.filter_input).small().cleanable(true).prefix(gpui_kit::component::Icon::new(gpui_kit::component::IconName::Search).size(px(14.)).text_color(c(FAINT)))))
            .child(
                div().flex().gap(px(2.)).p(px(2.)).rounded(px(RADIUS)).bg(c(PANEL)).children(
                    ListFilter::ALL.into_iter().map(|f| {
                        let active = self.list_filter == f;
                        let label = match f {
                            ListFilter::Updates if self.update_count > 0 => {
                                format!("Updates ({})", self.update_count)
                            }
                            _ => f.label().to_owned(),
                        };
                        div()
                            .id(SharedString::from(format!("filter-{}", f.label())))
                            .px(px(10.))
                            .py(px(4.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .text_xs()
                            .when(active, |d| d.bg(c(RAISED)).text_color(c(TEXT)))
                            .when(!active, |d| d.text_color(c(MUTED)))
                            .hover(|s| s.text_color(c(TEXT)))
                            .child(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.list_filter = f;
                                this.recompute_rows(cx);
                                cx.notify();
                            }))
                    }),
                ),
            )
            .child(div().flex_1())
            .child(
                Button::new("enable-shown")
                    .small()
                    .ghost()
                    .icon(Lucide::Power).label("Enable shown")
                    .loading(bulk)
                    .disabled(bulk || enabled_visible.is_empty())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_enabled(enabled_visible.clone(), true, cx)
                    })),
            )
            .child(
                Button::new("disable-shown")
                    .small()
                    .ghost()
                    .label("Disable shown")
                    .disabled(bulk || disabled_visible.is_empty())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_enabled(disabled_visible.clone(), false, cx)
                    })),
            )
            .when(!updatable.is_empty(), |d| {
                let n = updatable.len();
                d.child(
                    Button::new("update-all")
                        .small()
                        .success()
                        .icon(Lucide::Download)
                        .label(format!("Update all ({n})"))
                        .tooltip("Queue every available update in Downloads")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update_all(updatable.clone(), cx)
                        })),
                )
            })
            .child({
                let fixable = self.issues.repairable_mods().len();
                Button::new("mod-issues")
                    .small()
                    .ghost()
                    .icon(Lucide::Wrench)
                    .label(if fixable > 0 {
                        format!("Mod issues ({fixable})")
                    } else {
                        "Mod issues".to_owned()
                    })
                    .tooltip("Check mods against the installed game and fix what a game update broke")
                    .on_click(cx.listener(|this, _, _, cx| this.open_issues(cx)))
            })
            .child(
                Button::new("groups")
                    .small()
                    .ghost()
                    .icon(Lucide::Boxes)
                    .label("Groups")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.modal = Some(Modal::Groups);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("conflicts")
                    .small()
                    .ghost()
                    .icon(Lucide::Swords).label("Conflicts")
                    .on_click(cx.listener(|this, _, _, cx| this.open_conflicts(cx))),
            )
            .child(
                Button::new("snapshots")
                    .small()
                    .ghost()
                    .icon(Lucide::Camera).label("Snapshots")
                    .on_click(cx.listener(|this, _, _, cx| this.open_snapshots(cx))),
            )
            .child(
                Button::new("install-file")
                    .small()
                    .primary()
                    .icon(Lucide::FolderInput).label("Install from file…")
                    .tooltip("Archives (.zip .7z .rar), .vpk files or folders. You can also drop files anywhere.")
                    .on_click(cx.listener(|this, _, _, cx| this.install_files_dialog(cx))),
            )
    }

    fn render_mod_list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.lib.mods.is_empty() {
            return div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(12.))
                .child(heading("No mods yet"))
                .child(muted(
                    "Drop an archive or .vpk here, browse Discover, or import mods already in your addons folder.",
                ))
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(
                            Button::new("empty-discover")
                                .primary()
                                .label("Browse Discover")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.set_tab(crate::app::Tab::Discover, cx)
                                })),
                        )
                        .child(
                            Button::new("empty-import")
                                .label("Import existing")
                                .on_click(cx.listener(|this, _, _, cx| this.open_adopt(cx))),
                        ),
                )
                .into_any_element();
        }
        if self.rows.is_empty() {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(muted("No mods match this filter."))
                .into_any_element();
        }
        let count = self.rows.len();
        let bar = self.render_multi_bar(cx);
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .px(px(16.))
            .pb(px(if bar.is_some() { 76. } else { 12. }))
            .child(
                uniform_list(
                    "mod-list",
                    count,
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .filter_map(|ix| this.render_row(ix, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.list_scroll)
                .size_full(),
            )
            .children(bar)
            .into_any_element()
    }

    fn render_row(&mut self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.rows.get(ix)?.clone();
        let m = self.lib.mods.get(&id)?;
        let profile = self.lib.profile();
        let on = profile.is_enabled(&id);
        let pinned = profile.is_pinned(&id);
        let pending = self.is_pending(&format!("toggle:{id}"));
        let shown_on = if pending { !on } else { on };
        let in_multi = self.multi.contains(&id);
        let issue_chip = self.issue_chip(&id);
        let selected = self.selected.as_ref() == Some(&id) && self.multi.len() <= 1;
        let favorite = m.favorite;
        let group = profile
            .groups
            .iter()
            .find(|g| g.mod_ids.contains(&id))
            .map(|g| g.name.clone());
        let rank = on.then(|| {
            let existing: BTreeSet<ModId> = self.lib.mods.keys().cloned().collect();
            effective_order(profile, &existing)
                .iter()
                .position(|x| x == &id)
                .map(|p| p + 1)
        });
        let problem = has_problem(m);
        let title = m.title.clone();
        let hero = m.primary_hero().map(str::to_owned);
        let update = m.update.available;
        let size = format_bytes(m.total_size());
        let variants = m.roots().count();
        let thumb = m.thumbnail_url.clone().and_then(|u| self.thumb(&u, cx));
        let dragging = self.dragging.as_ref() == Some(&id);
        let drag = DraggedMod {
            id: id.clone(),
            title: title.clone(),
            rank: rank.flatten().unwrap_or(0),
            hero: hero.clone(),
            size: size.clone(),
            thumb: thumb.clone(),
        };
        let weak = cx.entity().downgrade();
        let toggle_id = id.clone();
        let select_id = id.clone();
        let drop_rank = rank.flatten();
        Some(
            div()
                .id(("mod-row", ix))
                .h(px(ROW_H))
                .w_full()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(10.))
                .mb(px(2.))
                .rounded(px(RADIUS))
                .cursor_pointer()
                .border_1()
                .border_color(if in_multi {
                    ca(ACCENT, 0.55)
                } else if selected {
                    c(PRIMARY)
                } else {
                    ca(BORDER, 0.0)
                })
                .bg(if in_multi {
                    ca(ACCENT, 0.10)
                } else if selected {
                    ca(PRIMARY, 0.18)
                } else {
                    c(PANEL)
                })
                .hover(move |s| {
                    if in_multi {
                        s.bg(ca(ACCENT, 0.16))
                    } else {
                        s.bg(c(HOVER))
                    }
                })
                .when(dragging, |d| d.opacity(0.35).border_color(ca(ACCENT, 0.6)))
                .when(on, |d| {
                    d.on_drag(drag, move |dragged: &DraggedMod, _, window, cx| {
                        let id = dragged.id.clone();
                        weak.update(cx, |this, cx| {
                            this.dragging = Some(id);
                            cx.notify();
                        })
                        .ok();
                        // Match the list width (window minus details pane and padding).
                        let width = (window.viewport_size().width - px(380. + 32.)).max(px(320.));
                        let m = dragged.clone();
                        cx.new(|_| DragGhost { m, width })
                    })
                })
                .when_some(drop_rank, |d, rank| {
                    // Line above when the dragged row moves up, below when it moves down.
                    d.drag_over::<DraggedMod>(move |s, dragged, _, _| {
                        let s = s.bg(ca(ACCENT, 0.08));
                        if dragged.rank == rank {
                            s
                        } else if dragged.rank > rank {
                            s.border_t_2().border_color(c(ACCENT))
                        } else {
                            s.border_b_2().border_color(c(ACCENT))
                        }
                    })
                    .on_drop(cx.listener(
                        move |this, dragged: &DraggedMod, _, cx| {
                            this.dragging = None;
                            this.move_enabled(dragged.id.clone(), rank - 1, cx);
                        },
                    ))
                })
                .child(
                    div()
                        .id(("grip", ix))
                        .flex_none()
                        .w(px(16.))
                        .flex()
                        .justify_center()
                        .when(in_multi, |d| {
                            d.child(
                                Icon::new(gpui_kit::component::IconName::Check)
                                    .size(px(14.))
                                    .text_color(c(ACCENT)),
                            )
                        })
                        .when(on && !in_multi, |d| {
                            d.cursor_grab()
                                .child(
                                    Icon::new(Lucide::GripVertical)
                                        .size(px(16.))
                                        .text_color(c(FAINT)),
                                )
                                .tooltip(|window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(
                                        "Drag to change load order",
                                    )
                                    .build(window, cx)
                                })
                        }),
                )
                .child(
                    div()
                        .w(px(28.))
                        .flex_none()
                        .text_xs()
                        .text_color(if on { c(ACCENT) } else { c(FAINT) })
                        .child(match rank.flatten() {
                            Some(r) => format!("#{r}"),
                            None => "–".to_owned(),
                        }),
                )
                .child(thumb_tile(thumb))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .truncate()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(if on { c(TEXT) } else { c(MUTED) })
                                        .child(title),
                                )
                                .when(favorite, |d| {
                                    d.child(
                                        Icon::new(gpui_kit::component::IconName::StarFill)
                                            .size(px(12.))
                                            .text_color(c(AMBER)),
                                    )
                                })
                                .when(pinned, |d| d.child(chip("Pinned", INFO))),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(6.))
                                .text_xs()
                                .text_color(c(FAINT))
                                .child(size)
                                .when(variants > 1, |d| d.child(format!("· {variants} variants")))
                                .when_some(group, |d, g| d.child(format!("· {g}"))),
                        ),
                )
                .when_some(hero, |d, h| d.child(chip(h, MUTED)))
                .when(update, |d| d.child(chip("Update", ACCENT)))
                .when(problem, |d| d.child(chip("Problem", CORAL)))
                .children(issue_chip)
                .when(pending, |d| {
                    d.child(gpui_kit::component::spinner::Spinner::new().xsmall())
                })
                .child(
                    Switch::new(("mod-switch", ix))
                        .checked(shown_on)
                        .disabled(pending || self.launch.phase.busy())
                        .tooltip(if on { "Disable" } else { "Enable" })
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            this.set_enabled(vec![toggle_id.clone()], *checked, cx);
                        })),
                )
                .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                    this.click_row(select_id.clone(), ev.modifiers(), cx);
                }))
                .into_any_element(),
        )
    }

    fn render_details(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let shell = div()
            .w(px(380.))
            .flex_none()
            .h_full()
            .border_l_1()
            .border_color(c(BORDER))
            .bg(c(PANEL));
        let Some(m) = self
            .selected
            .as_ref()
            .and_then(|id| self.lib.mods.get(id))
            .cloned()
        else {
            return shell
                .flex()
                .items_center()
                .justify_center()
                .flex_col()
                .gap(px(6.))
                .child(muted("Select a mod to see its details."))
                .child(faint("Ctrl-click or Shift-click to select several."))
                .into_any_element();
        };
        let id = m.id.clone();
        let profile = self.lib.profile();
        let on = profile.is_enabled(&id);
        let pinned = profile.is_pinned(&id);
        let existing: BTreeSet<ModId> = self.lib.mods.keys().cloned().collect();
        let order = effective_order(profile, &existing);
        let pos = order.iter().position(|x| x == &id);
        let last = order.len().saturating_sub(1);
        let reordering = self.is_pending("reorder");
        let roots: Vec<(String, bool, String)> = m
            .roots()
            .map(|f| (f.rel_path.clone(), f.selected, f.placement_label()))
            .collect();
        let selecting = self.is_pending(&format!("select:{id}"));
        let update_pending = self.is_pending(&format!("update:{id}"));
        let group_of = profile
            .groups
            .iter()
            .find(|g| g.mod_ids.contains(&id))
            .map(|g| (g.id.clone(), g.name.clone()));

        let mut body = div()
            .id("details")
            .size_full()
            .overflow_y_scroll()
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(14.));

        // Header
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(heading(m.title.clone()))
                .child(muted(
                    [
                        m.author.clone().map(|a| format!("by {a}")),
                        m.version
                            .clone()
                            .map(|v| format!("v{}", v.trim_start_matches(['v', 'V']))),
                        Some(format_bytes(m.total_size())),
                        Some(format!("installed {}", ago(m.installed_at))),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · "),
                ))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(6.))
                        .child(if on {
                            chip("Enabled", ACCENT)
                        } else {
                            chip("Disabled", FAINT)
                        })
                        .children(m.primary_hero().map(|h| chip(h.to_owned(), MUTED)))
                        .children(m.category.clone().map(|c| chip(c, MUTED)))
                        .when(m.files_state() != FilesState::Ok, |d| {
                            d.child(chip("Files missing", CORAL))
                        })
                        .when_some(group_of.clone(), |d, (gid, name)| {
                            let rm_id = id.clone();
                            d.child(
                                div()
                                    .id("d-group")
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .pl(px(8.))
                                    .pr(px(4.))
                                    .py(px(2.))
                                    .rounded(px(999.))
                                    .bg(ca(INFO, 0.16))
                                    .text_xs()
                                    .text_color(c(INFO))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(ca(INFO, 0.24)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.modal = Some(Modal::Groups);
                                        cx.notify();
                                    }))
                                    .child(Icon::new(Lucide::Boxes).size(px(12.)))
                                    .child(name)
                                    .child(
                                        div()
                                            .id("d-group-remove")
                                            .px(px(3.))
                                            .rounded(px(999.))
                                            .hover(|s| s.bg(ca(INFO, 0.3)))
                                            .child("×")
                                            .tooltip(|window, cx| {
                                                gpui_kit::component::tooltip::Tooltip::new(
                                                    "Remove from group",
                                                )
                                                .build(window, cx)
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.remove_from_group(
                                                    gid.clone(),
                                                    rm_id.clone(),
                                                    cx,
                                                );
                                            })),
                                    ),
                            )
                        }),
                ),
        );

        // Primary actions
        let t_id = id.clone();
        body =
            body.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .child({
                        let pending = self.is_pending(&format!("toggle:{id}"));
                        let b = Button::new("d-toggle")
                            .small()
                            .label(if on { "Disable" } else { "Enable" })
                            .loading(pending)
                            .disabled(pending)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_enabled(vec![t_id.clone()], !on, cx)
                            }));
                        if on { b } else { b.primary() }
                    })
                    .when(m.update.available, |d| {
                        let u_id = id.clone();
                        d.child(
                            Button::new("d-update")
                                .small()
                                .success()
                                .label(match &m.update.latest_version {
                                    Some(v) => format!("Update to {v}"),
                                    None => "Update".to_owned(),
                                })
                                .loading(update_pending)
                                .disabled(update_pending)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update_mod(u_id.clone(), cx)
                                })),
                        )
                        .child({
                            let s_id = id.clone();
                            Button::new("d-skip")
                                .small()
                                .ghost()
                                .icon(Lucide::SkipForward)
                                .label("Skip this version")
                                .tooltip("Hide this update; newer versions will still show")
                                .loading(self.is_pending(&format!("skip:{id}")))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.skip_update(s_id.clone(), cx)
                                }))
                        })
                    })
                    .child({
                        let f_id = id.clone();
                        let fav = m.favorite;
                        Button::new("d-fav")
                            .small()
                            .ghost()
                            .icon(if fav {
                                gpui_kit::component::IconName::StarFill
                            } else {
                                gpui_kit::component::IconName::Star
                            })
                            .label(if fav { "Favorited" } else { "Favorite" })
                            .loading(self.is_pending("favorite"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_favorite(vec![f_id.clone()], !fav, cx)
                            }))
                    })
                    .child({
                        let p_id = id.clone();
                        Button::new("d-pin")
                            .small()
                            .ghost()
                            .icon(if pinned { Lucide::PinOff } else { Lucide::Pin })
                            .label(if pinned { "Unpin" } else { "Pin to top" })
                            .tooltip("Pinned mods always load first and win conflicts")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let id = p_id.clone();
                                this.run(
                                    format!("pin:{id}"),
                                    "Pinning",
                                    cx,
                                    move |e, _| e.set_pinned(&id, !pinned),
                                    |_, _, _| {},
                                )
                            }))
                    })
                    .child({
                        let i_id = id.clone();
                        Button::new("d-inspect")
                            .small()
                            .ghost()
                            .icon(gpui_kit::component::IconName::Search)
                            .label("Inspect files")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_inspector(i_id.clone(), cx)
                            }))
                    })
                    .when_some(m.page_url.clone(), |d, url| {
                        d.child(
                            Button::new("d-page")
                                .small()
                                .ghost()
                                .icon(gpui_kit::component::IconName::ExternalLink)
                                .label("Open page")
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                        )
                    })
                    .child({
                        let u_id = id.clone();
                        Button::new("d-uninstall")
                            .small()
                            .ghost()
                            .danger()
                            .icon(Lucide::Trash)
                            .label("Uninstall")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.confirm_uninstall(vec![u_id.clone()], cx)
                            }))
                    }),
            );

        if let Some(section) = self.render_issue_section(&id, cx) {
            body = body.child(section);
        }
        if let Some(section) = self.render_sounds_section(&id, cx) {
            body = body.child(section);
        }
        if let Some(section) = self.render_versions_section(&id, cx) {
            body = body.child(section);
        }

        // Load order
        if let Some(pos) = pos {
            let ids = [id.clone(), id.clone(), id.clone(), id.clone()];
            let [a, b, c2, d2] = ids;
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(section_title("Load order"))
                    .child(muted(format!(
                        "Position #{} of {} enabled. Higher mods win when files overlap. Drag rows to reorder.",
                        pos + 1,
                        order.len()
                    )))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(
                                Button::new("mv-top")
                                    .xsmall()
                                    .icon(Lucide::ArrowUpToLine).label("Top")
                                    .disabled(pos == 0 || reordering)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.move_enabled(a.clone(), 0, cx)
                                    })),
                            )
                            .child(
                                Button::new("mv-up")
                                    .xsmall()
                                    .label("Up")
                                    .disabled(pos == 0 || reordering)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.move_enabled(b.clone(), pos.saturating_sub(1), cx)
                                    })),
                            )
                            .child(
                                Button::new("mv-down")
                                    .xsmall()
                                    .label("Down")
                                    .disabled(pos >= last || reordering)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.move_enabled(c2.clone(), pos + 1, cx)
                                    })),
                            )
                            .child(
                                Button::new("mv-bottom")
                                    .xsmall()
                                    .icon(Lucide::ArrowDownToLine).label("Bottom")
                                    .disabled(pos >= last || reordering)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.move_enabled(d2.clone(), usize::MAX, cx)
                                    })),
                            ),
                    ),
            );
        }

        // Variants
        if roots.len() > 1 {
            let all: Vec<String> = roots.iter().map(|r| r.0.clone()).collect();
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(section_title("Variants"))
                    .child(muted("Choose which parts of this mod are loaded."))
                    .children(roots.iter().enumerate().map(|(i, (rel, sel, place))| {
                        let rel2 = rel.clone();
                        let current: Vec<String> = roots
                            .iter()
                            .filter(|r| r.1)
                            .map(|r| r.0.clone())
                            .collect();
                        let v_id = id.clone();
                        let sel = *sel;
                        card()
                            .id(("variant", i))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .px(px(10.))
                            .py(px(8.))
                            .child(
                                Switch::new(("variant-sw", i))
                                    .small()
                                    .checked(sel)
                                    .disabled(selecting),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().text_sm().truncate().child(rel.clone()))
                                    .child(faint(place.clone())),
                            )
                            .cursor_pointer()
                            .hover(|s| s.bg(c(HOVER)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let mut next = current.clone();
                                if sel {
                                    next.retain(|r| r != &rel2);
                                } else {
                                    next.push(rel2.clone());
                                }
                                if next.is_empty() {
                                    this.toast(gpui_kit::component::notification::Notification::warning(
                                        "Keep at least one variant selected (or disable the mod).",
                                    ));
                                    cx.notify();
                                    return;
                                }
                                let id = v_id.clone();
                                this.run(
                                    format!("select:{id}"),
                                    "Changing variants",
                                    cx,
                                    move |e, _| e.set_selection(&id, &next),
                                    |_, _, _| {},
                                );
                            }))
                    }))
                    .child({
                        let s_id = id.clone();
                        Button::new("variants-all")
                            .xsmall()
                            .ghost()
                            .label("Select all")
                            .disabled(selecting)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let id = s_id.clone();
                                let all = all.clone();
                                this.run(
                                    format!("select:{id}"),
                                    "Changing variants",
                                    cx,
                                    move |e, _| e.set_selection(&id, &all),
                                    |_, _, _| {},
                                );
                            }))
                    }),
            );
        } else if let Some((rel, _, place)) = roots.first() {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(section_title("File"))
                    .child(div().text_sm().child(rel.clone()))
                    .child(faint(place.clone())),
            );
        }

        // Safety
        if !m.safety.is_empty() {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(section_title("Safety check"))
                    .children(m.safety.iter().map(|f| {
                        let color = match f.level {
                            Level::Error => CORAL,
                            Level::Warning => AMBER,
                            Level::Info => INFO,
                        };
                        div()
                            .flex()
                            .gap(px(8.))
                            .items_start()
                            .child(div().mt(px(6.)).child(dot(color)))
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .text_color(c(TEXT))
                                    .child(f.message.clone()),
                            )
                    })),
            );
        }

        // Requirements
        if !m.requirements.is_empty() {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(section_title("Requirements"))
                    .children(m.requirements.iter().enumerate().map(|(i, r)| {
                        let installed = r.submission_id.is_some_and(|sid| {
                            self.lib.mods.values().any(|m| m.remote_id == Some(sid))
                        });
                        let url = r.url.clone();
                        div()
                            .id(("req", i))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(dot(if installed { ACCENT } else { AMBER }))
                            .child(div().flex_1().text_sm().child(r.name.clone()))
                            .child(faint(if installed { "installed" } else { "missing" }))
                            .when_some(url, |d, url| {
                                d.cursor_pointer()
                                    .on_click(move |_, _, cx| cx.open_url(&url))
                            })
                    })),
            );
        }

        // Description
        if let Some(desc) = m.description.clone().filter(|d| !d.trim().is_empty()) {
            let text: String = desc.chars().take(1200).collect();
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(section_title("About"))
                    .child(div().text_sm().text_color(c(MUTED)).child(text)),
            );
        }

        shell
            .child(crate::anim::rise_in(
                body,
                SharedString::from(format!("details-{}", id.0)),
                8.,
                crate::anim::MEDIUM,
            ))
            .into_any_element()
    }

    pub fn update_all(&mut self, ids: Vec<ModId>, cx: &mut Context<Self>) {
        let n = ids.len();
        for id in ids {
            self.update_mod(id, cx);
        }
        self.toast(
            gpui_kit::component::notification::Notification::info(format!(
                "{n} update(s) queued. They install automatically when downloaded."
            ))
            .title("Updating"),
        );
        cx.notify();
    }

    /// Hides the currently offered update until a newer file appears.
    pub fn skip_update(&mut self, id: ModId, cx: &mut Context<Self>) {
        self.run(
            format!("skip:{id}"),
            "Skipping update",
            cx,
            move |e, p| {
                e.mutate("skip update", p, |s| {
                    let m = s
                        .mods
                        .get_mut(&id)
                        .ok_or_else(|| kombini_core::AppError::invalid("Mod not found"))?;
                    m.update.skipped_file_id = m.update.file_id;
                    m.update.available = false;
                    Ok(format!("Skipped the update for {}", m.title))
                })
            },
            |this, _, _| this.success("Update skipped. You'll be told when a newer one is out."),
        );
    }

    pub fn update_mod(&mut self, id: ModId, cx: &mut Context<Self>) {
        let Some(m) = self.lib.mods.get(&id).cloned() else {
            return;
        };
        let (Some(remote), Some(file_id)) = (m.remote_id, m.update.file_id) else {
            self.toast(gpui_kit::component::notification::Notification::warning(
                "No downloadable update was found for this mod.",
            ));
            cx.notify();
            return;
        };
        self.queue_gamebanana_install(remote, Some(file_id), true, cx);
    }
}

/// Human description of where a file currently lives.
trait PlacementLabel {
    fn placement_label(&self) -> String;
}

impl PlacementLabel for kombini_core::ModFile {
    fn placement_label(&self) -> String {
        match &self.placement {
            kombini_core::Placement::Mounted { shard, file_name } => {
                let folder = if *shard <= 1 {
                    "addons".to_owned()
                } else {
                    format!("addons{shard}")
                };
                format!(
                    "Loaded as {folder}\\{file_name} · {}",
                    format_bytes(self.size)
                )
            }
            kombini_core::Placement::Missing => "Missing from disk".to_owned(),
            _ if !self.selected => format!("Not selected · {}", format_bytes(self.size)),
            _ => format!("Stored, not loaded · {}", format_bytes(self.size)),
        }
    }
}
