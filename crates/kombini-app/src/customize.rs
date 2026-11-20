//! "Customize" tab: a crosshair designer and a library of autoexec commands.
//! Both write managed blocks into the active profile's autoexec, so lines the
//! user wrote themselves are never touched.

use crate::app::AppView;
use crate::crosshair::{COMMAND_LIBRARY, Crosshair, get_block, has_command, set_block};
use crate::icons::Lucide;
use crate::smooth_scroll::Smooth as _;
use crate::theme::*;
use crate::widgets::*;
use gpui_kit::component::button::*;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable, IconName, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kombini_core::ProfileId;

const CROSSHAIR_BLOCK: &str = "crosshair";
const COMMANDS_BLOCK: &str = "commands";
/// Preview box size and zoom (1080p layout units to px).
const PREVIEW: f32 = 220.;
const PREVIEW_SCALE: f32 = 3.;
const PREVIEW_BG: u32 = 0x4a5059;

const SWATCHES: [[u8; 3]; 8] = [
    [255, 255, 255],
    [0, 0, 0],
    [0, 255, 0],
    [0, 255, 255],
    [255, 255, 0],
    [255, 0, 255],
    [255, 0, 0],
    [255, 128, 0],
];

#[derive(Default)]
pub struct CustomizeState {
    /// Profile whose autoexec is loaded.
    pub loaded_for: Option<ProfileId>,
    pub loading: bool,
    /// The autoexec text as last read or saved.
    pub text: String,
    pub crosshair: Option<Crosshair>,
    /// The crosshair currently in the autoexec (for "unsaved changes").
    pub saved: Option<Crosshair>,
    /// One slider per control row (see `channel`).
    pub sliders: Vec<Entity<SliderState>>,
    pub slider_subs: Vec<Subscription>,
}

type Field = fn(&mut Crosshair) -> &mut f32;

/// Number rows: label, field, step, min, max, decimals.
const NUMBERS: [(&str, Field, f32, f32, f32, bool); 12] = [
    ("Gap", |x| &mut x.pip_gap, 1., -10., 50., false),
    ("Length", |x| &mut x.pip_height, 1., 0., 50., false),
    ("Thickness", |x| &mut x.pip_width, 1., 0., 20., false),
    ("Opacity", |x| &mut x.pip_opacity, 0.1, 0., 1., true),
    (
        "Outline width",
        |x| &mut x.pip_outline_border,
        1.,
        0.,
        10.,
        false,
    ),
    (
        "Outline gap",
        |x| &mut x.pip_outline_gap,
        1.,
        0.,
        10.,
        false,
    ),
    (
        "Outline opacity",
        |x| &mut x.pip_outline_opacity,
        0.1,
        0.,
        1.,
        true,
    ),
    ("Dot size", |x| &mut x.dot_size, 1., 0., 30., false),
    ("Dot opacity", |x| &mut x.dot_opacity, 0.1, 0., 1., true),
    (
        "Dot outline width",
        |x| &mut x.dot_outline_border,
        1.,
        0.,
        10.,
        false,
    ),
    (
        "Dot outline gap",
        |x| &mut x.dot_outline_gap,
        1.,
        0.,
        10.,
        false,
    ),
    (
        "Dot outline opacity",
        |x| &mut x.dot_outline_opacity,
        0.1,
        0.,
        1.,
        true,
    ),
];

/// Width of the label column in the crosshair controls.
const LABEL_W: f32 = 130.;
/// Slider rows: the [`NUMBERS`], then colour R, G, B, then outline R, G, B.
const CHANNELS: usize = 18;

#[derive(Clone, Copy)]
struct ChannelSpec {
    label: &'static str,
    step: f32,
    min: f32,
    max: f32,
    decimals: bool,
    /// Colour channels sit under their swatches.
    indent: bool,
}

