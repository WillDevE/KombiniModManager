//! Downloads tab, persistence (`downloads.json` in the data dir) and the
//! hand-off from a finished download to the installer.

use crate::app::AppView;
use crate::theme::*;
use crate::widgets::*;
use gpui_kit::component::button::*;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kombini_download::{DownloadItem, Status};
use kombini_engine::AppDirs;
use kombini_engine::install::InstallMeta;

fn file(dirs: &AppDirs) -> std::path::PathBuf {
    dirs.data.join("downloads.json")
}

pub fn load_persisted(dirs: &AppDirs) -> Vec<DownloadItem> {
    std::fs::read(file(dirs))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn persist(dirs: &AppDirs, items: &[DownloadItem]) -> std::io::Result<()> {
    // Only unfinished work (and finished files awaiting install) survives a restart.
    let keep: Vec<&DownloadItem> = items
        .iter()
        .filter(|d| !matches!(d.status, Status::Cancelled))
        .collect();
    std::fs::create_dir_all(&dirs.data)?;
    let tmp = file(dirs).with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&keep).unwrap_or_default())?;
    std::fs::rename(tmp, file(dirs))
}

impl AppView {
    /// Called from the tick when the download manager reported a change.
    pub fn downloads_changed(&mut self, cx: &mut Context<Self>) {
        let Some(dm) = self.downloads.clone() else {
            return;
        };
        let items = dm.items();
        let dirs = self.dirs.clone();
        let snapshot = items.clone();
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = persist(&dirs, &snapshot) {
                    tracing::warn!(error = %e, "could not persist downloads");
                }
            })
            .detach();
        for item in &items {
            if item.status == Status::Completed
                && !self.dl_installing.contains(&item.spec.id)
                && let Some(path) = item.path.clone()
            {
                self.dl_installing.insert(item.spec.id.clone());
                self.install_download(item.clone(), path, cx);
            }
            if item.status == Status::Failed
                && let Some(err) = &item.error
                && self
                    .dl_installing
                    .insert(format!("failed:{}", item.spec.id))
            {
                self.error(&format!("Downloading {}", item.spec.title), err);
            }
        }
        self.dl_items = items;
    }

    fn install_download(
        &mut self,
        item: DownloadItem,
        path: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        let mut meta: InstallMeta =
            serde_json::from_value(item.spec.meta.clone()).unwrap_or_default();
        let keep = self.settings.keep_archives;
        if keep {
            meta.archive_path = Some(path.display().to_string());
        }
        let dm = self.downloads.clone();
        let id = item.spec.id.clone();
        let key = meta
            .remote_id
            .and_then(|r| {
                self.lib
                    .mods
                    .values()
                    .find(|m| m.remote_id == Some(r))
                    .map(|m| format!("update:{}", m.id))
            })
            .unwrap_or_else(|| format!("install:{id}"));
        self.run_with(
            key,
            format!("Installing {}", item.spec.title),
            true,
            cx,
            move |e, p| {
                let report = e.install_archive(&path, meta, p)?;
                if !keep {
                    let _ = std::fs::remove_file(&path);
                }
                Ok(report)
            },
            move |this, report, cx| {
                if let Some(dm) = dm {
                    dm.remove(&id);
                }
                this.dl_dirty
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                this.after_install(report, cx);
            },
        );
    }

    pub fn render_downloads(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.dl_items.clone();
        if items.is_empty() {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .child(heading("No downloads"))
                .child(muted(
                    "Mods you install from Discover show their progress here.",
                ))
                .into_any_element();
        }
        let finished = items.iter().any(|d| d.status.is_finished());
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.))
                    .px(px(16.))
                    .py(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .child(muted(format!("{} item(s)", items.len()))),
                    )
                    .child(
                        Button::new("dl-clear")
                            .small()
                            .ghost()
                            .label("Clear finished")
                            .disabled(!finished)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(dm) = &this.downloads {
                                    for d in dm.items() {
                                        if d.status.is_finished() {
                                            dm.remove(&d.spec.id);
                                        }
                                    }
                                }
                                this.dl_dirty
                                    .store(true, std::sync::atomic::Ordering::Relaxed);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("dl-folder")
                            .small()
                            .ghost()
                            .label("Open folder")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let dir = this.dirs.downloads();
                                this.open_folder(&dir, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .id("dl-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(16.))
                    .pb(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .children({
                        let mut v = Vec::new();
                        for (i, d) in items.into_iter().enumerate() {
                            v.push(self.render_download(i, d));
                        }
                        v
                    }),
            )
            .into_any_element()
    }

    fn render_download(&self, i: usize, d: DownloadItem) -> Div {
        let id = d.spec.id.clone();
        let installing = self.dl_installing.contains(&id) && d.status == Status::Completed;
        let fraction = d
            .total
            .filter(|t| *t > 0)
            .map(|t| d.received as f32 / t as f32);
        let (status_text, color) = match d.status {
            Status::Queued => ("Queued".to_owned(), FAINT),
            Status::Downloading => (
                format!(
                    "{} of {} · {}/s",
                    format_bytes(d.received),
                    d.total.map(format_bytes).unwrap_or_else(|| "?".into()),
                    format_bytes(d.bytes_per_sec)
                ),
                INFO,
            ),
            Status::Paused => ("Paused".to_owned(), AMBER),
            Status::Verifying => ("Verifying…".to_owned(), INFO),
            Status::Completed if installing => ("Installing…".to_owned(), ACCENT),
            Status::Completed => ("Downloaded".to_owned(), ACCENT),
            Status::Failed => (
                d.error
                    .as_ref()
                    .map(|e| e.message.clone())
                    .unwrap_or_else(|| "Failed".into()),
                CORAL,
            ),
            Status::Cancelled => ("Cancelled".to_owned(), FAINT),
        };
        let dm = self.downloads.clone();
        let mut actions = div().flex().gap(px(6.));
        let act =
            |label: &'static str, key: usize, f: fn(&kombini_download::DownloadManager, &str)| {
                let dm = dm.clone();
                let id = id.clone();
                Button::new((label, key))
                    .xsmall()
                    .ghost()
                    .label(label)
                    .on_click(move |_, _, _| {
                        if let Some(dm) = &dm {
                            f(dm, &id);
                        }
                    })
            };
        match d.status {
            Status::Downloading | Status::Queued => {
                actions = actions
                    .child(act("Pause", i, |dm, id| dm.pause(id)))
                    .child(act("Cancel", i, |dm, id| dm.cancel(id)));
            }
            Status::Paused => {
                actions = actions
                    .child(act("Resume", i, |dm, id| dm.resume(id)))
                    .child(act("Cancel", i, |dm, id| dm.cancel(id)));
            }
            Status::Failed => {
                actions = actions
                    .child(act("Retry", i, |dm, id| dm.resume(id)))
                    .child(act("Remove", i, |dm, id| {
                        dm.remove(id);
                    }));
            }
            Status::Cancelled => {
                actions = actions.child(act("Remove", i, |dm, id| {
                    dm.remove(id);
                }));
            }
            Status::Completed | Status::Verifying => {}
        }
        card()
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(dot(color))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().truncate().child(d.spec.title.clone()))
                            .child(faint(d.spec.file_name.clone())),
                    )
                    .child(div().text_xs().text_color(c(color)).child(status_text))
                    .child(actions),
            )
            .when(
                matches!(d.status, Status::Downloading | Status::Verifying) || installing,
                |el| {
                    el.child(progress_bar(
                        SharedString::from(format!("dl-progress-{i}")),
                        if installing { None } else { fraction },
                    ))
                },
            )
    }

    #[allow(dead_code)]
    fn download_note(&mut self, msg: &str) {
        self.toast(Notification::info(msg.to_owned()));
    }
}
