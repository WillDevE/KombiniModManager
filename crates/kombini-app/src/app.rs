//! Root view: boot sequence, shared state, the operation runner and the shell
//! (top bar, banners, tabs, status bar). Each tab renders from its own module.
//!
//! Threading rule: every engine/network/file call runs on the background
//! executor via [`AppView::run`] / [`AppView::background`]; the UI thread only
//! clones in-memory snapshots.

use crate::discover::Discover;
use crate::launch::LaunchUi;
use crate::modal::Modal;
use crate::ops::{HISTORY, Op, OpProgress, OpRecord};
use crate::theme::*;
use crate::widgets::*;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{Disableable, Sizable, WindowExt};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kombini_core::{AppError, ErrorCode, Level, ModId};
use kombini_download::{DownloadItem, DownloadManager};
use kombini_engine::health::HealthReport;
use kombini_engine::settings::Settings;
use kombini_engine::{AppDirs, Engine, EngineConfig, SystemProcess, watch::AddonsWatcher};
use kombini_gamebanana::{Client, ImageCache};
use kombini_storage::LibraryState;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Mods,
    Heroes,
    Discover,
    Customize,
    Downloads,
    Settings,
}

pub enum Boot {
    Loading(String),
    NeedGame {
        detected: Vec<PathBuf>,
        error: Option<String>,
    },
    Ready,
}

/// Bootstrap config the engine cannot hold (it needs the game path to open).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub game_path: Option<PathBuf>,
}

impl AppConfig {
    fn path(dirs: &AppDirs) -> PathBuf {
        dirs.data.join("app.json")
    }
    pub fn load(dirs: &AppDirs) -> Self {
        std::fs::read(Self::path(dirs))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    pub fn save(&self, dirs: &AppDirs) -> std::io::Result<()> {
        std::fs::create_dir_all(&dirs.data)?;
        let tmp = Self::path(dirs).with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, Self::path(dirs))
    }
}

pub enum Thumb {
    Loading,
    Ready(PathBuf),
    Failed,
}

/// Automatic mod update checks run at most this often (manual ones always).
pub const UPDATE_CHECK_COOLDOWN_SECS: i64 = 6 * 60 * 60;
/// Focus changes check the mods at most this often.
const AUTO_REPAIR_THROTTLE: Duration = Duration::from_secs(20);
/// The "Check for updates" button can be pressed again after this.
const MANUAL_UPDATE_COOLDOWN: Duration = Duration::from_secs(60);

pub struct AppView {
    pub boot: Boot,
    pub dirs: AppDirs,
    pub config: AppConfig,
    pub engine: Option<Arc<Engine>>,
    pub gb: Arc<Client>,
    pub images: Arc<ImageCache>,
    pub downloads: Option<Arc<DownloadManager>>,
    pub tab: Tab,
    pub lib: Arc<LibraryState>,
    pub settings: Settings,
    pub health: Option<HealthReport>,
    pub selected: Option<ModId>,
    /// GameBanana changelogs by submission id (details pane).
    pub changelogs: std::collections::HashMap<u64, crate::versions::Changelog>,
    /// Hero shown in the Heroes tab.
    pub locker_hero: Option<String>,
    /// The game's hero list for the Heroes grid (empty when unreadable).
    pub roster: Option<Arc<kombini_engine::roster::Roster>>,
    pub roster_loading: bool,
    pub customize: crate::customize::CustomizeState,
    pub audio: crate::audio::Audio,
    /// Sound preview playing now, with a token for its end timer.
    pub playing: Option<(String, u64)>,
    pub play_token: u64,
    pub mod_sounds:
        std::collections::HashMap<ModId, (Vec<kombini_core::BlobHash>, crate::sounds::ModSounds)>,
    /// Installed game version and update dates (see `age.rs`).
    pub timeline: Option<kombini_engine::timeline::GameTimeline>,
    /// Latest mod issue scan (see `issues.rs`).
    pub issues: crate::issues::IssuesState,
    /// Ctrl/Shift-click multi-selection in My Mods.
    pub multi: BTreeSet<ModId>,
    /// When launch options were last auto-saved (drives the "Saved" hint).
    pub launch_saved_at: Option<Instant>,
    /// Row being dragged (dimmed in the list).
    pub dragging: Option<ModId>,
    pub list_filter: crate::mods::ListFilter,
    pub filter_input: Entity<InputState>,
    pub rows: Vec<ModId>,
    pub list_scroll: UniformListScrollHandle,
    /// Smooth wheel scrolling state of each scroll area.
    pub smooth: crate::smooth_scroll::SmoothAreas,
    pub pending: HashSet<String>,
    pub ops: Vec<Op>,
    pub history: Vec<OpRecord>,
    pub toasts: Vec<Notification>,
    pub modal: Option<Modal>,
    pub launch: LaunchUi,
    pub discover: Discover,
    pub dl_items: Vec<DownloadItem>,
    pub dl_installing: HashSet<String>,
    pub dl_dirty: Arc<AtomicBool>,
    pub fs_dirty: Arc<AtomicBool>,
    pub thumbs: HashMap<String, Thumb>,
    pub text_input: Entity<InputState>,
    pub launch_input: Entity<InputState>,
    /// Profile whose launch options are loaded in `launch_input`.
    pub launch_input_for: Option<kombini_core::ProfileId>,
    pub watcher: Option<AddonsWatcher>,
    pub foreign_count: usize,
    pub other_managers: Vec<String>,
    /// Mods switched on for the game (title-bar toggle, kept after closing).
    pub mods_active: bool,
    /// Hero of every mod (own, or its GameBanana page's), rebuilt on refresh.
    pub heroes_by_mod: std::collections::BTreeMap<ModId, String>,
    /// An automatic or manual repair check is running.
    pub repairing: bool,
    /// Last automatic repair check, to throttle focus-triggered checks.
    pub last_repair_check: Option<Instant>,
    /// Last press of the status-bar "Check for updates" button.
    pub last_manual_update_check: Option<Instant>,
    pub update_count: usize,
    pub last_tick_busy: bool,
    pub inbox: crate::instance::Inbox,
    pub first_frame_logged: bool,
    _ticker: Task<()>,
    _subs: Vec<Subscription>,
}