fn channel(i: usize) -> ChannelSpec {
    if let Some(&(label, _, step, min, max, decimals)) = NUMBERS.get(i) {
        return ChannelSpec {
            label,
            step,
            min,
            max,
            decimals,
            indent: false,
        };
    }
    ChannelSpec {
        label: ["Red", "Green", "Blue"][(i - NUMBERS.len()) % 3],
        step: 1.,
        min: 0.,
        max: 255.,
        decimals: false,
        indent: true,
    }
}

fn colour_channel(i: usize) -> (bool, usize) {
    let c = i - NUMBERS.len();
    (c >= 3, c % 3)
}

fn channel_value(i: usize, x: &Crosshair) -> f32 {
    let mut copy = *x;
    if let Some(&(_, field, ..)) = NUMBERS.get(i) {
        return *field(&mut copy);
    }
    let (outline, ch) = colour_channel(i);
    f32::from(if outline {
        x.outline_color[ch]
    } else {
        x.color[ch]
    })
}

fn set_channel(i: usize, x: &mut Crosshair, value: f32) {
    if let Some(&(_, field, _, min, max, decimals)) = NUMBERS.get(i) {
        let v = if decimals {
            (value * 100.).round() / 100.
        } else {
            value.round()
        };
        *field(x) = v.clamp(min, max);
        return;
    }
    let (outline, ch) = colour_channel(i);
    let v = value.round().clamp(0., 255.) as u8;
    if outline {
        x.outline_color[ch] = v;
    } else {
        x.color[ch] = v;
    }
}

fn rgba(rgb: [u8; 3], a: f32) -> Rgba {
    Rgba {
        r: rgb[0] as f32 / 255.,
        g: rgb[1] as f32 / 255.,
        b: rgb[2] as f32 / 255.,
        a: a.clamp(0., 1.),
    }
}

fn hex(rgb: [u8; 3]) -> u32 {
    (rgb[0] as u32) << 16 | (rgb[1] as u32) << 8 | rgb[2] as u32
}

/// Absolutely placed rectangle relative to the preview centre.
fn place(x: f32, y: f32, w: f32, h: f32) -> Div {
    let mid = PREVIEW / 2.;
    div()
        .absolute()
        .left(px(mid + x))
        .top(px(mid + y))
        .w(px(w))
        .h(px(h))
}

fn preview(x: &Crosshair) -> Div {
    let l = x.layout(PREVIEW_SCALE);
    let bg = c(PREVIEW_BG);
    let mut root = div()
        .relative()
        .flex_none()
        .size(px(PREVIEW))
        .rounded(px(RADIUS))
        .overflow_hidden()
        .bg(bg);
    // Dot outline: a ring drawn as a filled circle with the middle cut out.
    let b = l.dot_outline_border;
    if b > 0. && x.dot_outline_opacity > 0. && l.dot_outline_size > 0. {
        let s = l.dot_outline_size;
        let inner = (s - 2. * b).max(0.);
        root = root
            .child(
                place(-s / 2., -s / 2., s, s)
                    .rounded_full()
                    .bg(rgba(x.outline_color, x.dot_outline_opacity)),
            )
            .child(
                place(-inner / 2., -inner / 2., inner, inner)
                    .rounded_full()
                    .bg(bg),
            );
    }
    // Pip outlines: borders painted inward along the grown box.
    let b = l.pip_outline_border;
    if b > 0. && x.pip_outline_opacity > 0. {
        let col = rgba(x.outline_color, x.pip_outline_opacity);
        for r in &l.pip_outlines {
            let side = (r.h - 2. * b).max(0.);
            root = root
                .child(place(r.x, r.y, r.w, b).bg(col))
                .child(place(r.x, r.y + r.h - b, r.w, b).bg(col))
                .child(place(r.x, r.y + b, b, side).bg(col))
                .child(place(r.x + r.w - b, r.y + b, b, side).bg(col));
        }
    }
    if x.pip_opacity > 0. {
        let col = rgba(x.color, x.pip_opacity);
        for r in &l.pips {
            root = root.child(place(r.x, r.y, r.w, r.h).bg(col));
        }
    }
    if x.dot_opacity > 0. && l.dot_size > 0. {
        let s = l.dot_size;
        root = root.child(
            place(-s / 2., -s / 2., s, s)
                .rounded_full()
                .bg(rgba(x.color, x.dot_opacity)),
        );
    }
    root
}

