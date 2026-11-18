//! In-app modal sheets (confirmations, profiles, snapshots, conflicts,
//! activity, health, VPK inspector, unmanaged import).

use crate::app::AppView;
use crate::theme::*;
use crate::widgets::*;
use gpui_kit::component::button::*;
use gpui_kit::component::input::Input;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kombini_core::{Level, ModId};
use kombini_engine::library::ConflictsView;
use kombini_engine::snapshots::{SnapshotInfo, SnapshotKind};
use kombini_import::managers::Found;
use kombini_import::unmanaged::Candidate;
use kombini_vpk::{EntryPage, EntryQuery, SortKey, VpkIndex};
use std::rc::Rc;
use std::sync::Arc;

pub const INSPECTOR_PAGE: usize = 200;

/// What the import sheet offers: other managers' libraries, then loose files.
pub struct AdoptData {
    pub managers: Vec<Found>,
    /// Unmanaged files no manager above claims.
    pub candidates: Vec<Candidate>,
}

type ConfirmFn = Rc<dyn Fn(&mut AppView, &mut Context<AppView>)>;

pub enum Modal {
    Confirm {
        title: String,
        body: String,
        ok: String,
        danger: bool,
        action: ConfirmFn,
    },
    Profiles,
    Activity,
    Health,
    Snapshots(Option<Vec<SnapshotInfo>>),
    Conflicts(Option<Arc<ConflictsView>>),
    Inspector(Box<Inspector>),
    Adopt(Option<Arc<AdoptData>>),
    ShareCode(String),
    Groups,
    /// Mod issues found by comparing mods with the installed game.
    Issues,
    /// Choose which variant of a freshly installed mod to load.
    Variants(ModId),
    Merge {
        ids: Vec<ModId>,
        disable_sources: bool,
    },
    Prompt {
        title: String,
        hint: String,
        ok: String,
        action: crate::selection::PromptFn,
    },
    /// Pick mods to install from a GameBanana collection.
    Collection(Box<crate::collections::CollectionView>),
    /// Full-size screenshots of a GameBanana mod.
    Lightbox(Box<crate::gallery::Lightbox>),
    /// Enabling `id` would stack a second skin on its hero.
    HeroSwap {
        id: ModId,
        others: Vec<ModId>,
    },
}

impl Modal {
    /// Stable name of the modal kind; keys the open animation so it replays
    /// when a different modal opens but not when the same one reloads.
    pub fn kind(&self) -> &'static str {
        match self {
            Modal::Confirm { .. } => "confirm",
            Modal::Profiles => "profiles",
            Modal::Activity => "activity",
            Modal::Health => "health",
            Modal::Snapshots(_) => "snapshots",
            Modal::Conflicts(_) => "conflicts",
            Modal::Inspector(_) => "inspector",
            Modal::Adopt(_) => "adopt",
            Modal::ShareCode(_) => "share",
            Modal::Groups => "groups",
            Modal::Issues => "issues",
            Modal::Variants(_) => "variants",
            Modal::Merge { .. } => "merge",
            Modal::Prompt { .. } => "prompt",
            Modal::HeroSwap { .. } => "hero-swap",
            Modal::Collection(_) => "collection",
            Modal::Lightbox(_) => "lightbox",
        }
    }
}

pub struct Inspector {
    pub title: String,
    /// The archive shown (for sound previews).
    pub blob: Option<kombini_core::BlobHash>,
    pub index: Option<Arc<VpkIndex>>,
    pub page: Option<EntryPage>,
    pub offset: usize,
    pub error: Option<String>,
}

impl Modal {
    pub fn confirm(
        title: impl Into<String>,
        body: impl Into<String>,
        ok: impl Into<String>,
        danger: bool,
        action: impl Fn(&mut AppView, &mut Context<AppView>) + 'static,
    ) -> Self {
        Self::Confirm {
            title: title.into(),
            body: body.into(),
            ok: ok.into(),
            danger,
            action: Rc::new(action),
        }
    }
}

