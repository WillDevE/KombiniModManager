//! Launch state machine driver: Preparing → Launching →
//! WaitingForProcess → Running → Exited, with Failed/TimedOut exits. The UI
//! never claims the game is running before `deadlock.exe` is actually seen.
//! Also hosts the "Find broken mod" bisect banner.

use crate::app::{AppView, banner};
use crate::theme::*;
use crate::widgets::*;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::*;
use gpui_kit::component::notification::Notification;
use gpui_kit::*;
use kombini_engine::bisect::{Bisect, BisectResult};
use kombini_engine::launch::{LaunchEvent, LaunchMode, LaunchPhase, SessionOutcome};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub struct LaunchUi {
    pub phase: LaunchPhase,
    pub mode: Option<LaunchMode>,
    pub phase_at: Instant,
    pub error: Option<String>,
    pub outcome: Option<(SessionOutcome, i64)>,
    pub stop_waiting: Arc<AtomicBool>,
    pub bisect: Option<Bisect>,
    pub dismissed: bool,
}

impl Default for LaunchUi {
    fn default() -> Self {
        Self {
            phase: LaunchPhase::Idle,
            mode: None,
            phase_at: Instant::now(),
            error: None,
            outcome: None,
            stop_waiting: Arc::new(AtomicBool::new(false)),
            bisect: None,
            dismissed: false,
        }
    }
}

impl LaunchUi {
    fn apply(&mut self, event: LaunchEvent) {
        let next = self.phase.next(event);
        if next != self.phase {
            self.phase = next;
            self.phase_at = Instant::now();
        }
    }

    pub fn phase_text(&self) -> String {
        let secs = self.phase_at.elapsed().as_secs();
        match self.phase {
            LaunchPhase::Preparing => "Preparing…".into(),
            LaunchPhase::Launching => "Starting Steam…".into(),
            LaunchPhase::WaitingForProcess => format!("Waiting for game… {secs}s"),
            LaunchPhase::Running => format!("Running {}:{:02}", secs / 60, secs % 60),
            _ => "Ready".into(),
        }
    }

    pub fn button_label(&self) -> String {
        if self.phase.busy() && self.mode == Some(LaunchMode::Modded) {
            self.phase_text()
        } else {
            "Launch Modded".into()
        }
    }

    pub fn render_banner(&self, cx: &mut Context<AppView>) -> Option<AnyElement> {
        if let Some(b) = &self.bisect {
            return Some(render_bisect(b, cx));
        }
        if self.dismissed {
            return None;
        }
        let dismiss = || {
            Button::new("launch-dismiss")
                .small()
                .ghost()
                .label("Dismiss")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.launch.dismissed = true;
                    this.launch.apply(LaunchEvent::Reset);
                    cx.notify();
                }))
        };
        let text = |s: String| div().flex_1().text_sm().text_color(c(TEXT)).child(s);
        let el = match self.phase {
            LaunchPhase::WaitingForProcess => banner(INFO)
                .child(gpui_kit::component::spinner::Spinner::new().xsmall())
                .child(text(format!(
                    "Steam is starting Deadlock… ({}s). Accept any Steam prompt if one appears.",
                    self.phase_at.elapsed().as_secs()
                )))
                .child(
                    Button::new("launch-stop-wait")
                        .small()
                        .ghost()
                        .label("Stop waiting")
                        .on_click(cx.listener(|this, _, _, _| {
                            this.launch.stop_waiting.store(true, Ordering::Relaxed);
                        })),
                ),
            LaunchPhase::Running => banner(ACCENT)
                .child(dot(ACCENT))
                .child(text(format!(
                    "Deadlock is running ({}). Mod changes are paused until it closes.",
                    match self.mode {
                        Some(LaunchMode::Vanilla) => "without mods",
                        _ => "with mods",
                    }
                ))),
            LaunchPhase::TimedOut => banner(AMBER)
                .child(text(
                    "Deadlock did not start in time. Check Steam for an update, a prompt, or a game already running."
                        .into(),
                ))
                .child(dismiss()),
            LaunchPhase::Failed => banner(CORAL)
                .child(text(format!(
                    "Launch failed: {}",
                    self.error.clone().unwrap_or_default()
                )))
                .child(dismiss()),
            LaunchPhase::Exited => match (self.outcome, self.mode) {
                (Some((SessionOutcome::Crash | SessionOutcome::ShortSession, secs)), Some(LaunchMode::Modded)) => {
                    let crashed = matches!(self.outcome, Some((SessionOutcome::Crash, _)));
                    banner(CORAL)
                        .child(text(if crashed {
                            "Deadlock crashed. A mod may be the cause.".to_owned()
                        } else {
                            format!("Deadlock closed after {secs}s. If it crashed, a mod may be the cause.")
                        }))
                        .child(
                            Button::new("bisect-start")
                                .small()
                                .danger()
                                .label("Find broken mod")
                                .tooltip("Disables half of your mods at a time to find the culprit")
                                .on_click(cx.listener(|this, _, _, cx| this.bisect_start(cx))),
                        )
                        .child(
                            Button::new("launch-vanilla-test")
                                .small()
                                .ghost()
                                .label("Try vanilla")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.start_launch(LaunchMode::Vanilla, cx)
                                })),
                        )
                        .child(dismiss())
                }
                _ => return None,
            },
            _ => return None,
        };
        Some(el.into_any_element())
    }
}

