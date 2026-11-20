//! Small styled building blocks shared by the views.

use crate::theme::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// Height of toolbar controls and text fields (search, filters, buttons).
pub const BAR_H: f32 = 32.;

/// How far control text is raised. Segoe UI's tall ascent draws glyphs
/// below the centre of their box (about 0.1em), out of line with icons,
/// switches and dots next to them.
pub const TEXT_LIFT: f32 = 1.5;

/// Text inside a control (button, tab, chip), raised to sit centred.
pub fn ctl_text(text: impl Into<SharedString>) -> Div {
    div().relative().top(px(-TEXT_LIFT)).child(text.into())
}

/// Button labels that line up with their icon. Use instead of `.label()`.
pub trait ButtonText: Sized {
    fn text_label(self, text: impl Into<SharedString>) -> Self;
}

impl ButtonText for gpui_kit::component::button::Button {
    fn text_label(self, text: impl Into<SharedString>) -> Self {
        use gpui_kit::ParentElement as _;
        let text = text.into();
        self.accessibility_label(text.clone()).child(ctl_text(text))
    }
}

/// Track of a segmented control (list filters, sort orders).
pub fn segments() -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(2.))
        .h(px(BAR_H))
        .p(px(3.))
        .rounded(px(RADIUS))
        .bg(c(PANEL))
        .border_1()
        .border_color(c(BORDER))
}

/// One option of [`segments`]; add `.on_click`.
pub fn segment(
    id: impl Into<SharedString>,
    text: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .h_full()
        .min_h(px(24.))
        .flex()
        .items_center()
        .px(px(10.))
        .rounded(px(RADIUS - 2.))
        .cursor_pointer()
        .text_xs()
        .when(active, |d| d.bg(c(HOVER)).text_color(c(TEXT)))
        .when(!active, |d| d.text_color(c(MUTED)))
        .hover(|s| s.text_color(c(TEXT)))
        .child(ctl_text(text))
}

/// Pill that picks one of many (categories); add `.on_click`.
pub fn choice_chip(
    id: impl Into<SharedString>,
    text: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .flex()
        .items_center()
        .h(px(24.))
        .px(px(10.))
        .rounded(px(999.))
        .border_1()
        .cursor_pointer()
        .text_xs()
        .border_color(c(if active { ACCENT } else { BORDER }))
        .bg(ca(ACCENT, if active { 0.10 } else { 0.0 }))
        .text_color(c(if active { ACCENT } else { MUTED }))
        .hover(|s| s.text_color(c(TEXT)))
        .child(ctl_text(text))
}

/// Card whose picture fills it, for mod thumbnails (Discover, Heroes). Add
/// the text with a [`card_info`] child: it sits on a translucent panel at
/// the bottom, so any image aspect ratio looks right.
pub fn picture_card(
    id: impl Into<ElementId>,
    w: f32,
    h: f32,
    picture: Option<std::path::PathBuf>,
    border: u32,
) -> Stateful<Div> {
    div()
        .id(id)
        .relative()
        .flex_none()
        .w(px(w))
        .h(px(h))
        .flex()
        .flex_col()
        .justify_end()
        .rounded(px(10.))
        .overflow_hidden()
        .bg(c(PANEL))
        .border_1()
        .border_color(c(border))
        .child(match picture {
            Some(path) => img(path)
                .absolute()
                .top_0()
                .left_0()
                .w(px(w))
                .h(px(h))
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            None => div()
                .absolute()
                .top_0()
                .left_0()
                .w(px(w))
                .h(px(h))
                .bg(c(RAISED))
                .into_any_element(),
        })
}

/// Text panel of a [`picture_card`].
pub fn card_info() -> Div {
    div()
        .p(px(10.))
        .flex()
        .flex_col()
        .gap(px(4.))
        .bg(ca(WINDOW, 0.78))
        .border_t_1()
        .border_color(ca(TEXT, 0.08))
}

/// Thin vertical line between groups of toolbar controls.
pub fn vsep() -> Div {
    div().flex_none().w(px(1.)).h(px(18.)).bg(c(BORDER))
}

pub fn label(text: impl Into<SharedString>) -> Div {
    div().text_color(c(TEXT)).child(text.into())
}

pub fn muted(text: impl Into<SharedString>) -> Div {
    div().text_sm().text_color(c(MUTED)).child(text.into())
}

pub fn faint(text: impl Into<SharedString>) -> Div {
    div().text_xs().text_color(c(FAINT)).child(text.into())
}

pub fn heading(text: impl Into<SharedString>) -> Div {
    div()
        .text_lg()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(c(TEXT))
        .child(text.into())
}

pub fn section_title(text: impl Into<SharedString>) -> Div {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(c(FAINT))
        .child(text.into().to_uppercase())
}

/// Small colored chip (status, hero, tags).
pub fn chip(text: impl Into<SharedString>, color: u32) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .h(px(22.))
        .px(px(10.))
        .rounded(px(999.))
        .bg(ca(color, 0.16))
        .text_xs()
        .text_color(c(color))
        .child(ctl_text(text))
}

pub fn dot(color: u32) -> Div {
    div()
        .flex_none()
        .size(px(8.))
        .rounded(px(999.))
        .bg(c(color))
}

pub fn panel() -> Div {
    div()
        .bg(c(PANEL))
        .border_1()
        .border_color(c(BORDER))
        .rounded(px(10.))
}

pub fn card() -> Div {
    div()
        .bg(c(RAISED))
        .border_1()
        .border_color(c(BORDER))
        .rounded(px(RADIUS))
}

pub fn divider() -> Div {
    div().h(px(1.)).w_full().bg(c(BORDER))
}

/// Determinate (Some) or indeterminate (None, animated sweep) progress bar.
/// `id` must be unique among siblings.
pub fn progress_bar(id: impl Into<ElementId>, fraction: Option<f32>) -> Div {
    let track = div()
        .relative()
        .h(px(4.))
        .w_full()
        .rounded(px(2.))
        .bg(c(BORDER))
        .overflow_hidden();
    match fraction {
        Some(f) => track.child(
            div()
                .h_full()
                .w(relative(f.clamp(0.0, 1.0)))
                .rounded(px(2.))
                .bg(c(ACCENT)),
        ),
        None => track.child(crate::anim::sweep(id)),
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn format_count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1_000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// "3m ago" style relative time from a unix timestamp.
pub fn ago(ts: i64) -> String {
    let now = kombini_storage_now();
    let d = (now - ts).max(0);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", d / 60),
        3600..=86_399 => format!("{}h ago", d / 3600),
        86_400..=2_591_999 => format!("{}d ago", d / 86_400),
        _ => format!("{}mo ago", d / 2_592_000),
    }
}

fn kombini_storage_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{format_bytes, format_count};

    #[test]
    fn formatting() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_count(1500), "1.5k");
        assert_eq!(format_count(12), "12");
    }
}