impl AppView {
    /// Reads the active profile's autoexec (its stored copy, else the file).
    fn customize_load(&mut self, cx: &mut Context<Self>) {
        let profile = self.lib.profile().clone();
        self.customize.loaded_for = Some(profile.id.clone());
        if let Some(text) = profile.autoexec.clone() {
            self.customize_set_text(text);
            return;
        }
        self.customize.loading = true;
        self.background(
            cx,
            |e| Ok(e.read_autoexec().unwrap_or_default()),
            |this, text, cx| {
                this.customize.loading = false;
                this.customize_set_text(text);
                cx.notify();
            },
        );
    }

    fn customize_set_text(&mut self, text: String) {
        let saved = get_block(&text, CROSSHAIR_BLOCK)
            .map(|b| Crosshair::from_commands(Crosshair::GAME, &b));
        self.customize.crosshair = Some(saved.unwrap_or(Crosshair::GAME));
        self.customize.saved = saved;
        self.customize.text = text;
    }

    /// Writes `text` as the profile's autoexec.
    fn customize_write(&mut self, text: String, message: String, cx: &mut Context<Self>) {
        let write = text.clone();
        self.run(
            "autoexec",
            "Saving autoexec",
            cx,
            move |e, _| e.set_profile_autoexec(Some(write)),
            move |this, _, cx| {
                let crosshair = this.customize.crosshair;
                this.customize_set_text(text);
                // Keep unsaved crosshair edits when only the command list changed.
                if crosshair.is_some() {
                    this.customize.crosshair = crosshair;
                }
                this.success(message);
                cx.notify();
            },
        );
    }

    fn crosshair_save(&mut self, cx: &mut Context<Self>) {
        let Some(x) = self.customize.crosshair else {
            return;
        };
        let text = set_block(&self.customize.text, CROSSHAIR_BLOCK, &x.commands());
        self.customize.saved = Some(x);
        self.customize_write(
            text,
            "Crosshair saved to this profile's autoexec. It applies the next time the game starts."
                .into(),
            cx,
        );
    }

    fn crosshair_remove(&mut self, cx: &mut Context<Self>) {
        let text = set_block(&self.customize.text, CROSSHAIR_BLOCK, "");
        self.customize.crosshair = Some(Crosshair::GAME);
        self.customize_write(
            text,
            "Crosshair removed from the autoexec; the game's own settings apply again.".into(),
            cx,
        );
    }

    fn crosshair_from_game(&mut self, cx: &mut Context<Self>) {
        self.background(
            cx,
            |e| {
                // The game saves client settings per Steam account; older builds
                // kept them next to the game.
                let paths = kombini_steam::steam_dir_for(&e.paths().root)
                    .map(|d| kombini_steam::machine_convars_files(&d))
                    .unwrap_or_default();
                let fallback = e.paths().citadel.join("cfg").join("machine_convars.vcfg");
                Ok(paths
                    .into_iter()
                    .chain(std::iter::once(fallback))
                    .find_map(|p| std::fs::read(p).ok())
                    .map(|b| String::from_utf8_lossy(&b).into_owned()))
            },
            |this, text, cx| {
                match text {
                    Some(t) => {
                        this.customize.crosshair =
                            Some(Crosshair::from_machine_convars(Crosshair::GAME, &t));
                        this.success("Loaded the crosshair from the game's settings. Save to keep it.");
                    }
                    None => this.toast(gpui_kit::component::notification::Notification::info(
                        "The game's settings file was not found. Start Deadlock once, then try again.",
                    )),
                }
                cx.notify();
            },
        );
    }

