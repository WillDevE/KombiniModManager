//! Discover detail screenshots: a main image with a thumbnail strip, and a
//! lightbox modal showing the full-size uploads.

use crate::app::AppView;
use crate::modal::Modal;
use crate::theme::*;
use crate::widgets::*;
use gpui_kit::component::button::*;
use gpui_kit::component::{Disableable, IconName, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kombini_gamebanana::ModProfile;

/// Height of the main image in the detail pane.
const MAIN_H: f32 = 200.;
const STRIP_W: f32 = 64.;
const STRIP_H: f32 = 40.;

pub struct Lightbox {
    pub title: String,
    /// Full-size URL and caption per image.
    pub images: Vec<(String, Option<String>)>,
    pub index: usize,
}

fn caption(c: Option<&String>) -> Option<String> {
    c.map(|c| c.trim().to_owned()).filter(|c| !c.is_empty())
}

impl AppView {
    fn open_lightbox(&mut self, p: &ModProfile, index: usize, cx: &mut Context<Self>) {
        let images: Vec<(String, Option<String>)> = p
            .gallery()
            .iter()
            .map(|i| (i.original_url(), caption(i.caption.as_ref())))
            .collect();
        if images.is_empty() {
            return;
        }
        self.modal = Some(Modal::Lightbox(Box::new(Lightbox {
            title: p.name.clone(),
            index: index.min(images.len() - 1),
            images,
        })));
        cx.notify();
    }

    pub fn lightbox_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(Modal::Lightbox(lb)) = &mut self.modal {
            let n = lb.images.len() as isize;
            lb.index = (lb.index as isize + delta).rem_euclid(n.max(1)) as usize;
            cx.notify();
        }
    }

    pub fn lightbox_title(&self) -> String {
        match &self.modal {
            Some(Modal::Lightbox(lb)) if lb.images.len() > 1 => {
                format!("{} ({} of {})", lb.title, lb.index + 1, lb.images.len())
            }
            Some(Modal::Lightbox(lb)) => lb.title.clone(),
            _ => String::new(),
        }
    }

    /// The detail pane's screenshots, or `None` when the mod has none.
    pub fn render_gallery(&mut self, p: &ModProfile, cx: &mut Context<Self>) -> Option<AnyElement> {
        let images = p.gallery();
        if images.is_empty() {
            return None;
        }
        let index = self.discover.gallery.min(images.len() - 1);
        let main = self.thumb(&images[index].url(800), cx);
        let main_caption = caption(images[index].caption.as_ref());
        let strip: Vec<(usize, Option<std::path::PathBuf>)> = if images.len() > 1 {
            images
                .iter()
                .enumerate()
                .map(|(i, im)| (i, self.thumb(&im.url(220), cx)))
                .collect()
        } else {
            Vec::new()
        };
        let profile = p.clone();
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .id("gallery-main")
                        .w_full()
                        .h(px(MAIN_H))
                        .rounded(px(RADIUS))
                        .overflow_hidden()
                        .bg(c(RAISED))
                        .cursor_pointer()
                        .when_some(main, |d, path| {
                            d.child(img(path).size_full().object_fit(ObjectFit::Cover))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_lightbox(&profile, index, cx)
                        })),
                )
                .children(main_caption.map(faint))
                .when(!strip.is_empty(), |d| {
                    d.child(
                        div()
                            .id("gallery-strip")
                            .flex()
                            .gap(px(4.))
                            .overflow_x_scroll()
                            .children(strip.into_iter().map(|(i, path)| {
                                let active = i == index;
                                div()
                                    .id(("gallery-thumb", i))
                                    .flex_none()
                                    .w(px(STRIP_W))
                                    .h(px(STRIP_H))
                                    .rounded(px(4.))
                                    .overflow_hidden()
                                    .bg(c(RAISED))
                                    .border_2()
                                    .border_color(c(if active { ACCENT } else { BORDER }))
                                    .cursor_pointer()
                                    .when_some(path, |d, path| {
                                        d.child(img(path).size_full().object_fit(ObjectFit::Cover))
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.discover.gallery = i;
                                        cx.notify();
                                    }))
                            })),
                    )
                })
                .into_any_element(),
        )
    }

    pub fn render_lightbox(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(Modal::Lightbox(lb)) = &self.modal else {
            return div().into_any_element();
        };
        let n = lb.images.len();
        let (url, cap) = lb.images[lb.index.min(n - 1)].clone();
        let image = self.thumb(&url, cx);
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .w_full()
                    .h(px(520.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(RADIUS))
                    .overflow_hidden()
                    .bg(c(WINDOW))
                    .child(match image {
                        Some(path) => img(path)
                            .size_full()
                            .object_fit(ObjectFit::Contain)
                            .into_any_element(),
                        None => gpui_kit::component::spinner::Spinner::new().into_any_element(),
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        Button::new("lb-prev")
                            .small()
                            .icon(IconName::ChevronLeft)
                            .disabled(n < 2)
                            .on_click(cx.listener(|this, _, _, cx| this.lightbox_step(-1, cx))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_center()
                            .text_sm()
                            .text_color(c(MUTED))
                            .children(cap),
                    )
                    .child(
                        Button::new("lb-next")
                            .small()
                            .icon(IconName::ChevronRight)
                            .disabled(n < 2)
                            .on_click(cx.listener(|this, _, _, cx| this.lightbox_step(1, cx))),
                    ),
            )
            .into_any_element()
    }
}
