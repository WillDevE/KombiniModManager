//! Multi-select in My Mods with its floating bulk-action bar, mod groups,
//! VPK merging and the shared one-line text prompt.

use crate::app::AppView;
use crate::icons::Lucide;
use crate::modal::Modal;
use crate::theme::*;
use crate::widgets::*;
use gpui_kit::component::button::*;
use gpui_kit::component::input::Input;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable, Icon, IconName, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kombini_core::{GroupId, ModId};
use kombini_engine::library::ModPatch;
use std::rc::Rc;

/// Called with the prompt's trimmed, non-empty text.
pub type PromptFn = Rc<dyn Fn(&mut AppView, String, &mut Context<AppView>)>;

impl AppView {
    // ------------------------------------------------------------ selection

    /// Row click. Plain: select one. Ctrl: add/remove. Shift: range from the
    /// current selection (the anchor) to the clicked row.
    pub fn click_row(&mut self, id: ModId, mods: Modifiers, cx: &mut Context<Self>) {
        if mods.shift {
            let anchor = self.selected.clone().unwrap_or_else(|| id.clone());
            let a = self.rows.iter().position(|x| x == &anchor);
            let b = self.rows.iter().position(|x| x == &id);
            if let (Some(a), Some(b)) = (a, b) {
                let (lo, hi) = (a.min(b), a.max(b));
                self.multi = self.rows[lo..=hi].iter().cloned().collect();
            }
            if self.selected.is_none() {
                self.selected = Some(id);
            }
        } else if mods.control || mods.platform {
            if self.multi.is_empty()
                && let Some(s) = self.selected.clone()
            {
                self.multi.insert(s);
            }
            if self.multi.remove(&id) {
                if self.selected.as_ref() == Some(&id) {
                    self.selected = self.multi.iter().next().cloned();
                }
            } else {
                self.multi.insert(id.clone());
                self.selected = Some(id);
            }
        } else {
            self.multi.clear();
            self.selected = Some(id);
        }
        cx.notify();
    }

    /// The multi-selection in list order (enabled by load order first).
    pub fn multi_ids(&self) -> Vec<ModId> {
        crate::mods::ordered_ids(&self.lib)
            .into_iter()
            .filter(|id| self.multi.contains(id))
            .collect()
    }

    pub fn clear_multi(&mut self, cx: &mut Context<Self>) {
        self.multi.clear();
        cx.notify();
    }

    pub fn set_favorite(&mut self, ids: Vec<ModId>, favorite: bool, cx: &mut Context<Self>) {
        if ids.is_empty() {
            return;
        }
        self.run(
            "favorite",
            if favorite {
                "Adding to favorites"
            } else {
                "Removing from favorites"
            },
            cx,
            move |e, _| {
                for id in &ids {
                    e.update_mod(
                        id,
                        ModPatch {
                            favorite: Some(favorite),
                            ..ModPatch::default()
                        },
                    )?;
                }
                Ok(())
            },
            |_, _, _| {},
        );
    }

