//! Smooth mouse-wheel scrolling for scroll areas.
//!
//! GPUI moves a scroll area by whole lines per wheel notch, which jumps.
//! [`SmoothScroll::wheel`] runs right after GPUI has applied that jump (an
//! element's own wheel handler runs before the listeners added to it), puts
//! the view back where it was and glides to the new position instead. The
//! jump is never drawn: both happen before the next frame. Notches that
//! arrive mid-glide add to where it is heading. Pixel-precise input
//! (touchpads) is left alone.

use gpui_kit::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

const GLIDE: Duration = Duration::from_millis(170);
const FRAME: Duration = Duration::from_millis(8);

#[derive(Clone, Default)]
pub struct SmoothScroll {
    pub handle: ScrollHandle,
    state: Rc<RefCell<Glide>>,
}

#[derive(Default)]
struct Glide {
    /// Offset currently shown while gliding (y, 0 at the top, negative below).
    visual: Option<f32>,
    /// Where the running glide ends.
    target: Option<f32>,
    /// Last offset known to be shown when idle.
    settled: Option<f32>,
    /// Bumped by every notch so an older glide stops.
    generation: u64,
}

impl SmoothScroll {
    pub fn new() -> Self {
        Self::default()
    }

    /// Smooths an existing handle (a list that already tracks one).
    pub fn for_handle(handle: ScrollHandle) -> Self {
        Self {
            handle,
            state: Rc::default(),
        }
    }

    /// Wheel listener for the scroll area itself (`.on_scroll`).
    pub fn wheel<T: 'static>(&self, event: &ScrollEvent, window: &Window, cx: &mut Context<T>) {
        let offset = self.handle.offset();
        let now = f32::from(offset.y);
        let ScrollDelta::Lines(lines) = event.delta else {
            // Touchpad: GPUI already moved it; stop any glide.
            let mut g = self.state.borrow_mut();
            g.generation += 1;
            g.visual = None;
            g.target = None;
            g.settled = Some(now);
            return;
        };
        let max = f32::from(self.handle.max_offset().y).max(0.);
        let jump = lines.y * f32::from(window.line_height());
        let (from, target, generation) = {
            let mut g = self.state.borrow_mut();
            let from = g.visual.unwrap_or_else(|| match g.settled {
                // GPUI moved it from where it settled (it may have clamped
                // the jump at an end).
                Some(s) if (now - s) * jump >= 0. && (now - s).abs() <= jump.abs() + 0.5 => s,
                _ => (now - jump).clamp(-max, 0.),
            });
            let target = (g.target.unwrap_or(from) + jump).clamp(-max, 0.);
            g.generation += 1;
            if (target - from).abs() < 0.5 {
                g.visual = None;
                g.target = None;
                g.settled = Some(from);
            } else {
                g.visual = Some(from);
                g.target = Some(target);
            }
            (from, target, g.generation)
        };
        self.handle.set_offset(point(offset.x, px(from)));
        if (target - from).abs() < 0.5 {
            return;
        }
        let handle = self.handle.clone();
        let state = self.state.clone();
        let x = offset.x;
        cx.spawn(async move |this, cx| {
            let began = Instant::now();
            loop {
                cx.background_executor().timer(FRAME).await;
                if state.borrow().generation != generation {
                    return;
                }
                let t = (began.elapsed().as_secs_f32() / GLIDE.as_secs_f32()).min(1.);
                // Ease out: quick start, gentle stop.
                let eased = 1. - (1. - t).powi(3);
                let y = from + (target - from) * eased;
                handle.set_offset(point(x, px(y)));
                {
                    let mut g = state.borrow_mut();
                    if t >= 1. {
                        g.visual = None;
                        g.target = None;
                        g.settled = Some(y);
                    } else {
                        g.visual = Some(y);
                    }
                }
                if this.update(cx, |_, cx| cx.notify()).is_err() || t >= 1. {
                    return;
                }
            }
        })
        .detach();
    }
}

/// Smooth scroll areas of a view, by name, kept across frames.
#[derive(Default)]
pub struct SmoothAreas(HashMap<&'static str, SmoothScroll>);

impl SmoothAreas {
    pub fn get(&mut self, key: &'static str) -> SmoothScroll {
        self.0.entry(key).or_default().clone()
    }

    pub fn for_handle(&mut self, key: &'static str, handle: &ScrollHandle) -> SmoothScroll {
        self.0
            .entry(key)
            .or_insert_with(|| SmoothScroll::for_handle(handle.clone()))
            .clone()
    }
}

/// Makes a scroll area track `s` and glide on wheel notches.
pub trait Smooth: StatefulInteractiveElement + Sized {
    fn smooth<T: 'static>(self, s: &SmoothScroll, cx: &mut Context<T>) -> Self {
        let wheel = s.clone();
        self.track_scroll(&s.handle).on_scroll(
            cx.listener(move |_, ev: &ScrollEvent, window, cx| wheel.wheel(ev, window, cx)),
        )
    }
}

impl Smooth for Stateful<Div> {}