    fn command_toggle(
        &mut self,
        command: &'static str,
        name: &'static str,
        cx: &mut Context<Self>,
    ) {
        let text = self.customize.text.clone();
        let body = get_block(&text, COMMANDS_BLOCK).unwrap_or_default();
        let (body, message) = if has_command(&body, command) {
            let kept: Vec<&str> = body.lines().filter(|l| !has_command(l, command)).collect();
            (
                kept.join("\n"),
                format!("Removed \"{name}\" from the autoexec."),
            )
        } else {
            let mut lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
            // One value per convar: replace e.g. fps_max 144 with fps_max 0.
            let convar = command.split_whitespace().next().unwrap_or(command);
            lines.retain(|l| l.split_whitespace().next() != Some(convar));
            lines.push(command);
            (
                lines.join("\n"),
                format!("Added \"{name}\" to the autoexec."),
            )
        };
        self.customize_write(set_block(&text, COMMANDS_BLOCK, &body), message, cx);
    }

    /// Creates the slider states once (one per [`channel`]) and keeps them
    /// in step with the crosshair when it changes elsewhere (stepper,
    /// preset, load from game).
    fn sync_sliders(&mut self, x: &Crosshair, window: &mut Window, cx: &mut Context<Self>) {
        if self.customize.sliders.is_empty() {
            for i in 0..CHANNELS {
                let spec = channel(i);
                let state = cx.new(|_| {
                    SliderState::new()
                        .min(spec.min)
                        .max(spec.max)
                        .step(spec.step)
                        .default_value(spec.min)
                });
                let sub = cx.subscribe(&state, move |this, _, event: &SliderEvent, cx| {
                    let SliderEvent::Change(value) = event else {
                        return;
                    };
                    if let Some(x) = this.customize.crosshair.as_mut() {
                        set_channel(i, x, value.start());
                        cx.notify();
                    }
                });
                self.customize.sliders.push(state);
                self.customize.slider_subs.push(sub);
            }
        }
        for (i, state) in self.customize.sliders.clone().into_iter().enumerate() {
            let want = channel_value(i, x);
            if (state.read(cx).value().start() - want).abs() > 1e-4 {
                state.update(cx, |s, cx| s.set_value(want, window, cx));
            }
        }
    }

    /// Label, slider, and the −/value/+ stepper for channel `i`.
    fn render_channel_row(
        &mut self,
        i: usize,
        x: &Crosshair,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let spec = channel(i);
        let value = channel_value(i, x);
        let shown = if spec.decimals {
            format!("{value:.1}")
        } else {
            format!("{}", value.trunc())
        };
        let bump = |dir: f32| {
            cx.listener(move |this: &mut AppView, _: &ClickEvent, _, cx| {
                if let Some(x) = this.customize.crosshair.as_mut() {
                    let v = channel_value(i, x) + dir * spec.step;
                    set_channel(i, x, ((v * 10.).round() / 10.).clamp(spec.min, spec.max));
                    cx.notify();
                }
            })
        };
        let dec = bump(-1.);
        let inc = bump(1.);
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .child(
                div()
                    .w(px(LABEL_W))
                    .flex_none()
                    .text_sm()
                    .text_color(c(MUTED))
                    .when(spec.indent, |d| d.pl(px(12.)))
                    .child(spec.label),
            )
            .child(
                div().flex_1().min_w_0().px(px(6.)).children(
                    self.customize
                        .sliders
                        .get(i)
                        .map(|s| Slider::new(s).disabled(busy)),
                ),
            )
            .child(
                Button::new(("xh-dec", i))
                    .small()
                    .ghost()
                    .icon(IconName::Minus)
                    .disabled(busy || value <= spec.min)
                    .on_click(dec),
            )
            .child(
                div()
                    .w(px(36.))
                    .text_center()
                    .text_sm()
                    .text_color(c(TEXT))
                    .child(shown),
            )
            .child(
                Button::new(("xh-inc", i))
                    .small()
                    .ghost()
                    .icon(IconName::Plus)
                    .disabled(busy || value >= spec.max)
                    .on_click(inc),
            )
    }

