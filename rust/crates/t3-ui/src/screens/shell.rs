//! The workspace shell, arranged by layout class from one tree.
//!
//! Medium and Wide: sidebar beside the top route. Compact: the route stack as pages, the sidebar
//! being the root page, so a deep link on a phone opens the thread with Back to the list.

use std::rc::Rc;

use nsh_base::{nav_page, nav_stack};
use nsh_core::prelude::*;
use t3_client::view_logic::shell::navigation::{Direction, HistoryAvailability};

use super::history_swipe::history_swipe_area;
use super::{router, sidebar};
use crate::i18n::t;
use crate::state::layout::{LayoutClass, layout_class};
use crate::state::nav::{Navigator, Route, navigator};
use crate::state::panels::panels;
use crate::state::url::UrlState;
use crate::theme::{palette, tw};
use crate::ui::surface::{SeparatorOrientation, separator};

pub fn app_shell() -> impl IntoElement {
    let compact = computed(move || layout_class() == LayoutClass::Compact);
    // Untracked: only the layout class switches the arrangement.
    dynamic(move || {
        let compact = compact.get();
        untracked(|| {
            if compact {
                compact_shell().into_element()
            } else {
                split_shell().into_element()
            }
        })
    })
}

fn split_shell() -> impl IntoElement {
    use crate::runtime::services::shell::{
        SidebarWidthStore, load_thread_sidebar_width, reset_thread_sidebar_width,
        thread_sidebar_rail,
    };
    use crate::ui::resizable::{Side, SidebarRailOptions, sidebar_rail};
    let nav = navigator();
    let panels = panels();
    let top = computed(move || nav.top());
    // The client's width record (AppSidebarLayout's `readInitialSidebarWidth`); the demo has none.
    let store = use_context::<SidebarWidthStore>();
    load_thread_sidebar_width(panels.sidebar_width, store.clone());
    let store = stored(store);
    let (availability, navigate) = history_navigation(nav);
    view()
        .flex_1()
        .flex_row()
        .min_h(px(0.0))
        .child(when(
            move || panels.sidebar_open.get(),
            move || {
                let tasks = crate::runtime::tasks::TaskOwner::here();
                // AppSidebarLayout's `<Sidebar resizable>` with its SidebarRail inside.
                view()
                    .flex_row()
                    .h_full()
                    .app_class(tw("relative"))
                    .w(move || px(panels.sidebar_width.get()))
                    .child(view().flex_1().h_full().child(sidebar::sidebar()))
                    .child(separator(SeparatorOrientation::Vertical))
                    .child(sidebar_rail(SidebarRailOptions {
                        side: Side::Left,
                        open: (move || panels.sidebar_open.get()).into_reactive(),
                        offcanvas: true,
                        resize: Some(thread_sidebar_rail(panels.sidebar_width, store.get())),
                        toggle: Rc::new(move || panels.sidebar_open.update(|open| *open = !*open)),
                        on_double_click: Some(Rc::new(move || {
                            reset_thread_sidebar_width(panels.sidebar_width, store.get(), tasks)
                        })),
                    }))
            },
        ))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .h_full()
                .bg(move || palette().background)
                .test_id("main-content")
                .child(history_swipe_area(
                    availability,
                    navigate,
                    dynamic(move || {
                        let route = top.get();
                        untracked(|| router::screen(&route))
                    }),
                )),
        )
}

type HistoryAvailabilityFn = Rc<dyn Fn() -> HistoryAvailability>;
type HistoryNavigateFn = Rc<dyn Fn(Direction)>;

/// The history HistorySwipeNavigation moves through: the URL service's when it is installed;
/// until then the navigator's route stack, the app's only history, which goes back only.
fn history_navigation(nav: Navigator) -> (HistoryAvailabilityFn, HistoryNavigateFn) {
    if let Some(urls) = use_context::<UrlState>() {
        return (
            Rc::new(move || HistoryAvailability {
                back: urls.can_go_back.get_untracked(),
                forward: urls.can_go_forward.get_untracked(),
            }),
            Rc::new(move |direction| {
                urls.go(direction);
            }),
        );
    }
    (
        Rc::new(move || HistoryAvailability {
            back: nav.stack.with_untracked(|stack| stack.len() > 1),
            forward: false,
        }),
        Rc::new(move |direction| {
            if matches!(direction, Direction::Back) {
                nav.back();
            }
        }),
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