impl AppView {
    pub fn new(inbox: crate::instance::Inbox, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let dirs = AppDirs::from_env().unwrap_or_else(|_| AppDirs {
            data: std::env::temp_dir().join("kombini-data"),
            cache: std::env::temp_dir().join("kombini-cache"),
        });
        let config = AppConfig::load(&dirs);
        let images = Arc::new(ImageCache::new(&dirs.cache.join("images")));
        let filter_input = cx
            .new(|cx| InputState::new(window, cx).placeholder("Filter mods (name, hero, author)"));
        let text_input = cx.new(|cx| InputState::new(window, cx));
        let launch_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Default launch options (e.g. -novid -high)")
        });
        let discover_input = cx
            .new(|cx| InputState::new(window, cx).placeholder("Search GameBanana (2+ characters)"));
        let mut subs = Vec::new();
        // Coming back to Kombini (from the game, Explorer, another manager):
        // check that the mods are still in place.
        subs.push(
            cx.observe_window_activation(window, |this: &mut Self, window, cx| {
                if window.is_window_active() {
                    this.auto_repair(cx);
                }
            }),
        );
        subs.push(cx.subscribe_in(
            &filter_input,
            window,
            |this: &mut Self, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.recompute_rows(cx);
                    cx.notify();
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &discover_input,
            window,
            |this: &mut Self, state, ev: &InputEvent, _, cx| match ev {
                InputEvent::Change => {
                    let text = state.read(cx).value().to_string();
                    this.discover_search_changed(text, cx);
                }
                InputEvent::PressEnter { .. } => this.discover_fetch(cx),
                _ => {}
            },
        ));
        subs.push(cx.subscribe_in(
            &text_input,
            window,
            |this: &mut Self, _, ev: &InputEvent, _, cx| match ev {
                InputEvent::PressEnter { .. } => this.submit_prompt(cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        ));
        subs.push(cx.subscribe_in(
            &launch_input,
            window,
            |this: &mut Self, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    this.save_launch_options(cx);
                }
            },
        ));
        // Automatic update checks while Kombini stays open (cooldown applies).
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(15 * 60))
                    .await;
                if this
                    .update(cx, |this, cx| this.maybe_check_updates(cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        });
        let mut view = Self {
            boot: Boot::Loading("Looking for Deadlock…".into()),
            dirs,
            config,
            engine: None,
            gb: Arc::new(Client::new()),
            images,
            downloads: None,
            tab: Tab::Mods,
            lib: Arc::new(LibraryState::empty()),
            settings: Settings::default(),
            health: None,
            mods_active: true,
            heroes_by_mod: Default::default(),
            repairing: false,
            last_repair_check: None,
            last_manual_update_check: None,
            selected: None,
            locker_hero: None,
            roster: None,
            roster_loading: false,
            customize: Default::default(),
            audio: crate::audio::Audio::new(),
            playing: None,
            play_token: 0,
            mod_sounds: Default::default(),
            changelogs: Default::default(),
            multi: BTreeSet::new(),
            issues: crate::issues::IssuesState::default(),
            timeline: None,
            launch_saved_at: None,
            dragging: None,
            list_filter: crate::mods::ListFilter::All,
            filter_input,
            rows: Vec::new(),
            list_scroll: UniformListScrollHandle::new(),
            smooth: Default::default(),
            pending: HashSet::new(),
            ops: Vec::new(),
            history: Vec::new(),
            toasts: Vec::new(),
            modal: None,
            launch: LaunchUi::default(),
            discover: Discover::new(discover_input),
            dl_items: Vec::new(),
            dl_installing: HashSet::new(),
            dl_dirty: Arc::new(AtomicBool::new(false)),
            fs_dirty: Arc::new(AtomicBool::new(false)),
            thumbs: HashMap::new(),
            text_input,
            launch_input,
            launch_input_for: None,
            watcher: None,
            foreign_count: 0,
            other_managers: Vec::new(),
            update_count: 0,
            last_tick_busy: false,
            inbox,
            first_frame_logged: false,
            _ticker: ticker,
            _subs: subs,
        };
        view.boot(cx);
        view
    }

    // ----------------------------------------------------------------- boot

    fn boot(&mut self, cx: &mut Context<Self>) {
        let configured = self.config.game_path.clone();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let detected: Vec<PathBuf> = kombini_steam::find_deadlock_installs()
                        .into_iter()
                        .map(|i| i.root)
                        .collect();
                    (configured, detected)
                })
                .await;
            this.update(cx, |this, cx| match found {
                (Some(path), _) => this.open_game(path, cx),
                (None, detected) if detected.len() == 1 => {
                    this.open_game(detected[0].clone(), cx);
                }
                (None, detected) => {
                    this.boot = Boot::NeedGame {
                        detected,
                        error: None,
                    };
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn open_game(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.boot = Boot::Loading(format!("Opening {}…", root.display()));
        cx.notify();
        let dirs = self.dirs.clone();
        cx.spawn(async move |this, cx| {
            let root2 = root.clone();
            let opened = cx
                .background_executor()
                .spawn(async move {
                    let root = kombini_steam::validate_game_root(&root2)
                        .map_err(|e| AppError::new(ErrorCode::GameNotFound, e.to_string()))?;
                    Engine::open(EngineConfig {
                        dirs,
                        game_root: root,
                        process: Arc::new(SystemProcess::new()),
                    })
                })
                .await;
            this.update(cx, |this, cx| match opened {
                Ok((engine, report)) => {
                    this.config.game_path = Some(root);
                    if let Err(e) = this.config.save(&this.dirs) {
                        tracing::warn!(error = %e, "could not save app.json");
                    }
                    this.engine_ready(Arc::new(engine), report, cx);
                }
                Err(e) => {
                    let detected = match &this.boot {
                        Boot::NeedGame { detected, .. } => detected.clone(),
                        _ => Vec::new(),
                    };
                    this.boot = Boot::NeedGame {
                        detected,
                        error: Some(e.to_string()),
                    };
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn engine_ready(
        &mut self,
        engine: Arc<Engine>,
        report: kombini_engine::OpenReport,
        cx: &mut Context<Self>,
    ) {
        self.settings = engine.settings();
        let dl_dirty = self.dl_dirty.clone();
        let dm = Arc::new(DownloadManager::new(
            &self.dirs.downloads(),
            self.settings.max_concurrent_downloads as usize,
            move |_| dl_dirty.store(true, Ordering::Relaxed),
        ));
        dm.set_bandwidth_kbps(self.settings.bandwidth_limit_kbps);
        dm.restore(crate::downloads::load_persisted(&self.dirs));
        self.downloads = Some(dm);
        self.dl_dirty.store(true, Ordering::Relaxed);
        let fs_dirty = self.fs_dirty.clone();
        let quiet_engine = engine.clone();
        match AddonsWatcher::start(
            engine.paths(),
            move || quiet_engine.is_quiet(),
            move || fs_dirty.store(true, Ordering::Relaxed),
        ) {
            Ok(w) => self.watcher = Some(w),
            Err(e) => tracing::warn!(error = %e, "addons watcher unavailable"),
        }
        self.engine = Some(engine);
        self.boot = Boot::Ready;
        if let Some(t) = crate::STARTED.get() {
            tracing::info!(
                ms = t.elapsed().as_millis() as u64,
                "startup: library ready"
            );
        }
        self.refresh(cx);
        self.load_bisect(cx);
        self.scan_issues_quietly(cx);
        self.load_timeline(cx);
        for line in &report.recovered {
            self.toast(Notification::info(line.clone()).title("Recovered an interrupted change"));
        }
        if report.corrupt_db_backup.is_some() {
            self.toast(
                Notification::warning(
                    "The library database was damaged and has been reset. A backup was kept.",
                )
                .title("Library reset"),
            );
        }
        if report.game_updated && report.gameinfo_reapplied {
            self.toast(
                Notification::success("Deadlock updated; your mods were re-enabled.")
                    .title("Game update handled"),
            );
        }
        self.maybe_check_updates(cx);
        self.auto_repair(cx);
        cx.notify();
    }

    // ------------------------------------------------------------ refreshes

    /// Re-reads the in-memory library snapshot and schedules health checks.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            return;
        };
        self.lib = Arc::new(engine.state());
        self.heroes_by_mod = crate::heroes::resolve_heroes(self.lib.mods.values());
        self.foreign_count = engine.foreign_files().len();
        self.other_managers = engine.other_managers();
        self.mods_active = engine.mods_active();
        self.update_count = self
            .lib
            .mods
            .values()
            .filter(|m| m.update.available)
            .count();
        if let Some(sel) = &self.selected
            && !self.lib.mods.contains_key(sel)
        {
            self.selected = None;
        }
        self.recompute_rows(cx);
        self.background(
            cx,
            move |e| Ok(e.health()),
            |this, report, cx| {
                this.health = Some(report);
                cx.notify();
            },
        );
        cx.notify();
    }

    pub fn recompute_rows(&mut self, cx: &mut Context<Self>) {
        let needle = self.filter_input.read(cx).value().to_lowercase();
        let outdated = if self.list_filter == crate::mods::ListFilter::Outdated {
            self.outdated_ids()
        } else {
            Default::default()
        };
        self.rows =
            crate::mods::filtered_rows(&self.lib, self.list_filter, needle.trim(), &outdated);
        let lib = self.lib.clone();
        self.multi.retain(|id| lib.mods.contains_key(id));
        if self
            .selected
            .as_ref()
            .is_some_and(|id| !lib.mods.contains_key(id))
        {
            self.selected = None;
        }
    }

    // ------------------------------------------------------------ operations

    /// Runs engine work on the background executor as a tracked operation.
    /// `key` provides double-submit protection: while an operation with the
    /// same key runs, further requests are ignored (and the control renders
    /// its loading state via [`Self::is_pending`]).
    pub fn run<T: Send + 'static>(
        &mut self,
        key: impl Into<String>,
        label: impl Into<String>,
        cx: &mut Context<Self>,
        work: impl FnOnce(&Engine, &OpProgress) -> Result<T, AppError> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        self.run_with(key, label, false, cx, work, done);
    }

    pub fn run_with<T: Send + 'static>(
        &mut self,
        key: impl Into<String>,
        label: impl Into<String>,
        cancellable: bool,
        cx: &mut Context<Self>,
        work: impl FnOnce(&Engine, &OpProgress) -> Result<T, AppError> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        let Some(engine) = self.engine.clone() else {
            self.toast(Notification::error("Choose your Deadlock folder first."));
            cx.notify();
            return;
        };
        let key = key.into();
        let label = label.into();
        if !self.pending.insert(key.clone()) {
            return;
        }
        let progress = OpProgress::default();
        self.ops.push(Op {
            key: key.clone(),
            label: label.clone(),
            started: Instant::now(),
            progress: progress.clone(),
            cancellable,
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let result = cx
                .background_executor()
                .spawn(async move { work(&engine, &progress) })
                .await;
            this.update(cx, |this, cx| {
                this.pending.remove(&key);
                this.ops.retain(|o| o.key != key);
                let ok = result.is_ok();
                let message = match &result {
                    Ok(_) => "Done".to_owned(),
                    Err(e) => e.to_string(),
                };
                this.history.insert(
                    0,
                    OpRecord {
                        label: label.clone(),
                        ok,
                        message,
                        took: started.elapsed(),
                        at: Instant::now(),
                    },
                );
                this.history.truncate(HISTORY);
                this.refresh(cx);
                match result {
                    Ok(value) => done(this, value, cx),
                    Err(e) if e.code == ErrorCode::Cancelled => {
                        this.toast(Notification::info(format!("{label} was cancelled")));
                    }
                    Err(e) => this.error(&label, &e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Untracked background work (reads, health checks). Errors are logged.
    pub fn background<T: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        work: impl FnOnce(&Engine) -> Result<T, AppError> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        let Some(engine) = self.engine.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { work(&engine) })
                .await;
            this.update(cx, |this, cx| match result {
                Ok(v) => done(this, v, cx),
                Err(e) => tracing::warn!(error = %e, "background task failed"),
            })
            .ok();
        })
        .detach();
    }

    pub fn is_pending(&self, key: &str) -> bool {
        self.pending.contains(key)
    }

    pub fn toast(&mut self, note: Notification) {
        self.toasts.push(note);
    }

    pub fn success(&mut self, msg: impl Into<SharedString>) {
        self.toast(Notification::success(msg));
    }

    /// Actionable error toast: what failed, why, and what to do.
    pub fn error(&mut self, what: &str, e: &AppError) {
        tracing::warn!(code = e.code.as_str(), error = %e, "{what} failed");
        let mut msg = e.message.clone();
        if let Some(hint) = hint(e.code) {
            msg.push('\n');
            msg.push_str(hint);
        }
        self.toast(
            Notification::error(msg)
                .title(format!("{what} failed"))
                .autohide(false),
        );
    }

    // ----------------------------------------------------------------- tick

    /// 10 Hz: samples progress, polls downloads and the addons watcher.
    fn tick(&mut self, cx: &mut Context<Self>) {
        let mut repaint = !self.ops.is_empty();
        if self.dl_dirty.swap(false, Ordering::Relaxed) {
            self.downloads_changed(cx);
            repaint = true;
        }
        if self.fs_dirty.swap(false, Ordering::Relaxed) && self.engine.is_some() {
            self.run(
                "rescan",
                "Checking the addons folder",
                cx,
                |e, _| e.rescan(),
                |this, report, _| {
                    if report.missing_files > 0 {
                        this.toast(
                            Notification::warning(format!(
                                "{} mod file(s) were removed outside the app.",
                                report.missing_files
                            ))
                            .title("Addons changed"),
                        );
                    }
                },
            );
        }
        if self
            .launch_saved_at
            .is_some_and(|t| t.elapsed() > Duration::from_secs(3))
        {
            self.launch_saved_at = None;
            repaint = true;
        }
        if self.launch.phase.busy() {
            repaint = true;
        }
        if matches!(self.boot, Boot::Ready) {
            for arg in self.inbox.drain() {
                self.handle_external(&arg, cx);
                repaint = true;
            }
        }
        if repaint || self.last_tick_busy {
            cx.notify();
        }
        self.last_tick_busy = repaint;
    }

    /// A deep link, file path or share code from the command line or a
    /// second instance. Anything that changes more than a download asks first.
    pub fn handle_external(&mut self, arg: &str, cx: &mut Context<Self>) {
        use kombini_gamebanana::links::InstallRequest as R;
        cx.activate(true);
        let Some(request) = kombini_gamebanana::links::parse(arg) else {
            tracing::info!(arg, "ignored unrecognized argument");
            return;
        };
        match request {
            R::GameBanana {
                submission_id,
                file_id,
            } => self.queue_gamebanana_install(submission_id, file_id, cx),
            R::Collection { collection_id } => self.open_collection(collection_id, cx),
            R::Archive { path } => self.install_local(vec![path], cx),
            R::ArchiveUrl { url } => {
                let body = format!(
                    "{url}\n\nThis is not a GameBanana link. Only continue if you trust the site."
                );
                self.modal = Some(Modal::confirm(
                    "Download from this link?",
                    body,
                    "Download",
                    false,
                    move |this, cx| this.download_url(url.clone(), cx),
                ));
            }
            R::ProfileCode { text } => {
                self.modal = Some(Modal::confirm(
                    "Import a shared profile?",
                    "A new profile is created and activated from this code. Mods you don't have are downloaded from GameBanana.",
                    "Import",
                    false,
                    move |this, cx| this.import_profile_code(text.clone(), cx),
                ));
            }
            R::Interchange { path } => {
                let body = format!("{}\n\nMods you already have are skipped.", path.display());
                self.modal = Some(Modal::confirm(
                    "Import this mod bundle?",
                    body,
                    "Import",
                    false,
                    move |this, cx| {
                        let path = path.clone();
                        this.run_with(
                            "interchange",
                            "Importing bundle",
                            true,
                            cx,
                            move |e, p| kombini_import::interchange::import(e, &path, None, p),
                            |this, r, _| {
                                this.success(format!(
                                    "Imported {} mod(s), skipped {}.",
                                    r.imported.len(),
                                    r.skipped.len()
                                ));
                            },
                        );
                    },
                ));
            }
        }
        cx.notify();
    }

    pub fn import_profile_code(&mut self, text: String, cx: &mut Context<Self>) {
        self.run(
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
                    this.queue_gamebanana_install(m.submission_id, m.file_id, cx);
                }
            },
        );
    }

    /// Queues a non-GameBanana archive URL (after the user confirmed it).
    pub fn download_url(&mut self, url: String, cx: &mut Context<Self>) {
        let Some(dm) = self.downloads.clone() else {
            return;
        };
        let name = url
            .rsplit('/')
            .next()
            .and_then(|n| n.split('?').next())
            .filter(|n| !n.is_empty())
            .unwrap_or("download.zip")
            .to_owned();
        let meta = kombini_engine::install::InstallMeta {
            source_key: Some(url.clone()),
            enable: Some(true),
            ..Default::default()
        };
        dm.enqueue(kombini_download::DownloadSpec {
            id: format!("url-{}", kombini_storage::random_hex(4)),
            url,
            file_name: kombini_download::safe_file_name(&name),
            title: name,
            expected_md5: None,
            expected_size: None,
            meta: serde_json::to_value(&meta).unwrap_or_default(),
        });
        self.toast(Notification::info("Download started."));
        cx.notify();
    }

    // ------------------------------------------------------------- actions

    pub fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        if tab == Tab::Discover && self.discover.page.is_none() && !self.discover.loading {
            self.discover_fetch(cx);
        }
        cx.notify();
    }

    pub fn pick_game_folder(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Select the Deadlock folder".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(p) = paths.into_iter().next()
            {
                this.update(cx, |this, cx| this.open_game(p, cx)).ok();
            }
        })
        .detach();
    }

    pub fn install_files_dialog(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Install mods".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                this.update(cx, |this, cx| this.install_local(paths, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// Installs dropped/picked archives, VPKs or folders (one mod per item;
    /// loose VPK sets dropped together become one mod).
    pub fn install_local(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let all_vpk = paths
            .iter()
            .all(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("vpk")));
        let batches: Vec<Vec<PathBuf>> = if all_vpk {
            vec![paths]
        } else {
            paths.into_iter().map(|p| vec![p]).collect()
        };
        for batch in batches {
            let name = batch[0]
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let key = format!("install:{}", batch[0].display());
            self.run_with(
                key,
                format!("Installing {name}"),
                true,
                cx,
                move |e, p| {
                    e.install_paths(&batch, kombini_engine::install::InstallMeta::default(), p)
                },
                |this, report, cx| this.after_install(report, cx),
            );
        }
    }

    pub fn after_install(
        &mut self,
        report: kombini_engine::install::InstallReport,
        cx: &mut Context<Self>,
    ) {
        let verb = if report.replaced {
            "Updated"
        } else {
            "Installed"
        };
        let msg = format!(
            "{verb} {} ({} file(s), {}).",
            report.title,
            report.files,
            format_bytes(report.bytes)
        );
        self.toast(Notification::success(msg));
        for w in report.warnings.iter().take(2) {
            self.toast(Notification::warning(w.clone()));
        }
        if report.safety.iter().any(|f| f.level == Level::Error) {
            self.toast(
                Notification::warning(
                    "This mod contains files flagged by the safety check. Review it before enabling.",
                )
                .title(report.title.clone()),
            );
        }
        // Select it for when the user visits My Mods, but stay on the
        // current tab. Several variants: ask which one right away.
        if report.roots > 1 && self.modal.is_none() {
            self.modal = Some(Modal::Variants(report.mod_id.clone()));
        }
        self.selected = Some(report.mod_id);
        self.scan_issues_quietly(cx);
        cx.notify();
    }

    /// Enables/disables mods; enabling a second skin for a hero asks to
    /// swap first (Settings → "Ask before stacking skins").
    pub fn set_enabled(&mut self, ids: Vec<ModId>, on: bool, cx: &mut Context<Self>) {
        if on
            && self.settings.hero_swap_prompt
            && self.modal.is_none()
            && let [id] = ids.as_slice()
        {
            let others = self.other_enabled_skins(id);
            if !others.is_empty() {
                self.modal = Some(Modal::HeroSwap {
                    id: id.clone(),
                    others,
                });
                cx.notify();
                return;
            }
        }
        self.set_enabled_now(ids, on, cx);
    }

    pub fn set_enabled_now(&mut self, ids: Vec<ModId>, on: bool, cx: &mut Context<Self>) {
        if ids.is_empty() {
            return;
        }
        let key = if ids.len() == 1 {
            format!("toggle:{}", ids[0])
        } else {
            "toggle:bulk".to_owned()
        };
        let count = ids.len();
        let label = if on { "Enabling" } else { "Disabling" };
        self.run(
            key,
            format!("{label} {count} mod(s)"),
            cx,
            move |e, _| e.set_enabled(&ids, on),
            move |this, report, _| {
                if count > 1 {
                    this.success(report.summary);
                }
                for w in report.warnings.iter().take(2) {
                    this.toast(Notification::warning(w.clone()));
                }
            },
        );
    }

    /// Switches mods on or off for the game; stays that way after closing.
    pub fn set_mods_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.is_pending("mods-active") {
            return;
        }
        self.mods_active = active;
        self.run(
            "mods-active",
            if active {
                "Switching mods on"
            } else {
                "Switching mods off"
            },
            cx,
            move |e, _| e.set_mods_active(active),
            move |this, _, _| {
                this.success(if active {
                    "Mods are on. Deadlock starts with them, even after you close Kombini."
                        .to_owned()
                } else {
                    "Mods are off. Deadlock starts without them until you switch them back on."
                        .to_owned()
                })
            },
        );
    }

    /// Automatic check that enabled mods are in the game (startup, window
    /// focus, after the game closes). Quiet unless it had to fix something.
    pub fn auto_repair(&mut self, cx: &mut Context<Self>) {
        if !self.settings.auto_reapply_gameinfo
            || self
                .last_repair_check
                .is_some_and(|t| t.elapsed() < AUTO_REPAIR_THROTTLE)
        {
            return;
        }
        self.last_repair_check = Some(Instant::now());
        self.repair(cx);
    }

    /// Re-reads the addons folders and puts back whatever is out of place
    /// (missing files of enabled mods, gameinfo.gi). Quiet unless it fixed
    /// something.
    pub fn repair(&mut self, cx: &mut Context<Self>) {
        if self.repairing
            || self.engine.is_none()
            || self.launch.phase.busy()
            || !self.ops.is_empty()
        {
            return;
        }
        self.repairing = true;
        cx.notify();
        let Some(engine) = self.engine.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    engine.rescan()?;
                    engine.repair_if_needed(&kombini_engine::NoProgress)
                })
                .await;
            this.update(cx, |this, cx| {
                this.repairing = false;
                match result {
                    Ok(Some(report)) => {
                        this.toast(
                            Notification::success(report.summary.clone()).title("Mods repaired"),
                        );
                        for w in report.warnings.iter().take(2) {
                            this.toast(Notification::warning(w.clone()));
                        }
                    }
                    Ok(None) => {}
                    Err(e) => tracing::warn!(error = %e, "automatic repair failed"),
                }
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    /// The status-bar button: checks now, then rests for a minute.
    pub fn manual_update_check(&mut self, cx: &mut Context<Self>) {
        if self
            .last_manual_update_check
            .is_some_and(|t| t.elapsed() < MANUAL_UPDATE_COOLDOWN)
        {
            return;
        }
        self.last_manual_update_check = Some(Instant::now());
        self.check_updates(true, cx);
        // Re-enable the button when the minute is up.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(MANUAL_UPDATE_COOLDOWN).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    /// Checks for mod updates if the setting is on and the last automatic
    /// check is older than [`UPDATE_CHECK_COOLDOWN_SECS`].
    pub fn maybe_check_updates(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.as_ref() else {
            return;
        };
        if !self.settings.check_updates_on_startup || self.is_pending("updates") {
            return;
        }
        let due = engine
            .last_update_check()
            .is_none_or(|t| kombini_storage::now() - t >= UPDATE_CHECK_COOLDOWN_SECS);
        if due {
            self.check_updates(false, cx);
        }
    }

    pub fn check_updates(&mut self, manual: bool, cx: &mut Context<Self>) {
        let gb = self.gb.clone();
        self.run(
            "updates",
            "Checking for updates",
            cx,
            move |e, p| {
                let result = crate::discover::check_updates(e, &gb, p)?;
                let _ = e.set_last_update_check(kombini_storage::now());
                Ok(result)
            },
            move |this, (found, failed), _| {
                if found > 0 {
                    this.toast(
                        Notification::info(format!("{found} mod update(s) available."))
                            .title("Updates"),
                    );
                } else if manual {
                    this.success("All mods are up to date.");
                }
                if failed > 0 && manual {
                    this.toast(Notification::warning(format!(
                        "{failed} mod(s) could not be checked (network)."
                    )));
                }
            },
        );
    }

    pub fn save_settings(&mut self, settings: Settings, cx: &mut Context<Self>) {
        let dm = self.downloads.clone();
        self.run(
            "settings",
            "Saving settings",
            cx,
            move |e, _| e.set_settings(settings),
            move |this, saved, cx| {
                if let Some(dm) = dm {
                    dm.set_max_concurrent(saved.max_concurrent_downloads as usize);
                    dm.set_bandwidth_kbps(saved.bandwidth_limit_kbps);
                }
                this.settings = saved;
                cx.notify();
            },
        );
    }

    pub fn open_folder(&mut self, path: &Path, cx: &mut Context<Self>) {
        if path.exists() {
            cx.reveal_path(path);
        } else {
            self.toast(Notification::warning(format!(
                "{} does not exist yet.",
                path.display()
            )));
        }
    }

    // --------------------------------------------------------------- render

    fn render_topbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let profile_name = self.lib.profile().name.clone();
        let enabled = self.lib.profile().enabled.len();
        let busy = self.launch.phase.busy();
        let (status_text, status_color) = self.status_pill();
        let status_busy = busy || !self.ops.is_empty();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(12.))
            .h(px(TOPBAR_H))
            .px(px(16.))
            .bg(c(PANEL))
            .border_b_1()
            .border_color(c(BORDER))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div().size(px(22.)).rounded(px(6.)).bg(c(PRIMARY)).child(
                            div()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(c(0xffffff))
                                .child("K"),
                        ),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c(TEXT))
                            .child("Kombini"),
                    ),
            )
            .child(
                Button::new("profile-switch")
                    .small()
                    .text_label(format!("{profile_name} · {enabled} on"))
                    .dropdown_caret(true)
                    .tooltip("Switch, create or share profiles")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.modal = Some(Modal::Profiles);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(px(999.))
                    .bg(ca(status_color, 0.12))
                    .child(if status_busy {
                        crate::anim::breathe(
                            dot(status_color),
                            "status-dot",
                            0.25,
                            Duration::from_millis(1400),
                        )
                        .into_any_element()
                    } else {
                        dot(status_color).into_any_element()
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(c(status_color))
                            .child(status_text),
                    ),
            )
            .child(div().flex_1())
            .child({
                let on = self.mods_active;
                let switching = self.is_pending("mods-active");
                let can_switch = !(switching || busy || self.engine.is_none());
                let mode = if on {
                    kombini_engine::launch::LaunchMode::Modded
                } else {
                    kombini_engine::launch::LaunchMode::Vanilla
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                div()
                                    .id("mods-active-label")
                                    .when(can_switch, |d| {
                                        d.cursor_pointer().on_click(cx.listener(
                                            move |this, _, _, cx| this.set_mods_active(!on, cx),
                                        ))
                                    })
                                    .text_sm()
                                    .text_color(c(if on { TEXT } else { MUTED }))
                                    .child(ctl_text("Mods")),
                            )
                            .child(
                                gpui_kit::component::switch::Switch::new("mods-active-switch")
                                    .checked(on)
                                    .disabled(!can_switch)
                                    .tooltip(if on {
                                        "Mods on: they load when Deadlock starts, even after you close Kombini."
                                    } else {
                                        "Mods off: Deadlock starts without them, even after you close Kombini. They stay installed."
                                    })
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.set_mods_active(*checked, cx)
                                    })),
                            ),
                    )
                    .child(vsep())
                    .child(
                        Button::new("launch-game")
                            .primary()
                            .min_w(px(140.))
                            .icon(crate::icons::Lucide::Gamepad2)
                            .text_label(self.launch.button_label())
                            .tooltip(if on {
                                format!("Start Deadlock with {enabled} mod(s)")
                            } else {
                                "Start Deadlock without mods".to_owned()
                            })
                            .loading(busy)
                            .disabled(busy || switching || self.engine.is_none())
                            .on_click(cx.listener(move |this, _, _, cx| this.start_launch(mode, cx))),
                    )
            })
    }

    fn status_pill(&self) -> (String, u32) {
        if self.launch.phase.busy() {
            return (self.launch.phase_text(), INFO);
        }
        if !self.ops.is_empty() {
            return ("Working…".into(), INFO);
        }
        match self.health.as_ref().and_then(|h| h.worst) {
            Some(Level::Error) => ("Needs attention".into(), CORAL),
            _ if !self.mods_active => ("Mods off".into(), MUTED),
            Some(Level::Warning) => ("Check warnings".into(), AMBER),
            _ => ("Ready".into(), ACCENT),
        }
    }

    fn render_tabs(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mods = self.lib.mods.len();
        let active_dl = self
            .dl_items
            .iter()
            .filter(|d| d.status.is_active())
            .count();
        let tabs = [
            (
                Tab::Mods,
                "My Mods".to_owned(),
                Some(mods),
                crate::icons::Lucide::Layers,
            ),
            (
                Tab::Heroes,
                "Heroes".to_owned(),
                None,
                crate::icons::Lucide::Swords,
            ),
            (
                Tab::Discover,
                "Discover".to_owned(),
                None,
                crate::icons::Lucide::Compass,
            ),
            (
                Tab::Customize,
                "Customize".to_owned(),
                None,
                crate::icons::Lucide::Crosshair,
            ),
            (
                Tab::Downloads,
                "Downloads".to_owned(),
                (active_dl > 0).then_some(active_dl),
                crate::icons::Lucide::Download,
            ),
            (
                Tab::Settings,
                "Settings".to_owned(),
                None,
                crate::icons::Lucide::Settings,
            ),
        ];
        div()
            .flex()
            .flex_none()
            .items_end()
            .gap(px(4.))
            .h(px(40.))
            .px(px(12.))
            .bg(c(PANEL))
            .border_b_1()
            .border_color(c(BORDER))
            .children(tabs.into_iter().map(|(tab, name, count, icon)| {
                let active = self.tab == tab;
                div()
                    .id(SharedString::from(format!("tab-{name}")))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(36.))
                    .px(px(12.))
                    .cursor_pointer()
                    .border_b_2()
                    .border_color(if active { c(ACCENT) } else { ca(ACCENT, 0.0) })
                    .text_sm()
                    .text_color(if active { c(TEXT) } else { c(MUTED) })
                    .hover(|s| s.text_color(c(TEXT)))
                    .active(|s| s.opacity(0.8))
                    .child(
                        gpui_kit::component::Icon::new(icon)
                            .size(px(15.))
                            .text_color(if active { c(ACCENT) } else { c(FAINT) }),
                    )
                    .child(ctl_text(name))
                    .when_some(count, |d, n| {
                        d.child(
                            div()
                                .px(px(6.))
                                .rounded(px(999.))
                                .bg(c(RAISED))
                                .text_xs()
                                .text_color(c(MUTED))
                                .child(n.to_string()),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
            }))
    }

    fn render_banners(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut banners: Vec<(&str, AnyElement)> = Vec::new();
        if let Some(b) = self.launch.render_banner(cx) {
            banners.push(("launch", b));
        }
        if let Some(health) = &self.health {
            let errors: Vec<_> = health
                .issues
                .iter()
                .filter(|i| i.level == Level::Error)
                .cloned()
                .collect();
            if let Some(first) = errors.first() {
                let fix = first.fix.clone();
                let more = errors.len().saturating_sub(1);
                banners.push((
                    "health",
                    banner(CORAL)
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .text_color(c(TEXT))
                                .child(if more > 0 {
                                    format!("{} (+{more} more)", first.message)
                                } else {
                                    first.message.clone()
                                }),
                        )
                        .when_some(fix, |d, fix| {
                            let pending = self.is_pending("fix");
                            d.child(
                                Button::new("banner-fix")
                                    .small()
                                    .danger()
                                    .text_label(crate::mods::fix_label(&fix))
                                    .loading(pending)
                                    .disabled(pending)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.apply_fix(fix.clone(), cx)
                                    })),
                            )
                        })
                        .child(
                            Button::new("banner-health")
                                .small()
                                .ghost()
                                .text_label("Details")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.modal = Some(Modal::Health);
                                    cx.notify();
                                })),
                        )
                        .into_any_element(),
                ));
            }
        }
        if !self.other_managers.is_empty() {
            banners.push(("managers", 
                banner(AMBER)
                    .child(div().flex_1().text_sm().text_color(c(TEXT)).child(format!(
                        "Another mod manager's files were found ({}). Mixing managers can cause conflicts.",
                        self.other_managers.join(", ")
                    )))
                    .child(
                        Button::new("banner-import")
                            .small()
                            .warning()
                            .text_label("Review & import")
                            .on_click(cx.listener(|this, _, _, cx| this.open_adopt(cx))),
                    )
                    .into_any_element(),
            ));
        } else if self.foreign_count > 0 {
            banners.push(("foreign", 
                banner(INFO)
                    .child(div().flex_1().text_sm().text_color(c(TEXT)).child(format!(
                        "{} addon file(s) are not managed by Kombini. Import them to manage their order.",
                        self.foreign_count
                    )))
                    .child(
                        Button::new("banner-adopt")
                            .small()
                            .text_label("Import")
                            .on_click(cx.listener(|this, _, _, cx| this.open_adopt(cx))),
                    )
                    .into_any_element(),
            ));
        }
        div()
            .flex()
            .flex_col()
            .children(banners.into_iter().map(|(key, el)| {
                crate::anim::rise_in(
                    div().child(el),
                    SharedString::from(format!("banner-{key}")),
                    -8.,
                    crate::anim::MEDIUM,
                )
            }))
    }

    pub fn apply_fix(&mut self, fix: kombini_core::FixAction, cx: &mut Context<Self>) {
        use kombini_core::FixAction as F;
        match fix {
            F::OpenConflicts => {
                self.open_conflicts(cx);
            }
            F::OpenImport => self.open_adopt(cx),
            F::CheckUpdates => self.check_updates(true, cx),
            fix => self.run(
                "fix",
                "Applying fix",
                cx,
                move |e, _| e.apply_fix(&fix),
                |this, msg, _| this.success(msg),
            ),
        }
    }

    fn render_statusbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let visible: Vec<&Op> = self.ops.iter().filter(|o| o.visible()).collect();
        let game = self
            .engine
            .as_ref()
            .map(|e| e.paths().root.display().to_string())
            .unwrap_or_default();
        let content: AnyElement = if let Some(op) = visible.first() {
            let stage = op.progress.stage_label();
            let text = if stage.is_empty() {
                op.label.clone()
            } else {
                format!("{} — {stage}", op.label)
            };
            let more = visible.len() - 1;
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .flex_1()
                .child(gpui_kit::component::spinner::Spinner::new().xsmall())
                .child(
                    div()
                        .text_xs()
                        .text_color(c(TEXT))
                        .truncate()
                        .child(if more > 0 {
                            format!("{text} (+{more})")
                        } else {
                            text
                        }),
                )
                .child(
                    div()
                        .w(px(160.))
                        .child(progress_bar("op-progress", op.progress.fraction())),
                )
                .when(op.cancellable, |d| {
                    let p = op.progress.clone();
                    d.child(
                        Button::new("op-cancel")
                            .small()
                            .ghost()
                            .text_label("Cancel")
                            .on_click(move |_, _, _| p.cancel()),
                    )
                })
                .into_any_element()
        } else {
            div()
                .flex_1()
                .text_xs()
                .text_color(c(FAINT))
                .truncate()
                .child(game)
                .into_any_element()
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(12.))
            .h(px(30.))
            .px(px(12.))
            .bg(c(PANEL))
            .border_t_1()
            .border_color(c(BORDER))
            .child(content)
            .child({
                let wait = self
                    .last_manual_update_check
                    .map(|t| MANUAL_UPDATE_COOLDOWN.saturating_sub(t.elapsed()))
                    .filter(|d| !d.is_zero());
                Button::new("check-updates")
                    .small()
                    .ghost()
                    .icon(gpui_kit::component::IconName::RefreshCw)
                    .text_label("Check for updates")
                    .tooltip(match wait {
                        Some(d) => format!("Just checked. Try again in {}s.", d.as_secs() + 1),
                        None => "Ask GameBanana for newer files of your installed mods".to_owned(),
                    })
                    .loading(self.is_pending("updates"))
                    .disabled(wait.is_some() || self.is_pending("updates") || self.engine.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.manual_update_check(cx)))
            })
            .child(
                Button::new("activity")
                    .small()
                    .ghost()
                    .icon(crate::icons::Lucide::Clock)
                    .text_label(format!("Activity ({})", self.history.len()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.modal = Some(Modal::Activity);
                        cx.notify();
                    })),
            )
    }

    fn render_boot(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match &self.boot {
            Boot::Loading(msg) => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(12.))
                .child(gpui_kit::component::spinner::Spinner::new().large())
                .child(muted(msg.clone()))
                .into_any_element(),
            Boot::NeedGame { detected, error } => {
                let detected = detected.clone();
                let error = error.clone();
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        panel()
                            .w(px(520.))
                            .p(px(24.))
                            .flex()
                            .flex_col()
                            .gap(px(16.))
                            .child(heading("Where is Deadlock installed?"))
                            .child(muted(
                                "Pick the Deadlock folder (the one containing game\\citadel). \
                                 Kombini only changes files inside game\\citadel\\addons and gameinfo.gi.",
                            ))
                            .when_some(error, |d, e| {
                                d.child(
                                    div()
                                        .p(px(10.))
                                        .rounded(px(RADIUS))
                                        .bg(ca(CORAL, 0.12))
                                        .text_sm()
                                        .text_color(c(CORAL))
                                        .child(e),
                                )
                            })
                            .children(detected.into_iter().enumerate().map(|(i, p)| {
                                let shown = p.display().to_string();
                                card()
                                    .id(("detected", i))
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .p(px(12.))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(c(HOVER)))
                                    .child(dot(ACCENT))
                                    .child(div().flex_1().text_sm().text_color(c(TEXT)).child(shown))
                                    .child(muted("Use"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_game(p.clone(), cx)
                                    }))
                            }))
                            .child(
                                Button::new("browse-game")
                                    .primary()
                                    .text_label("Browse for folder…")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.pick_game_folder(cx)
                                    })),
                            ),
                    )
                    .into_any_element()
            }
            Boot::Ready => div().into_any_element(),
        }
    }
}

