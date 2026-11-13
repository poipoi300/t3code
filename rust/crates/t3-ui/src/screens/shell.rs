//! The workspace shell, arranged by layout class from one tree.
//!
//! Medium and Wide: sidebar beside the top route. Compact: the route stack as pages, the sidebar
//! being the root page, so a deep link on a phone opens the thread with Back to the list.

use nsh_base::{nav_page, nav_stack};
use nsh_core::prelude::*;

use super::{router, sidebar};
use crate::i18n::t;
use crate::state::layout::{LayoutClass, layout_class};
use crate::state::nav::{Route, navigator};
use crate::state::panels::panels;
use crate::theme::palette;
use crate::ui::surface::{SeparatorOrientation, separator};

pub fn app_shell() -> impl IntoElement {
    let compact = computed(move || layout_class() == LayoutClass::Compact);
    dynamic(move || {
        if compact.get() {
            compact_shell().into_element()
        } else {
            split_shell().into_element()
        }
    })
}

fn split_shell() -> impl IntoElement {
    let nav = navigator();
    let panels = panels();
    let top = computed(move || nav.top());
    view()
        .flex_1()
        .flex_row()
        .min_h(px(0.0))
        .child(when(
            move || panels.sidebar_open.get(),
            move || {
                view()
                    .flex_row()
                    .h_full()
                    .w(move || px(panels.sidebar_width.get()))
                    .child(view().flex_1().h_full().child(sidebar::sidebar()))
                    .child(separator(SeparatorOrientation::Vertical))
            },
        ))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .h_full()
                .bg(move || palette().background)
                .child(dynamic(move || router::screen(&top.get()))),
        )
}

fn compact_shell() -> impl IntoElement {
    let nav = navigator();
    nav_stack().flex_1().child(each(
        move || nav.stack.get(),
        |route: &Route| route.path(),
        move |route, index| {
            let route = route.get_untracked();
            let page = match &route {
                Route::Home => nav_page()
                    .title(t("Threads"))
                    .header_visible(false)
                    .child(view().flex_1().child(sidebar::sidebar())),
                other => nav_page()
                    .title(t("Thread"))
                    .header_visible(false)
                    .child(view().flex_1().child(router::screen(other))),
            };
            page.on_dismiss(move || nav.truncate(index.get_untracked()))
        },
    ))
}
