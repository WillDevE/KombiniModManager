//! Motion helpers on top of GPUI's `with_animation`.
//!
//! An animation plays once per element id while the element stays mounted, and
//! replays when the id changes or the element is mounted again. Callers key the
//! id by the state being revealed (tab, selected mod, modal kind) so changes
//! animate and unrelated re-renders do not.

use crate::theme::*;
use gpui_kit::*;
use std::time::Duration;

pub const FAST: Duration = Duration::from_millis(140);
pub const MEDIUM: Duration = Duration::from_millis(220);

fn ease(dur: Duration) -> Animation {
    Animation::new(dur).with_easing(ease_out_quint())
}

/// Fade in.
pub fn fade_in<E: IntoElement + Styled + 'static>(
    el: E,
    id: impl Into<ElementId>,
    dur: Duration,
) -> AnimationElement<E> {
    el.with_animation(id, ease(dur), |el, t| el.opacity(t))
}

/// Fade in while moving `dy` px into place (positive = rises from below,
/// negative = drops from above). The element must not be absolutely positioned.
pub fn rise_in<E: IntoElement + Styled + 'static>(
    el: E,
    id: impl Into<ElementId>,
    dy: f32,
    dur: Duration,
) -> AnimationElement<E> {
    el.with_animation(id, ease(dur), move |el, t| {
        el.opacity(t).relative().top(px(dy * (1.0 - t)))
    })
}

/// Like [`rise_in`] but starts after `delay` (used to stagger grid cards).
pub fn rise_in_delayed<E: IntoElement + Styled + 'static>(
    el: E,
    id: impl Into<ElementId>,
    dy: f32,
    delay: Duration,
    dur: Duration,
) -> AnimationElement<E> {
    let total = delay + dur;
    let start = delay.as_secs_f32() / total.as_secs_f32().max(f32::EPSILON);
    el.with_animation(id, Animation::new(total), move |el, t| {
        let local = if t <= start {
            0.0
        } else {
            (t - start) / (1.0 - start)
        };
        let e = 1.0 - (1.0 - local).powi(5);
        el.opacity(e).relative().top(px(dy * (1.0 - e)))
    })
}

/// Endless soft "breathing" opacity (status dots, skeletons).
pub fn breathe<E: IntoElement + Styled + 'static>(
    el: E,
    id: impl Into<ElementId>,
    min: f32,
    period: Duration,
) -> AnimationElement<E> {
    el.with_animation(
        id,
        Animation::new(period)
            .repeat()
            .with_easing(pulsating_between(min, 1.0)),
        |el, t| el.opacity(t),
    )
}

/// Indeterminate progress: a highlight sweeping across the track.
pub fn sweep(id: impl Into<ElementId>) -> AnimationElement<Div> {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .w(relative(0.3))
        .rounded(px(2.))
        .bg(c(ACCENT))
        .with_animation(
            id,
            Animation::new(Duration::from_millis(1100))
                .repeat()
                .with_easing(ease_in_out),
            |el, t| el.left(relative(-0.3 + 1.3 * t)),
        )
}