pub fn banner(color: u32) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(16.))
        .py(px(8.))
        .bg(ca(color, 0.10))
        .border_b_1()
        .border_color(ca(color, 0.35))
}

/// What the user can do about an error, by code.
pub fn hint(code: ErrorCode) -> Option<&'static str> {
    use ErrorCode as E;
    Some(match code {
        E::GameRunning => "Close Deadlock (and wait for Steam updates to finish), then try again.",
        E::PermissionDenied | E::GameinfoReadOnly => {
            "Another program may be locking the file. Close it, or use Health › Clear read-only."
        }
        E::GameNotFound | E::GameNotConfigured => "Choose the Deadlock folder in Settings.",
        E::GameinfoMarkersCorrupt => "Use Health › Repair gameinfo markers.",
        E::GameinfoMissingSearchPaths => "Verify game files in Steam, then press Reapply.",
        E::NoVpkInArchive => "This download has no .vpk files; it may not be a Deadlock mod.",
        E::ArchiveEncrypted => "Password-protected archives are not supported.",
        E::Network | E::RateLimited => "Check your connection and retry in a minute.",
        E::HashMismatch => "The download was corrupted. Retry it from Downloads.",
        E::LayoutTooManyRoots => "Disable some mods; Deadlock has a limited number of pak slots.",
        E::Io => "Check free disk space and that the folder is accessible.",
        _ => return None,
    })
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.first_frame_logged {
            self.first_frame_logged = true;
            if let Some(t) = crate::STARTED.get() {
                tracing::info!(ms = t.elapsed().as_millis() as u64, "startup: first frame");
            }
        }
        if self.dragging.is_some() && !cx.has_active_drag() {
            self.dragging = None;
        }
        for note in self.toasts.drain(..) {
            window.push_notification(note, cx);
        }
        let body: AnyElement = match self.boot {
            Boot::Ready => {
                let content: AnyElement = match self.tab {
                    Tab::Mods => self.render_mods(window, cx).into_any_element(),
                    Tab::Heroes => self.render_heroes(window, cx),
                    Tab::Discover => self.render_discover(window, cx).into_any_element(),
                    Tab::Customize => self.render_customize(window, cx),
                    Tab::Downloads => self.render_downloads(cx).into_any_element(),
                    Tab::Settings => self.render_settings(window, cx).into_any_element(),
                };
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(self.render_topbar(cx))
                    .child(self.render_banners(cx))
                    .child(self.render_tabs(cx))
                    .child(div().flex_1().min_h_0().child(crate::anim::rise_in(
                        div().size_full().child(content),
                        SharedString::from(format!("tab-{:?}", self.tab)),
                        6.,
                        crate::anim::MEDIUM,
                    )))
                    .child(self.render_statusbar(cx))
                    .into_any_element()
            }
            _ => self.render_boot(cx),
        };
        let modal = self.render_modal(window, cx);
        div()
            .id("root")
            .size_full()
            .relative()
            .bg(c(WINDOW))
            .text_color(c(TEXT))
            .font_family("Segoe UI")
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.install_local(paths.paths().to_vec(), cx);
            }))
            .child(body)
            .children(modal)
    }
}