fn render_bisect(b: &Bisect, cx: &mut Context<AppView>) -> AnyElement {
    let pending = |this: &AppView| this.is_pending("bisect");
    let _ = pending;
    match &b.result {
        Some(BisectResult::Found { mod_id }) => {
            let id = mod_id.clone();
            banner(ACCENT)
                .child(div().flex_1().text_sm().text_color(c(TEXT)).child(format!(
                    "Found it: {} looks like the broken mod.",
                    id
                )))
                .child(
                    Button::new("bisect-disable")
                        .small()
                        .primary()
                        .label("Restore others & disable it")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.bisect_finish(Some(id.clone()), cx)
                        })),
                )
                .child(
                    Button::new("bisect-keep")
                        .small()
                        .ghost()
                        .label("Restore all")
                        .on_click(cx.listener(|this, _, _, cx| this.bisect_finish(None, cx))),
                )
                .into_any_element()
        }
        Some(BisectResult::Inconclusive) => banner(AMBER)
            .child(div().flex_1().text_sm().text_color(c(TEXT)).child(
                "No single mod causes this. It may need two mods together, or not be mod related.",
            ))
            .child(
                Button::new("bisect-restore")
                    .small()
                    .label("Restore my mods")
                    .on_click(cx.listener(|this, _, _, cx| this.bisect_finish(None, cx))),
            )
            .into_any_element(),
        None => banner(INFO)
            .child(div().flex_1().text_sm().text_color(c(TEXT)).child(format!(
                "Finding the broken mod — round {}: {} of {} suspects enabled (about {} round(s) left). Launch, then tell us what happened.",
                b.round,
                b.testing.len(),
                b.suspects.len(),
                b.rounds_left()
            )))
            .child(
                Button::new("bisect-problem")
                    .small()
                    .danger()
                    .label("Problem still happens")
                    .on_click(cx.listener(|this, _, _, cx| this.bisect_report(true, cx))),
            )
            .child(
                Button::new("bisect-ok")
                    .small()
                    .success()
                    .label("Works fine")
                    .on_click(cx.listener(|this, _, _, cx| this.bisect_report(false, cx))),
            )
            .child(
                Button::new("bisect-cancel")
                    .small()
                    .ghost()
                    .label("Stop")
                    .on_click(cx.listener(|this, _, _, cx| this.bisect_finish(None, cx))),
            )
            .into_any_element(),
    }
}

async fn bg<T: Send + 'static>(cx: &mut AsyncApp, f: impl FnOnce() -> T + Send + 'static) -> T {
    cx.background_executor().spawn(async move { f() }).await
}