    fn render_crosshair_controls(
        &mut self,
        x: Crosshair,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let rows: Vec<Div> = (0..CHANNELS)
            .map(|i| self.render_channel_row(i, &x, busy, cx))
            .collect();
        let mut rows = rows.into_iter();
        let numbers: Vec<Div> = rows.by_ref().take(NUMBERS.len()).collect();
        let colour: Vec<Div> = rows.by_ref().take(3).collect();
        let outline: Vec<Div> = rows.collect();
        let swatches =
            |id: &'static str, current: [u8; 3], outline: bool, cx: &mut Context<Self>| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(30.))
                    .child(
                        div()
                            .w(px(LABEL_W))
                            .flex_none()
                            .text_sm()
                            .text_color(c(MUTED))
                            .child(if outline { "Outline colour" } else { "Colour" }),
                    )
                    .children(SWATCHES.iter().enumerate().map(|(i, &rgb)| {
                        let active = rgb == current;
                        div()
                            .id((id, i))
                            .size(px(18.))
                            .rounded_full()
                            .cursor_pointer()
                            .bg(c(hex(rgb)))
                            .border_2()
                            .border_color(if active { c(ACCENT) } else { c(BORDER) })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(x) = this.customize.crosshair.as_mut() {
                                    if outline {
                                        x.outline_color = rgb;
                                    } else {
                                        x.color = rgb;
                                    }
                                    cx.notify();
                                }
                            }))
                    }))
            };
        let switch_row = |id: &'static str,
                          label: &'static str,
                          value: bool,
                          set: fn(&mut Crosshair, bool),
                          cx: &mut Context<Self>| {
            div()
                .flex()
                .items_center()
                .h(px(30.))
                .child(div().flex_1().text_sm().text_color(c(MUTED)).child(label))
                .child(
                    Switch::new(id)
                        .checked(value)
                        .disabled(busy)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            if let Some(x) = this.customize.crosshair.as_mut() {
                                set(x, *checked);
                                cx.notify();
                            }
                        })),
                )
        };
        div()
            .flex()
            .flex_col()
            .children(numbers)
            .child(swatches("xh-color", x.color, false, cx))
            .children(colour)
            .child(swatches("xh-outline", x.outline_color, true, cx))
            .children(outline)
            .child(switch_row(
                "xh-static",
                "Fixed gap (no spread while moving)",
                x.pip_gap_static,
                |x, v| x.pip_gap_static = v,
                cx,
            ))
            .child(switch_row(
                "xh-hero",
                "Use this crosshair for every hero",
                x.disable_hero_specific,
                |x, v| x.disable_hero_specific = v,
                cx,
            ))
    }

    fn render_command_library(&mut self, busy: bool, cx: &mut Context<Self>) -> Div {
        let text = self.customize.text.clone();
        let managed = get_block(&text, COMMANDS_BLOCK).unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .children(
                COMMAND_LIBRARY
                    .iter()
                    .enumerate()
                    .map(|(g, (group, cmds))| {
                        card()
                            .px(px(12.))
                            .py(px(6.))
                            .flex()
                            .flex_col()
                            .child(div().py(px(4.)).child(section_title(*group)))
                            .children(cmds.iter().enumerate().map(|(i, cmd)| {
                                let added = has_command(&managed, cmd.command);
                                let elsewhere = !added && has_command(&text, cmd.command);
                                let (command, name) = (cmd.command, cmd.name);
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .py(px(4.))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .flex_col()
                                            .child(
                                                div().text_sm().text_color(c(TEXT)).child(cmd.name),
                                            )
                                            .child(faint(format!(
                                                "{} · {}",
                                                cmd.description, cmd.command
                                            ))),
                                    )
                                    .when(elsewhere, |d| d.child(chip("In your autoexec", MUTED)))
                                    .when(!elsewhere, |d| {
                                        d.child(
                                            Button::new(("cmd", g * 100 + i))
                                                .small()
                                                .when(added, |b| {
                                                    b.icon(IconName::Check).text_label("Added")
                                                })
                                                .when(!added, |b| {
                                                    b.icon(IconName::Plus).text_label("Add")
                                                })
                                                .when(!added, |b| b.primary())
                                                .disabled(busy)
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.command_toggle(command, name, cx)
                                                })),
                                        )
                                    })
                            }))
                    }),
            )
    }

    pub fn render_customize(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let profile = self.lib.profile().clone();
        if self.customize.loaded_for.as_ref() != Some(&profile.id) {
            self.customize_load(cx);
        }
        let Some(x) = self.customize.crosshair else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(muted("Reading the autoexec…"))
                .into_any_element();
        };
        self.sync_sliders(&x, window, cx);
        let busy = self.is_pending("autoexec") || self.customize.loading;
        let unsaved = self.customize.saved != Some(x);
        let has_block = self.customize.saved.is_some();
        let presets = Crosshair::PRESETS.iter().enumerate().map(|(i, (name, p))| {
            let p = *p;
            Button::new(("xh-preset", i))
                .small()
                .when(p == x, |b| b.primary())
                .when(p != x, |b| b.ghost())
                .text_label(*name)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.customize.crosshair = Some(p);
                    cx.notify();
                }))
        });
        let designer = card()
            .p(px(16.))
            .flex()
            .gap(px(20.))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(preview(&x))
                    .child(faint(format!(
                        "Shown at {PREVIEW_SCALE}× on a 1080p screen"
                    )))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(4.))
                            .w(px(PREVIEW))
                            .children(presets),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(
                                Button::new("xh-save")
                                    .small()
                                    .primary()
                                    .icon(Lucide::Save)
                                    .text_label(if unsaved { "Save to autoexec" } else { "Saved" })
                                    .disabled(busy || !unsaved)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.crosshair_save(cx)),
                                    ),
                            )
                            .child(
                                Button::new("xh-game")
                                    .small()
                                    .icon(IconName::FolderOpen)
                                    .text_label("Load from game settings")
                                    .tooltip(
                                        "Reads the crosshair you set up in Deadlock's own editor",
                                    )
                                    .disabled(busy)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.crosshair_from_game(cx)),
                                    ),
                            )
                            .when(has_block, |d| {
                                d.child(
                                    Button::new("xh-remove")
                                        .small()
                                        .ghost()
                                        .icon(IconName::Undo)
                                        .text_label("Stop overriding")
                                        .tooltip(
                                            "Removes Kombini's crosshair lines from the autoexec",
                                        )
                                        .disabled(busy)
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.crosshair_remove(cx)),
                                        ),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(self.render_crosshair_controls(x, busy, cx)),
            );
        div()
            .id("customize")
            .size_full()
            .overflow_y_scroll()
            .smooth(&self.smooth.get("customize"), cx)
            .p(px(20.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(heading("Crosshair"))
            .child(muted(
                "Saved to this profile's autoexec. Your own autoexec lines are left alone.",
            ))
            .child(designer)
            .child(div().pt(px(8.)).child(heading("Autoexec commands")))
            .child(self.render_command_library(busy, cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{CHANNELS, NUMBERS, channel, channel_value, set_channel};
    use crate::crosshair::Crosshair;

    #[test]
    fn channels_read_and_write_every_field() {
        let mut x = Crosshair::GAME;
        for i in 0..CHANNELS {
            let spec = channel(i);
            set_channel(i, &mut x, spec.max + 5.);
            assert_eq!(channel_value(i, &x), spec.max, "{}", spec.label);
            set_channel(i, &mut x, spec.min);
            assert_eq!(channel_value(i, &x), spec.min, "{}", spec.label);
        }
        set_channel(NUMBERS.len() + 4, &mut x, 127.6);
        assert_eq!(x.outline_color[1], 128);
        set_channel(3, &mut x, 0.333);
        assert_eq!(x.pip_opacity, 0.33);
    }
}