impl AppView {
    pub fn close_modal(&mut self, cx: &mut Context<Self>) {
        self.modal = None;
        cx.notify();
    }

    pub fn open_conflicts(&mut self, cx: &mut Context<Self>) {
        self.modal = Some(Modal::Conflicts(None));
        cx.notify();
        self.background(
            cx,
            |e| Ok(Arc::new(e.conflicts())),
            |this, view, cx| {
                if let Some(Modal::Conflicts(slot)) = &mut this.modal {
                    *slot = Some(view);
                }
                cx.notify();
            },
        );
    }

    pub fn open_snapshots(&mut self, cx: &mut Context<Self>) {
        self.modal = Some(Modal::Snapshots(None));
        cx.notify();
        self.reload_snapshots(cx);
    }

    pub fn export_snapshot_dialog(&mut self, id: String, label: String, cx: &mut Context<Self>) {
        let start = crate::settings::dirs_home().unwrap_or_else(|| self.dirs.data.clone());
        let safe: String = label
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == ' ' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let rx = cx.prompt_for_new_path(&start, Some(&format!("{}.kmmsnap", safe.trim())));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(dest))) = rx.await {
                this.update(cx, |this, cx| {
                    let shown = dest.display().to_string();
                    let reveal = dest.clone();
                    this.run_with(
                        format!("snap-export:{id}"),
                        "Exporting snapshot",
                        true,
                        cx,
                        move |e, p| e.export_snapshot(&id, &dest, p),
                        move |this, _, cx| {
                            this.success(format!("Snapshot exported to {shown}."));
                            cx.reveal_path(&reveal);
                        },
                    );
                })
                .ok();
            }
        })
        .detach();
    }

    pub fn import_snapshot_dialog(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose a .kmmsnap file".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update(cx, |this, cx| {
                    this.run_with(
                        "snapshot",
                        "Importing snapshot",
                        true,
                        cx,
                        move |e, p| e.import_snapshot(&path, p),
                        |this, info, cx| {
                            this.success(format!(
                                "Imported “{}”. Restore it to apply it.",
                                info.label
                            ));
                            this.open_snapshots(cx);
                        },
                    );
                })
                .ok();
            }
        })
        .detach();
    }

    fn reload_snapshots(&mut self, cx: &mut Context<Self>) {
        self.background(
            cx,
            |e| e.list_snapshots(),
            |this, list, cx| {
                if let Some(Modal::Snapshots(slot)) = &mut this.modal {
                    *slot = Some(list);
                }
                cx.notify();
            },
        );
    }

    pub fn open_adopt(&mut self, cx: &mut Context<Self>) {
        self.modal = Some(Modal::Adopt(None));
        cx.notify();
        self.background(
            cx,
            |e| {
                let managers = kombini_import::managers::detect(&e.paths().citadel);
                let claimed: std::collections::BTreeSet<String> =
                    managers.iter().flat_map(Found::file_set).collect();
                let candidates = kombini_import::unmanaged::scan(e)
                    .into_iter()
                    .filter(|c| {
                        !c.files.iter().all(|f| {
                            let path = e.paths().shard(f.shard).join(&f.file_name);
                            claimed.contains(&kombini_import::managers::path_key(&path))
                        })
                    })
                    .collect();
                Ok(AdoptData {
                    managers,
                    candidates,
                })
            },
            |this, data, cx| {
                if let Some(Modal::Adopt(slot)) = &mut this.modal {
                    *slot = Some(Arc::new(data));
                }
                cx.notify();
            },
        );
    }

    /// Opens the paged VPK inspector for a mod's first root file.
    pub fn open_inspector(&mut self, id: ModId, cx: &mut Context<Self>) {
        let Some(m) = self.lib.mods.get(&id) else {
            return;
        };
        let title = m.title.clone();
        let blob = m
            .roots()
            .find(|f| f.selected)
            .or_else(|| m.roots().next())
            .map(|f| f.blob.clone());
        self.modal = Some(Modal::Inspector(Box::new(Inspector {
            title,
            blob: blob.clone(),
            index: None,
            page: None,
            offset: 0,
            error: None,
        })));
        cx.notify();
        let Some(blob) = blob else { return };
        self.background(
            cx,
            move |e| {
                let path = e.blobs().path_for(&blob);
                let archive = kombini_vpk::VpkArchive::open(&path).map_err(|err| {
                    kombini_core::AppError::new(
                        kombini_core::ErrorCode::InvalidVpk,
                        err.to_string(),
                    )
                })?;
                let index = Arc::new(VpkIndex::new(archive));
                let page = index.query(&query(0));
                Ok((index, page))
            },
            |this, (index, page), cx| {
                if let Some(Modal::Inspector(ins)) = &mut this.modal {
                    ins.index = Some(index);
                    ins.page = Some(page);
                }
                cx.notify();
            },
        );
    }

    fn inspector_page(&mut self, offset: usize, cx: &mut Context<Self>) {
        let Some(Modal::Inspector(ins)) = &mut self.modal else {
            return;
        };
        let Some(index) = ins.index.clone() else {
            return;
        };
        ins.offset = offset;
        cx.spawn(async move |this, cx| {
            let page = cx
                .background_executor()
                .spawn(async move { index.query(&query(offset)) })
                .await;
            this.update(cx, |this, cx| {
                if let Some(Modal::Inspector(ins)) = &mut this.modal
                    && ins.offset == offset
                {
                    ins.page = Some(page);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn render_modal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let _ = window;
        let kind = self.modal.as_ref()?.kind();
        let modal = self.modal.as_ref()?;
        let (title, width, body): (String, f32, AnyElement) = match modal {
            Modal::Confirm {
                title,
                body,
                ok,
                danger,
                action,
            } => {
                let action = action.clone();
                let ok_btn = Button::new("confirm-ok")
                    .label(ok.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.modal = None;
                        action(this, cx);
                        cx.notify();
                    }));
                let ok_btn = if *danger {
                    ok_btn.danger()
                } else {
                    ok_btn.primary()
                };
                (
                    title.clone(),
                    440.,
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(16.))
                        .child(div().text_sm().text_color(c(MUTED)).child(body.clone()))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(8.))
                                .child(
                                    Button::new("confirm-cancel")
                                        .ghost()
                                        .label("Cancel")
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.close_modal(cx)),
                                        ),
                                )
                                .child(ok_btn),
                        )
                        .into_any_element(),
                )
            }
            Modal::Profiles => ("Profiles".into(), 480., self.render_profiles(cx)),
            Modal::Groups => ("Groups".into(), 600., self.render_groups(cx)),
            Modal::Issues => ("Mod issues".into(), 680., self.render_issues(cx)),
            Modal::HeroSwap { id, others } => {
                let (id, others) = (id.clone(), others.clone());
                (
                    "Swap skins?".into(),
                    520.,
                    self.render_hero_swap(id, others, cx),
                )
            }
            Modal::Variants(id) => {
                let id = id.clone();
                let title = self
                    .lib
                    .mods
                    .get(&id)
                    .map(|m| format!("Choose a variant · {}", m.title))
                    .unwrap_or_else(|| "Choose a variant".into());
                (title, 560., self.render_variants(id, cx))
            }
            Modal::Merge {
                ids,
                disable_sources,
            } => {
                let (ids, disable) = (ids.clone(), *disable_sources);
                (
                    "Merge mods".into(),
                    560.,
                    self.render_merge(ids, disable, cx),
                )
            }
            Modal::Prompt {
                title, hint, ok, ..
            } => {
                let (title, hint, ok) = (title.clone(), hint.clone(), ok.clone());
                (title, 440., self.render_prompt(hint, ok, cx))
            }
            Modal::Activity => ("Activity".into(), 520., self.render_activity()),
            Modal::Lightbox(_) => (self.lightbox_title(), 920., self.render_lightbox(cx)),
            Modal::Collection(_) => (
                "Install a collection".into(),
                600.,
                self.render_collection(cx),
            ),
            Modal::Health => ("Health".into(), 560., self.render_health(cx)),
            Modal::Snapshots(list) => {
                let list = list.clone();
                ("Snapshots".into(), 600., self.render_snapshots(list, cx))
            }
            Modal::Conflicts(view) => {
                let view = view.clone();
                ("Conflicts".into(), 640., self.render_conflicts(view))
            }
            Modal::Inspector(ins) => {
                let t = format!("Files in {}", ins.title);
                (t, 760., self.render_inspector(cx))
            }
            Modal::Adopt(data) => {
                let data = data.clone();
                (
                    "Import existing mods".into(),
                    600.,
                    self.render_adopt(data, cx),
                )
            }
            Modal::ShareCode(code) => {
                let code = code.clone();
                (
                    "Share code".into(),
                    520.,
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(12.))
                        .child(muted("Anyone with this code can recreate the profile (GameBanana mods only)."))
                        .child(
                            card()
                                .p(px(10.))
                                .text_xs()
                                .text_color(c(TEXT))
                                .child(code.chars().take(600).collect::<String>()),
                        )
                        .child(
                            Button::new("copy-code")
                                .primary()
                                .label("Copy to clipboard")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
                                    this.success("Share code copied.");
                                    cx.notify();
                                })),
                        )
                        .into_any_element(),
                )
            }
        };
        Some(
            div()
                .id("modal-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(ca(0x000000, 0.55))
                .on_click(cx.listener(|this, _, _, cx| this.close_modal(cx)))
                .with_animation(
                    SharedString::from(format!("modal-fade-{kind}")),
                    Animation::new(crate::anim::FAST),
                    |el, t| el.opacity(t),
                )
                .child(crate::anim::rise_in(
                    div()
                        .id("modal")
                        .w(px(width))
                        .max_h(relative(0.85))
                        .flex()
                        .flex_col()
                        .bg(c(PANEL))
                        .border_1()
                        .border_color(c(BORDER))
                        .rounded(px(12.))
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .px(px(18.))
                                .py(px(14.))
                                .border_b_1()
                                .border_color(c(BORDER))
                                .child(div().flex_1().child(heading(title)))
                                .child(
                                    Button::new("modal-close")
                                        .xsmall()
                                        .ghost()
                                        .label("Close")
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.close_modal(cx)),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .id("modal-body")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .p(px(18.))
                                .child(body),
                        ),
                    SharedString::from(format!("modal-panel-{kind}")),
                    14.,
                    crate::anim::MEDIUM,
                ))
                .into_any_element(),
        )
    }

    fn render_profiles(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let profiles = self.lib.profiles.clone();
        let active = self.lib.active_profile.clone();
        let switching = self.is_pending("profile");
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(profiles.into_iter().enumerate().map(|(i, p)| {
                let is_active = p.id == active;
                let id = p.id.clone();
                let del_id = p.id.clone();
                let share_id = p.id.clone();
                let name = p.name.clone();
                card()
                    .id(("profile", i))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .p(px(10.))
                    .border_color(if is_active { c(PRIMARY) } else { c(BORDER) })
                    .child(dot(if is_active { ACCENT } else { FAINT }))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .child(div().text_sm().child(p.name.clone()))
                            .child(faint(format!(
                                "{} enabled · {} in order",
                                p.enabled.len(),
                                p.order.len()
                            ))),
                    )
                    .child(
                        Button::new(("p-share", i))
                            .xsmall()
                            .ghost()
                            .label("Share")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let id = share_id.clone();
                                this.background(
                                    cx,
                                    move |e| {
                                        let (doc, _) =
                                            kombini_import::modprofile::export(e, Some(&id))?;
                                        kombini_import::modprofile::encode(&doc)
                                    },
                                    |this, code, cx| {
                                        this.modal = Some(Modal::ShareCode(code));
                                        cx.notify();
                                    },
                                );
                            })),
                    )
                    .when(!is_active, |d| {
                        d.child(
                            Button::new(("p-del", i))
                                .xsmall()
                                .ghost()
                                .danger()
                                .label("Delete")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let id = del_id.clone();
                                    this.modal = Some(Modal::confirm(
                                        "Delete profile?",
                                        format!("{name} will be deleted. Installed mods are kept."),
                                        "Delete",
                                        true,
                                        move |this, cx| {
                                            let id = id.clone();
                                            this.run(
                                                "profile",
                                                "Deleting profile",
                                                cx,
                                                move |e, _| e.delete_profile(&id),
                                                |this, _, _| this.success("Profile deleted."),
                                            );
                                        },
                                    ));
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new(("p-use", i))
                                .xsmall()
                                .primary()
                                .label("Switch")
                                .loading(switching)
                                .disabled(switching || self.launch.phase.busy())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let id = id.clone();
                                    this.run(
                                        "profile",
                                        "Switching profile",
                                        cx,
                                        move |e, p| e.activate_profile(&id, p),
                                        |this, r, cx| {
                                            this.success(r.summary);
                                            this.close_modal(cx);
                                        },
                                    );
                                })),
                        )
                    })
                    .when(is_active, |d| d.child(chip("Active", ACCENT)))
            }))
            .child(divider())
            .child(section_title("New profile or import a share code"))
            .child(Input::new(&self.text_input).small())
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        Button::new("p-new")
                            .small()
                            .label("Create (copy current)")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let name = this.text_input.read(cx).value().trim().to_string();
                                if name.is_empty() {
                                    this.toast(Notification::warning("Type a profile name first."));
                                    cx.notify();
                                    return;
                                }
                                let from = this.lib.active_profile.clone();
                                this.run(
                                    "profile",
                                    "Creating profile",
                                    cx,
                                    move |e, _| e.create_profile(&name, Some(&from)),
                                    |this, _, _| this.success("Profile created."),
                                );
                            })),
                    )
                    .child(
                        Button::new("p-import")
                            .small()
                            .label("Import code")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let text = this.text_input.read(cx).value().to_string();
                                this.run(
                                    "profile",
                                    "Importing profile",
                                    cx,
                                    move |e, p| {
                                        let doc = kombini_import::modprofile::decode(&text)?;
                                        kombini_import::modprofile::apply(e, &doc, true, p)
                                    },
                                    |this, r, cx| {
                                        this.success(format!("Imported profile {}.", r.name));
                                        for m in &r.missing {
                                            this.queue_gamebanana_install(
                                                m.submission_id,
                                                m.file_id,
                                                false,
                                                cx,
                                            );
                                        }
                                        this.close_modal(cx);
                                    },
                                );
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_activity(&self) -> AnyElement {
        if self.history.is_empty() {
            return muted("Nothing has happened yet.").into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .children(self.history.iter().map(|r| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(dot(if r.ok { ACCENT } else { CORAL }))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().child(r.label.clone()))
                            .when(!r.ok, |d| d.child(faint(r.message.clone()))),
                    )
                    .child(faint(format!(
                        "{} ms · {}s ago",
                        r.took.as_millis(),
                        r.at.elapsed().as_secs()
                    )))
            }))
            .into_any_element()
    }

    fn render_health(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let issues = self
            .health
            .as_ref()
            .map(|h| h.issues.clone())
            .unwrap_or_default();
        if issues.is_empty() {
            return muted("No problems found.").into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(issues.into_iter().enumerate().map(|(i, issue)| {
                let color = match issue.level {
                    Level::Error => CORAL,
                    Level::Warning => AMBER,
                    Level::Info => INFO,
                };
                card()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .p(px(10.))
                    .child(dot(color))
                    .child(div().flex_1().text_sm().child(issue.message.clone()))
                    .when_some(issue.fix.clone(), |d, fix| {
                        d.child(
                            Button::new(("h-fix", i))
                                .xsmall()
                                .label(crate::mods::fix_label(&fix))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.apply_fix(fix.clone(), cx)
                                })),
                        )
                    })
            }))
            .into_any_element()
    }

    fn render_snapshots(
        &mut self,
        list: Option<Vec<SnapshotInfo>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let creating = self.is_pending("snapshot");
        let head = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(div().flex_1().child(muted(
                "Snapshots save which mods are installed and enabled so you can roll back.",
            )))
            .child(
                Button::new("snap-import")
                    .small()
                    .ghost()
                    .icon(crate::icons::Lucide::FolderInput)
                    .label("Import…")
                    .tooltip("Import a .kmmsnap file exported from Kombini")
                    .disabled(creating)
                    .on_click(cx.listener(|this, _, _, cx| this.import_snapshot_dialog(cx))),
            )
            .child(
                Button::new("snap-new")
                    .small()
                    .primary()
                    .label("Take snapshot")
                    .loading(creating)
                    .disabled(creating)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.run(
                            "snapshot",
                            "Taking snapshot",
                            cx,
                            |e, _| e.create_snapshot("Manual snapshot", SnapshotKind::Manual),
                            |this, _, cx| {
                                this.success("Snapshot saved.");
                                this.reload_snapshots(cx);
                            },
                        )
                    })),
            );
        let Some(list) = list else {
            return div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(head)
                .child(gpui_kit::component::spinner::Spinner::new())
                .into_any_element();
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(head)
            .when(list.is_empty(), |d| d.child(muted("No snapshots yet.")))
            .children(list.into_iter().enumerate().map(|(i, s)| {
                let id = s.id.clone();
                let label = s.label.clone();
                card()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .p(px(10.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .child(div().text_sm().child(s.label.clone()))
                            .child(faint(format!("{} · {} mods, {} enabled · {}", ago(s.created_at), s.mods, s.enabled, s.kind))),
                    )
                    .child({
                        let (e_id, e_label) = (s.id.clone(), s.label.clone());
                        Button::new(("snap-export", i))
                            .xsmall()
                            .ghost()
                            .icon(crate::icons::Lucide::Upload)
                            .tooltip("Export to a .kmmsnap file (includes the mod files)")
                            .loading(self.is_pending(&format!("snap-export:{}", s.id)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.export_snapshot_dialog(e_id.clone(), e_label.clone(), cx)
                            }))
                    })
                    .child({
                        let (d_id, d_label) = (s.id.clone(), s.label.clone());
                        Button::new(("snap-delete", i))
                            .xsmall()
                            .ghost()
                            .icon(crate::icons::Lucide::Trash)
                            .tooltip("Delete snapshot")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let id = d_id.clone();
                                this.modal = Some(Modal::confirm(
                                    "Delete snapshot?",
                                    format!("“{d_label}” is deleted for good. Installed mods are not affected."),
                                    "Delete",
                                    true,
                                    move |this, cx| {
                                        let id = id.clone();
                                        this.run(
                                            "snapshot",
                                            "Deleting snapshot",
                                            cx,
                                            move |e, _| e.delete_snapshot(&id),
                                            |this, _, cx| {
                                                this.success("Snapshot deleted.");
                                                this.open_snapshots(cx);
                                            },
                                        );
                                    },
                                ));
                                cx.notify();
                            }))
                    })
                    .child(Button::new(("snap-restore", i)).xsmall().label("Restore").on_click(cx.listener(
                        move |this, _, _, cx| {
                            let id = id.clone();
                            this.modal = Some(Modal::confirm(
                                "Restore snapshot?",
                                format!("Your mods will be set back to “{label}”. A safety snapshot of the current state is taken first."),
                                "Restore",
                                false,
                                move |this, cx| {
                                    let id = id.clone();
                                    this.run("snapshot", "Restoring snapshot", cx, move |e, p| e.restore_snapshot(&id, p), |this, _, _| this.success("Snapshot restored."));
                                },
                            ));
                            cx.notify();
                        },
                    )))
            }))
            .into_any_element()
    }

    fn render_conflicts(&self, view: Option<Arc<ConflictsView>>) -> AnyElement {
        let Some(view) = view else {
            return div()
                .child(gpui_kit::component::spinner::Spinner::new())
                .into_any_element();
        };
        if view.pairs.is_empty() {
            return muted("No enabled mods overwrite each other.").into_any_element();
        }
        let title = |id: &ModId| {
            self.lib
                .mods
                .get(id)
                .map(|m| m.title.clone())
                .unwrap_or_else(|| id.to_string())
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(muted(format!(
                "{} overlapping file(s). The higher mod in the load order wins.",
                view.conflicts.len()
            )))
            .children(view.pairs.iter().map(|p| {
                card()
                    .p(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(div().text_sm().child(format!(
                        "{} overrides {}",
                        title(&p.winner),
                        title(&p.loser)
                    )))
                    .child(faint(format!(
                        "{} file(s), e.g. {}",
                        p.paths,
                        p.sample.first().cloned().unwrap_or_default()
                    )))
            }))
            .when(!view.fully_overridden.is_empty(), |d| {
                d.child(div().text_sm().text_color(c(AMBER)).child(format!(
                    "Fully hidden by higher mods: {}",
                    view.fully_overridden.iter().map(title).collect::<Vec<_>>().join(", ")
                )))
            })
            .into_any_element()
    }

    fn render_inspector(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(Modal::Inspector(ins)) = &self.modal else {
            return div().into_any_element();
        };
        if let Some(e) = &ins.error {
            return muted(e.clone()).into_any_element();
        }
        let Some(page) = &ins.page else {
            return div()
                .child(gpui_kit::component::spinner::Spinner::new())
                .into_any_element();
        };
        let offset = ins.offset;
        let total = page.total;
        let rows = page.rows.clone();
        let blob = ins.blob.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_1().child(muted(format!(
                        "{}–{} of {} entries",
                        if total == 0 { 0 } else { offset + 1 },
                        (offset + rows.len()).min(total),
                        total
                    ))))
                    .child(
                        Button::new("ins-prev")
                            .xsmall()
                            .label("Previous")
                            .disabled(offset == 0)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.inspector_page(offset.saturating_sub(INSPECTOR_PAGE), cx)
                            })),
                    )
                    .child(
                        Button::new("ins-next")
                            .xsmall()
                            .label("Next")
                            .disabled(offset + INSPECTOR_PAGE >= total)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.inspector_page(offset + INSPECTOR_PAGE, cx)
                            })),
                    ),
            )
            .children(rows.into_iter().enumerate().map(|(i, r)| {
                let play = blob
                    .clone()
                    .filter(|_| crate::sounds::is_sound(&r.path))
                    .map(|b| {
                        self.play_button(
                            ("ins-play", i),
                            format!("mod:{b}:{}", r.path),
                            crate::sounds::SoundOrigin::Mod(b),
                            r.path.clone(),
                            None,
                            cx,
                        )
                    });
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .py(px(2.))
                    .child(div().w(px(24.)).flex_none().children(play))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(c(TEXT))
                            .child(r.path),
                    )
                    .child(
                        div()
                            .w(px(80.))
                            .text_xs()
                            .text_color(c(FAINT))
                            .child(format_bytes(r.size)),
                    )
            }))
            .into_any_element()
    }

    fn render_adopt(&mut self, data: Option<Arc<AdoptData>>, cx: &mut Context<Self>) -> AnyElement {
        let Some(data) = data else {
            return div()
                .child(gpui_kit::component::spinner::Spinner::new())
                .into_any_element();
        };
        if data.managers.is_empty() && data.candidates.is_empty() {
            return muted("There are no unmanaged addon files to import.").into_any_element();
        }
        let managers: Vec<AnyElement> = (0..data.managers.len())
            .map(|i| self.render_manager_import(data.clone(), i, cx))
            .collect();
        let list = data.candidates.clone();
        let busy = self.is_pending("adopt");
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(managers)
            .when(!list.is_empty(), |d| {
                d.when(!data.managers.is_empty(), |d| {
                    d.child(div().pt(px(6.)).child(heading("Other addon files")))
                })
                .child(muted("These files are in your addons folder but not managed. Importing keeps their current order and enabled state."))
                .children(list.iter().map(|cand| {
                    card()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .p(px(10.))
                        .child(dot(if cand.enabled { ACCENT } else { FAINT }))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .child(div().text_sm().child(cand.title.clone()))
                                .child(faint(format!("{} · {} · {}", cand.origin, format_bytes(cand.size), if cand.enabled { "loaded" } else { "not loaded" }))),
                        )
                }))
                .child(
                    Button::new("adopt-all")
                        .primary()
                        .label(format!("Import {} item(s)", list.len()))
                        .loading(busy)
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let all = list.clone();
                            this.run_with(
                                "adopt",
                                "Importing existing addons",
                                true,
                                cx,
                                move |e, p| kombini_import::unmanaged::adopt(e, &all, p),
                                |this, report, cx| {
                                    this.success(format!("Imported {} mod(s).", report.adopted.len()));
                                    for (name, why) in report.failed.iter().take(2) {
                                        this.toast(Notification::warning(format!("{name}: {why}")));
                                    }
                                    this.close_modal(cx);
                                },
                            );
                        })),
                )
            })
            .into_any_element()
    }

    /// One other manager's library with its "Import" button.
    fn render_manager_import(
        &mut self,
        data: Arc<AdoptData>,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        const SHOWN: usize = 6;
        let held = data.clone();
        let found = &held.managers[index];
        let manager = found.manager;
        let busy = self.is_pending("migrate");
        let mods = &found.doc.mods;
        let profiles = found.doc.profiles.len();
        let summary = format!(
            "{} mod(s) · {} enabled · {} profile(s)",
            mods.len(),
            found.enabled_count(),
            profiles
        );
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
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().text_color(c(TEXT)).child(manager.name()))
                            .child(faint(summary))
                            .child(faint(format!("Read from {}", found.location))),
                    )
                    .child(
                        Button::new(SharedString::from(format!("migrate-{index}")))
                            .primary()
                            .label(format!("Import from {}", manager.short()))
                            .loading(busy)
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let found = data.managers[index].clone();
                                let name = found.manager.short();
                                this.run_with(
                                    "migrate",
                                    format!("Importing from {name}"),
                                    true,
                                    cx,
                                    move |e, p| kombini_import::managers::migrate(e, &found, p),
                                    move |this, r, cx| {
                                        let mut msg = format!(
                                            "Imported {} mod(s) and {} profile(s) from {name}.",
                                            r.imported, r.profiles
                                        );
                                        if r.already > 0 {
                                            msg.push_str(&format!(" {} were already in your library.", r.already));
                                        }
                                        this.success(msg);
                                        if let Some(dir) = &r.backup {
                                            this.toast(Notification::info(format!(
                                                "{name}'s {} file(s) were moved out of the game folder to {}",
                                                r.moved,
                                                dir.display()
                                            )));
                                        }
                                        for w in r.warnings.iter().take(3) {
                                            this.toast(Notification::warning(w.clone()));
                                        }
                                        this.close_modal(cx);
                                    },
                                );
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .children(mods.iter().take(SHOWN).map(|m| {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(dot(if m.enabled == Some(true) { ACCENT } else { FAINT }))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(c(MUTED))
                                    .child(m.name.clone()),
                            )
                    }))
                    .when(mods.len() > SHOWN, |d| {
                        d.child(faint(format!("and {} more", mods.len() - SHOWN)))
                    }),
            )
            .child(faint(format!(
                "Mods keep their names, GameBanana links, order and enabled state, and each {} profile becomes a Kombini profile. Its files move out of the game folder into a backup so nothing loads twice; {} will show them as missing.",
                manager.short(),
                manager.short()
            )))
            .into_any_element()
    }
}

fn query(offset: usize) -> EntryQuery {
    EntryQuery {
        filter: String::new(),
        offset,
        limit: INSPECTOR_PAGE,
        sort: SortKey::Path,
        descending: false,
    }
}