impl AppView {
    pub fn start_launch(&mut self, mode: LaunchMode, cx: &mut Context<Self>) {
        if self.launch.phase.busy() {
            return;
        }
        let Some(engine) = self.engine.clone() else {
            return;
        };
        // Skin shuffle (Settings), never during a bisect.
        let shuffle = (matches!(mode, LaunchMode::Modded)
            && self.settings.shuffle_on_launch
            && self.launch.bisect.is_none())
        .then(|| crate::heroes::shuffle_plan(&self.hero_entries(), &mut crate::heroes::Dice::new()))
        .filter(|(on, off)| !on.is_empty() || !off.is_empty());
        let bisect = self.launch.bisect.take();
        self.launch = LaunchUi {
            bisect,
            ..LaunchUi::default()
        };
        self.launch.mode = Some(mode);
        self.launch.apply(LaunchEvent::Start);
        let stop = self.launch.stop_waiting.clone();
        let timeout = Duration::from_secs(u64::from(self.settings.launch_timeout_seconds));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let e = engine.clone();
            let launched = bg(cx, move || {
                if let Some((on, off)) = &shuffle {
                    e.set_enabled_many(on, off, "Shuffled skins for this launch")?;
                }
                e.launch(mode)
            })
            .await;
            let ticket = match launched {
                Ok(t) => t,
                Err(err) => {
                    this.update(cx, |this, cx| {
                        this.launch.error = Some(err.message.clone());
                        this.launch.apply(LaunchEvent::Error);
                        this.error("Launch", &err);
                        this.refresh(cx);
                    })
                    .ok();
                    return;
                }
            };
            this.update(cx, |this, cx| {
                this.launch.apply(LaunchEvent::Prepared);
                this.launch.apply(LaunchEvent::SteamStarted);
                cx.notify();
            })
            .ok();
            // Wait for deadlock.exe.
            let waited = Instant::now();
            let mut seen = false;
            while waited.elapsed() < timeout && !stop.load(Ordering::Relaxed) {
                cx.background_executor()
                    .timer(Duration::from_millis(750))
                    .await;
                let e = engine.clone();
                if bg(cx, move || e.game_running()).await {
                    seen = true;
                    break;
                }
            }
            if !seen {
                let e = engine.clone();
                let t = ticket.clone();
                let _ = bg(cx, move || e.finish_launch(&t, false, 0)).await;
                this.update(cx, |this, cx| {
                    this.launch.apply(LaunchEvent::Timeout);
                    this.refresh(cx);
                })
                .ok();
                return;
            }
            this.update(cx, |this, cx| {
                this.launch.apply(LaunchEvent::ProcessSeen);
                cx.notify();
            })
            .ok();
            let running = Instant::now();
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                let e = engine.clone();
                if !bg(cx, move || e.game_running()).await {
                    break;
                }
            }
            let secs = running.elapsed().as_secs() as i64;
            let e = engine.clone();
            let record = bg(cx, move || e.finish_launch(&ticket, true, secs)).await;
            this.update(cx, |this, cx| {
                this.launch.apply(LaunchEvent::ProcessGone);
                match record {
                    Ok(r) => {
                        this.launch.outcome = r.outcome.map(|o| (o, secs));
                        if matches!(r.outcome, Some(SessionOutcome::Clean)) {
                            this.toast(Notification::info(format!(
                                "Deadlock closed after {}m {}s.",
                                secs / 60,
                                secs % 60
                            )));
                        }
                    }
                    Err(e) => this.error("Recording the session", &e),
                }
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    pub fn load_bisect(&mut self, cx: &mut Context<Self>) {
        self.background(
            cx,
            |e| Ok(e.bisect_state()),
            |this, state, cx| {
                this.launch.bisect = state;
                cx.notify();
            },
        );
    }

    pub fn bisect_start(&mut self, cx: &mut Context<Self>) {
        // Mods with known issues or uploaded before a major update go first.
        let likely = self.outdated_ids();
        self.run(
            "bisect",
            "Starting the broken-mod finder",
            cx,
            move |e, _| e.bisect_start_with(&likely),
            |this, b, cx| {
                let total = b.original_enabled.len();
                let testing = b.testing.len();
                this.launch.bisect = Some(b);
                this.launch.dismissed = true;
                this.toast(Notification::info(if testing * 2 < total {
                    format!(
                        "Testing the {testing} mod(s) most likely to be outdated first; the rest are disabled. Launch the game and check whether the problem happens."
                    )
                } else {
                    "Half of your mods are disabled. Launch the game and check whether the problem happens.".to_owned()
                }));
                cx.notify();
            },
        );
    }

    pub fn bisect_report(&mut self, problem: bool, cx: &mut Context<Self>) {
        self.run(
            "bisect",
            "Next round",
            cx,
            move |e, _| e.bisect_report(problem),
            |this, b, cx| {
                this.launch.bisect = Some(b);
                cx.notify();
            },
        );
    }

    pub fn bisect_finish(&mut self, disable: Option<kombini_core::ModId>, cx: &mut Context<Self>) {
        self.run(
            "bisect",
            "Restoring your mods",
            cx,
            move |e, _| {
                let report = e.bisect_finish()?;
                if let Some(id) = disable {
                    e.set_enabled(std::slice::from_ref(&id), false)?;
                }
                Ok(report)
            },
            |this, _, cx| {
                this.launch.bisect = None;
                this.success("Your mods were restored.");
                cx.notify();
            },
        );
    }
}
