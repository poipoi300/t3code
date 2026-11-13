//! `components/ui/sidebar.tsx` `SidebarMenuButton`: the row every sidebar list is made of.

use nsh_core::prelude::*;

use crate::theme::{metrics, palette};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SidebarRowSize {
    #[default]
    Default,
    Lg,
}

/// A selectable sidebar row. `content` lays out inside the row's insets; `selected` and `muted`
/// follow the store. `label` is the translated accessible name.
pub fn sidebar_row(
    label: impl Fn() -> String + 'static,
    size: SidebarRowSize,
    selected: impl Fn() -> bool + Copy + 'static,
    muted: impl Fn() -> bool + Copy + 'static,
    on_press: impl Fn() + 'static,
    content: impl IntoElement,
) -> impl IntoElement {
    let height = move || {
        let m = metrics();
        px(match size {
            SidebarRowSize::Default => m.sidebar_row,
            SidebarRowSize::Lg => m.sidebar_row.max(48.0),
        })
    };
    nsh_base::button()
        .selected(selected)
        .accessibility_label(move || Some(SharedString::from(label())))
        .on_press(on_press)
        .w_full()
        .h(height)
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(move || px(metrics().sidebar_row_inset))
        .rounded(move || px(metrics().radius))
        .bg(move || {
            if selected() {
                palette().sidebar_row_selected
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .hover(|s| s.bg(palette().sidebar_row_hover))
        .active(|s| s.bg(palette().sidebar_row_active))
        .focus_visible(|s| s.ring(px(2.0)).ring_inset().ring_color(palette().ring))
        .text_color(move || {
            if muted() && !selected() {
                palette().sidebar_muted_foreground.opacity(0.75)
            } else {
                palette().sidebar_foreground
            }
        })
        .child(content)
}