    /// Floating bar shown while several mods are selected.
    pub fn render_multi_bar(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.multi.is_empty() {
            return None;
        }
        let ids = self.multi_ids();
        let n = ids.len();
        let busy = self.launch.phase.busy();
        let profile = self.lib.profile();
        let all_on = ids.iter().all(|id| profile.is_enabled(id));
        let all_off = ids.iter().all(|id| !profile.is_enabled(id));
        let all_fav = ids
            .iter()
            .all(|id| self.lib.mods.get(id).is_some_and(|m| m.favorite));
        let toggling = self.is_pending("toggle:bulk");
        let (en, dis, fav, grp, mrg, del) = (
            ids.clone(),
            ids.clone(),
            ids.clone(),
            ids.clone(),
            ids.clone(),
            ids,
        );
        let bar = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .pl(px(14.))
            .pr(px(6.))
            .py(px(6.))
            .rounded(px(12.))
            .bg(c(RAISED))
            .border_1()
            .border_color(ca(ACCENT, 0.45))
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .pr(px(6.))
                    .child(
                        div()
                            .size(px(20.))
                            .rounded(px(999.))
                            .bg(c(ACCENT))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(c(WINDOW))
                            .child(n.to_string()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child("selected"),
                    ),
            )
            .child(div().w(px(1.)).h(px(20.)).bg(c(BORDER)))
            .child(
                Button::new("multi-enable")
                    .small()
                    .ghost()
                    .icon(Lucide::Power)
                    .text_label("Enable")
                    .loading(toggling)
                    .disabled(toggling || busy || all_on)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.set_enabled(en.clone(), true, cx)),
                    ),
            )
            .child(
                Button::new("multi-disable")
                    .small()
                    .ghost()
                    .text_label("Disable")
                    .disabled(toggling || busy || all_off)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.set_enabled(dis.clone(), false, cx)),
                    ),
            )
            .child(
                Button::new("multi-fav")
                    .small()
                    .ghost()
                    .icon(if all_fav {
                        IconName::StarOff
                    } else {
                        IconName::Star
                    })
                    .text_label(if all_fav { "Unfavorite" } else { "Favorite" })
                    .loading(self.is_pending("favorite"))
                    .disabled(self.is_pending("favorite"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_favorite(fav.clone(), !all_fav, cx)
                    })),
            )
            .child(
                Button::new("multi-group")
                    .small()
                    .ghost()
                    .icon(Lucide::Boxes)
                    .text_label("Group…")
                    .tooltip("Put these mods in a group you can switch on and off at once")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.prompt_new_group(grp.clone(), window, cx)
                    })),
            )
            .child(
                Button::new("multi-merge")
                    .small()
                    .ghost()
                    .icon(Lucide::Combine)
                    .text_label("Merge…")
                    .tooltip(if n < 2 {
                        "Select at least two mods to merge"
                    } else {
                        "Combine into one VPK (fewer files for the game to load)"
                    })
                    .disabled(n < 2 || busy || self.is_pending("merge"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_merge(mrg.clone(), window, cx)
                    })),
            )
            .child(
                Button::new("multi-uninstall")
                    .small()
                    .ghost()
                    .icon(Lucide::Trash)
                    .text_label("Uninstall")
                    .disabled(busy)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.confirm_uninstall(del.clone(), cx)),
                    ),
            )
            .child(div().w(px(1.)).h(px(20.)).bg(c(BORDER)))
            .child(
                Button::new("multi-clear")
                    .small()
                    .ghost()
                    .icon(IconName::Close)
                    .tooltip("Clear selection")
                    .on_click(cx.listener(|this, _, _, cx| this.clear_multi(cx))),
            );
        Some(
            div()
                .absolute()
                .bottom(px(18.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(crate::anim::rise_in(
                    bar,
                    "multi-bar",
                    18.,
                    crate::anim::MEDIUM,
                ))
                .into_any_element(),
        )
    }

    // --------------------------------------------------------------- prompt

    #[allow(clippy::too_many_arguments)]
    pub fn open_prompt(
        &mut self,
        title: impl Into<String>,
        hint: impl Into<String>,
        initial: impl Into<String>,
        ok: impl Into<String>,
        action: impl Fn(&mut AppView, String, &mut Context<AppView>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let initial: String = initial.into();
        self.text_input.update(cx, |input, cx| {
            input.set_value(initial, window, cx);
            input.focus(window, cx);
        });
        self.modal = Some(Modal::Prompt {
            title: title.into(),
            hint: hint.into(),
            ok: ok.into(),
            action: Rc::new(action),
        });
        cx.notify();
    }

    /// Enter in the prompt input, or the OK button.
    pub fn submit_prompt(&mut self, cx: &mut Context<Self>) {
        match &self.modal {
            Some(Modal::Prompt { action, .. }) => {
                let action = action.clone();
                let text = self.text_input.read(cx).value().trim().to_string();
                if text.is_empty() {
                    return;
                }
                self.modal = None;
                action(self, text, cx);
                cx.notify();
            }
            Some(Modal::Merge { .. }) => self.start_merge(cx),
            _ => {}
        }
    }

    pub fn render_prompt(
        &mut self,
        hint: String,
        ok: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let empty = self.text_input.read(cx).value().trim().is_empty();
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .when(!hint.is_empty(), |d| d.child(muted(hint)))
            .child(Input::new(&self.text_input))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        Button::new("prompt-cancel")
                            .ghost()
                            .text_label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.close_modal(cx))),
                    )
                    .child(
                        Button::new("prompt-ok")
                            .primary()
                            .text_label(ok)
                            .disabled(empty)
                            .on_click(cx.listener(|this, _, _, cx| this.submit_prompt(cx))),
                    ),
            )
            .into_any_element()
    }

    // --------------------------------------------------------------- groups

    pub fn prompt_new_group(
        &mut self,
        ids: Vec<ModId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let n = self.lib.profile().groups.len() + 1;
        self.open_prompt(
            "New group",
            format!(
                "{} mod(s) will join this group. A mod belongs to one group at most.",
                ids.len()
            ),
            format!("Group {n}"),
            "Create group",
            move |this, name, cx| {
                let ids = ids.clone();
                this.run(
                    "group",
                    "Creating group",
                    cx,
                    move |e, _| e.create_group(&name, &ids).map(|_| name),
                    |this, name, cx| {
                        this.multi.clear();
                        this.success(format!("Group “{name}” created."));
                        cx.notify();
                    },
                );
            },
            window,
            cx,
        );
    }

    pub fn set_group_enabled(&mut self, id: GroupId, on: bool, cx: &mut Context<Self>) {
        self.run(
            format!("group:{}", id.0),
            if on {
                "Enabling group"
            } else {
                "Disabling group"
            },
            cx,
            move |e, _| e.set_group_enabled(&id, on),
            |this, report, _| {
                for w in report.warnings.iter().take(2) {
                    this.toast(gpui_kit::component::notification::Notification::warning(
                        w.clone(),
                    ));
                }
            },
        );
    }

    pub fn remove_from_group(&mut self, gid: GroupId, mod_id: ModId, cx: &mut Context<Self>) {
        let Some(group) = self
            .lib
            .profile()
            .groups
            .iter()
            .find(|g| g.id == gid)
            .cloned()
        else {
            return;
        };
        let members: Vec<ModId> = group.mod_ids.into_iter().filter(|m| m != &mod_id).collect();
        self.run(
            "group",
            "Updating group",
            cx,
            move |e, _| e.update_group(&gid, None, Some(members), None),
            |_, _, _| {},
        );
    }

    pub fn render_groups(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let groups = self.lib.profile().groups.clone();
        let selection = self.multi_ids();
        let busy = self.launch.phase.busy();
        let head = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(div().flex_1().child(muted(
                "Groups switch several mods on or off together. A mod belongs to one group at most.",
            )))
            .child({
                let sel = selection.clone();
                Button::new("group-new")
                    .small()
                    .primary()
                    .icon(IconName::Plus)
                    .text_label(if sel.is_empty() {
                        "New group".to_owned()
                    } else {
                        format!("New group from {} selected", sel.len())
                    })
                    .tooltip(if sel.is_empty() {
                        "Tip: Ctrl- or Shift-click mods in the list first to fill the group"
                    } else {
                        "Creates a group with the selected mods"
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.prompt_new_group(sel.clone(), window, cx)
                    }))
            });
        let profile = self.lib.profile().clone();
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(head)
            .when(groups.is_empty(), |d| {
                d.child(
                    div()
                        .py(px(24.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(8.))
                        .child(Icon::new(Lucide::Boxes).size(px(28.)).text_color(c(FAINT)))
                        .child(muted("No groups yet.")),
                )
            })
            .children(groups.into_iter().enumerate().map(|(i, g)| {
                let total = g.mod_ids.len();
                let on_count = g.mod_ids.iter().filter(|m| profile.is_enabled(m)).count();
                let names: Vec<String> = g
                    .mod_ids
                    .iter()
                    .filter_map(|m| self.lib.mods.get(m).map(|m| m.title.clone()))
                    .collect();
                let preview = match names.len() {
                    0 => "Empty".to_owned(),
                    1..=3 => names.join(", "),
                    n => format!("{}, +{} more", names[..3].join(", "), n - 3),
                };
                let pending = self.is_pending(&format!("group:{}", g.id.0));
                let (state_text, state_color) = if total == 0 || on_count == 0 {
                    ("Off".to_owned(), FAINT)
                } else if on_count == total {
                    ("On".to_owned(), ACCENT)
                } else {
                    (format!("{on_count}/{total} on"), AMBER)
                };
                let (sw_id, sel_id, ren_id, del_id) =
                    (g.id.clone(), g.id.clone(), g.id.clone(), g.id.clone());
                let members = g.mod_ids.clone();
                let (ren_name, del_name) = (g.name.clone(), g.name.clone());
                crate::anim::rise_in_delayed(
                    card()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .p(px(12.))
                        .child(Icon::new(Lucide::Boxes).size(px(18.)).text_color(c(
                            if on_count > 0 { ACCENT } else { FAINT },
                        )))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(g.name.clone()),
                                        )
                                        .child(chip(state_text, state_color)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(c(FAINT))
                                        .truncate()
                                        .child(format!("{total} mod(s) · {preview}")),
                                ),
                        )
                        .child(
                            Button::new(("group-select", i))
                                .small()
                                .ghost()
                                .text_label("Select")
                                .tooltip("Select these mods in the list")
                                .disabled(total == 0)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let _ = &sel_id;
                                    this.multi = members.iter().cloned().collect();
                                    this.selected = members.first().cloned();
                                    this.list_filter = crate::mods::ListFilter::All;
                                    this.recompute_rows(cx);
                                    this.modal = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new(("group-rename", i))
                                .small()
                                .ghost()
                                .text_label("Rename")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let gid = ren_id.clone();
                                    this.open_prompt(
                                        "Rename group",
                                        "",
                                        ren_name.clone(),
                                        "Rename",
                                        move |this, name, cx| {
                                            let gid = gid.clone();
                                            this.run(
                                                "group",
                                                "Renaming group",
                                                cx,
                                                move |e, _| e.update_group(&gid, Some(name), None, None),
                                                |this, _, cx| {
                                                    this.modal = Some(Modal::Groups);
                                                    cx.notify();
                                                },
                                            );
                                        },
                                        window,
                                        cx,
                                    );
                                })),
                        )
                        .child(
                            Button::new(("group-delete", i))
                                .small()
                                .ghost()
                                .icon(Lucide::Trash)
                                .tooltip("Delete group (its mods stay installed)")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let gid = del_id.clone();
                                    this.modal = Some(Modal::confirm(
                                        "Delete group?",
                                        format!("“{del_name}” is removed. Its mods stay installed and keep their current state."),
                                        "Delete group",
                                        true,
                                        move |this, cx| {
                                            let gid = gid.clone();
                                            this.run(
                                                "group",
                                                "Deleting group",
                                                cx,
                                                move |e, _| e.delete_group(&gid),
                                                |this, _, cx| {
                                                    this.modal = Some(Modal::Groups);
                                                    cx.notify();
                                                },
                                            );
                                        },
                                    ));
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .when(pending, |d| {
                                    d.child(gpui_kit::component::spinner::Spinner::new().xsmall())
                                })
                                .child(
                                    Switch::new(("group-switch", i))
                                        .checked(total > 0 && on_count == total)
                                        .disabled(pending || busy || total == 0)
                                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                            this.set_group_enabled(sw_id.clone(), *checked, cx)
                                        })),
                                ),
                        ),
                    ("group-in", i),
                    8.,
                    std::time::Duration::from_millis(30 * i.min(10) as u64),
                    crate::anim::MEDIUM,
                )
            }))
            .into_any_element()
    }

    // ---------------------------------------------------------------- merge

    pub fn open_merge(&mut self, ids: Vec<ModId>, window: &mut Window, cx: &mut Context<Self>) {
        if ids.len() < 2 {
            return;
        }
        let titles: Vec<String> = ids
            .iter()
            .filter_map(|id| self.lib.mods.get(id).map(|m| m.title.clone()))
            .collect();
        let name = match titles.len() {
            2 => format!("{} + {}", titles[0], titles[1]),
            n => format!("{} + {} more", titles[0], n - 1),
        };
        self.text_input.update(cx, |input, cx| {
            input.set_value(name, window, cx);
            input.focus(window, cx);
        });
        self.modal = Some(Modal::Merge {
            ids,
            disable_sources: true,
        });
        cx.notify();
    }

    fn start_merge(&mut self, cx: &mut Context<Self>) {
        let Some(Modal::Merge {
            ids,
            disable_sources,
        }) = &self.modal
        else {
            return;
        };
        let (ids, disable) = (ids.clone(), *disable_sources);
        let title = self.text_input.read(cx).value().trim().to_string();
        if title.is_empty() {
            return;
        }
        self.modal = None;
        let count = ids.len();
        self.run_with(
            "merge",
            format!("Merging {count} mods"),
            true,
            cx,
            move |e, p| e.merge_mods(&ids, &title, disable, p),
            move |this, r, cx| {
                this.multi.clear();
                this.selected = Some(r.install.mod_id.clone());
                let mut msg = format!(
                    "Merged {count} mods into “{}” ({} files).",
                    r.install.title, r.entries
                );
                if r.overridden_paths > 0 {
                    msg.push_str(&format!(
                        " {} overlapping file(s) resolved by load order.",
                        r.overridden_paths
                    ));
                }
                if !r.disabled_sources.is_empty() {
                    msg.push_str(" The originals were disabled.");
                }
                this.success(msg);
                cx.notify();
            },
        );
    }

    pub fn render_merge(
        &mut self,
        ids: Vec<ModId>,
        disable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let empty = self.text_input.read(cx).value().trim().is_empty();
        let profile = self.lib.profile();
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(muted(
                "The selected mods are combined into a single new mod. When two mods contain the same file, the one higher in your load order wins.",
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .p(px(8.))
                    .rounded(px(RADIUS))
                    .bg(c(WINDOW))
                    .children(ids.iter().enumerate().map(|(i, id)| {
                        let (title, size) = self
                            .lib
                            .mods
                            .get(id)
                            .map(|m| (m.title.clone(), format_bytes(m.total_size())))
                            .unwrap_or_default();
                        let on = profile.is_enabled(id);
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(6.))
                            .py(px(3.))
                            .child(
                                div()
                                    .w(px(22.))
                                    .text_xs()
                                    .text_color(c(if i == 0 { ACCENT } else { FAINT }))
                                    .child(format!("{}", i + 1)),
                            )
                            .child(div().flex_1().min_w_0().truncate().text_sm().child(title))
                            .when(!on, |d| d.child(chip("Disabled", FAINT)))
                            .child(faint(size))
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(section_title("Name"))
                    .child(Input::new(&self.text_input)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        Switch::new("merge-disable")
                            .checked(disable)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                if let Some(Modal::Merge {
                                    disable_sources, ..
                                }) = &mut this.modal
                                {
                                    *disable_sources = *checked;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().text_sm().child("Disable the originals"))
                            .child(faint("They stay installed, so you can switch back any time.")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        Button::new("merge-cancel")
                            .ghost()
                            .text_label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.close_modal(cx))),
                    )
                    .child(
                        Button::new("merge-ok")
                            .primary()
                            .icon(Lucide::Combine)
                            .text_label(format!("Merge {} mods", ids.len()))
                            .disabled(empty)
                            .on_click(cx.listener(|this, _, _, cx| this.start_merge(cx))),
                    ),
            )
            .into_any_element()
    }

    // ------------------------------------------------------------- variants

    /// Centered picker shown right after installing a mod with several
    /// variants. Clicking a variant loads only that one and closes.
    pub fn render_variants(&mut self, id: ModId, cx: &mut Context<Self>) -> AnyElement {
        let Some(m) = self.lib.mods.get(&id).cloned() else {
            return muted("This mod is no longer installed.").into_any_element();
        };
        let selecting = self.is_pending(&format!("select:{id}"));
        let roots: Vec<(String, String, String, u64, bool)> = m
            .roots()
            .map(|f| {
                let (folder, file) = match f.rel_path.rsplit_once('/') {
                    Some((dir, file)) => (dir.replace('/', " / "), file.to_owned()),
                    None => (f.original_name.clone(), String::new()),
                };
                let size = f.size + m.chunks_of(&f.rel_path).iter().map(|c| c.size).sum::<u64>();
                (f.rel_path.clone(), folder, file, size, f.selected)
            })
            .collect();
        let selected_count = roots.iter().filter(|r| r.4).count();
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(muted(
                "This mod comes in several versions. Pick the one you want; you can change it later in the details pane.",
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .children(roots.into_iter().enumerate().map(
                        |(i, (rel, folder, file, size, sel))| {
                            let pick_id = id.clone();
                            crate::anim::rise_in_delayed(
                                div()
                                    .id(("variant-pick", i))
                                    .flex()
                                    .items_center()
                                    .gap(px(12.))
                                    .px(px(14.))
                                    .py(px(12.))
                                    .rounded(px(RADIUS))
                                    .border_1()
                                    .border_color(if sel { ca(ACCENT, 0.5) } else { c(BORDER) })
                                    .bg(if sel { ca(ACCENT, 0.08) } else { c(RAISED) })
                                    .cursor_pointer()
                                    .hover(|s| s.bg(c(HOVER)).border_color(c(ACCENT)))
                                    .child(
                                        div()
                                            .flex_none()
                                            .size(px(28.))
                                            .rounded(px(999.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .bg(if sel { c(ACCENT) } else { c(HOVER) })
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(if sel { c(WINDOW) } else { c(MUTED) })
                                            .child(format!("{}", i + 1)),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .flex_1()
                                            .min_w_0()
                                            .gap(px(2.))
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .truncate()
                                                    .child(folder),
                                            )
                                            .child(faint(if file.is_empty() {
                                                format_bytes(size)
                                            } else {
                                                format!("{file} · {}", format_bytes(size))
                                            })),
                                    )
                                    .when(sel && selected_count == 1, |d| {
                                        d.child(chip("Current", ACCENT))
                                    })
                                    .when(selecting, |d| {
                                        d.child(
                                            gpui_kit::component::spinner::Spinner::new().xsmall(),
                                        )
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if this.is_pending(&format!("select:{pick_id}")) {
                                            return;
                                        }
                                        let id = pick_id.clone();
                                        let only = vec![rel.clone()];
                                        this.modal = None;
                                        this.run(
                                            format!("select:{id}"),
                                            "Changing variant",
                                            cx,
                                            move |e, _| e.set_selection(&id, &only),
                                            |this, _, _| this.success("Variant selected."),
                                        );
                                        cx.notify();
                                    })),
                                ("variant-in", i),
                                8.,
                                std::time::Duration::from_millis(35 * i.min(10) as u64),
                                crate::anim::MEDIUM,
                            )
                        },
                    )),
            )
            .child(
                div().flex().justify_end().child(
                    Button::new("variants-later")
                        .ghost()
                        .text_label(if selected_count > 1 {
                            format!("Keep all {selected_count}")
                        } else {
                            "Keep current".to_owned()
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.close_modal(cx))),
                ),
            )
            .into_any_element()
    }
}
