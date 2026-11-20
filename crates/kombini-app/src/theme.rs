//! Design tokens applied on top of the gpui-component dark theme.

use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::*;

pub const WINDOW: u32 = 0x0e0e12;
pub const PANEL: u32 = 0x121216;
pub const RAISED: u32 = 0x17171c;
pub const HOVER: u32 = 0x1d1d24;
pub const BORDER: u32 = 0x24242c;
pub const PRIMARY: u32 = 0x2f6f4d;
pub const PRIMARY_HOVER: u32 = 0x37805a;
pub const ACCENT: u32 = 0x67d391;
pub const AMBER: u32 = 0xe0b04f;
pub const CORAL: u32 = 0xef6f61;
pub const INFO: u32 = 0x6fa8ef;
pub const TEXT: u32 = 0xe6e6ec;
pub const MUTED: u32 = 0x9a9aa5;
pub const FAINT: u32 = 0x6b6b75;

pub const ROW_H: f32 = 60.0;
/// Mod thumbnail size in list rows.
pub const THUMB: f32 = 44.0;
pub const TOPBAR_H: f32 = 56.0;
pub const RADIUS: f32 = 8.0;

pub fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

pub fn ca(hex: u32, alpha: f32) -> Hsla {
    let mut h: Hsla = rgb(hex).into();
    h.a = alpha;
    h
}

pub fn apply(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    Theme::update(cx, edit);
}

fn edit(theme: &mut Theme) {
    // Segoe UI: the Windows UI font (Inter is not installed by default).
    theme.font_family = "Segoe UI".into();
    theme.font_size = px(14.);
    theme.radius = px(RADIUS);
    theme.radius_lg = px(10.);
    theme.shadow = false;
    theme.notification.max_items = 3;
    // Always shown, so long lists (a mod with many files) read as scrollable.
    theme.scrollbar_mode = gpui_kit::component::scroll::ScrollbarMode::Always;
    let k = &mut theme.colors;
    k.background = c(WINDOW);
    k.foreground = c(TEXT);
    k.border = c(BORDER);
    k.input = c(BORDER);
    // Focus outline of inputs and buttons: a quiet tint, not a bright line.
    k.ring = ca(ACCENT, 0.32);
    k.caret = c(ACCENT);
    k.selection = ca(PRIMARY, 0.55);
    k.muted = c(RAISED);
    k.muted_foreground = c(MUTED);
    k.popover = c(RAISED);
    k.popover_foreground = c(TEXT);
    k.accent = c(HOVER);
    k.accent_foreground = c(TEXT);
    k.primary = c(PRIMARY);
    k.primary_hover = c(PRIMARY_HOVER);
    k.primary_active = c(PRIMARY);
    k.primary_foreground = c(0xffffff);
    k.button_primary = c(PRIMARY);
    k.button_primary_hover = c(PRIMARY_HOVER);
    k.button_primary_active = c(PRIMARY);
    k.button_primary_foreground = c(0xffffff);
    k.secondary = c(RAISED);
    k.secondary_hover = c(HOVER);
    k.secondary_active = c(BORDER);
    k.secondary_foreground = c(TEXT);
    k.button = c(RAISED);
    k.button_hover = c(HOVER);
    k.button_active = c(BORDER);
    k.button_foreground = c(TEXT);
    k.success = c(ACCENT);
    k.success_foreground = c(WINDOW);
    k.warning = c(AMBER);
    k.warning_foreground = c(WINDOW);
    k.danger = c(CORAL);
    k.danger_hover = c(0xf2857a);
    k.danger_foreground = c(0xffffff);
    k.info = c(INFO);
    k.switch = c(BORDER);
    k.switch_thumb = c(TEXT);
    k.progress_bar = c(ACCENT);
    k.skeleton = c(RAISED);
    k.scrollbar = ca(WINDOW, 0.0);
    k.scrollbar_thumb = c(BORDER);
    k.scrollbar_thumb_hover = c(FAINT);
    k.list = c(PANEL);
    k.list_hover = c(HOVER);
    k.list_active = ca(PRIMARY, 0.25);
    k.list_active_border = c(PRIMARY);
    k.title_bar = c(PANEL);
    k.title_bar_border = c(BORDER);
    k.status_bar = c(PANEL);
    k.status_bar_border = c(BORDER);
    k.overlay = ca(0x000000, 0.55);
    k.window_border = c(BORDER);
    k.link = c(ACCENT);
    k.link_hover = c(0x8fe3ad);
}
