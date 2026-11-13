//! Cards, separators and empty states (`card.tsx`, `separator.tsx`, `empty.tsx`).

use nsh_core::prelude::*;

use super::text::{TextRole, Tone, label};
use crate::theme::{metrics, palette};

/// A raised surface with the card border and radius around `content`.
pub fn card(content: impl IntoElement) -> impl IntoElement {
    view()
        .w_full()
        .p(px(20.0))
        .gap(px(16.0))
        .rounded(move || px(metrics().radius_xl))
        .border_1()
        .border_color(move || palette().border)
        .bg(move || palette().card)
        .child(content)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeparatorOrientation {
    Horizontal,
    Vertical,
}

pub fn separator(orientation: SeparatorOrientation) -> impl IntoElement {
    let line = view().bg(move || palette().border).flex_shrink_0();
    match orientation {
        SeparatorOrientation::Horizontal => line.h(px(1.0)).w_full(),
        SeparatorOrientation::Vertical => line.w(px(1.0)).h_full(),
    }
}

/// A centred title and description for a pane with nothing in it. Both already translated.
pub fn empty_state(title: String, description: String) -> impl IntoElement {
    view()
        .flex_1()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .p(px(24.0))
        .child(label(TextRole::Heading, Tone::Default, move || {
            title.clone()
        }))
        .child(label(TextRole::Caption, Tone::Muted, move || {
            description.clone()
        }))
}
