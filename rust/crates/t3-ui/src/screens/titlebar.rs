//! Port of apps/web/src/workspaceTitlebar.ts and apps/desktop/src/window/DesktopWindow.ts.

use nsh_core::desktop::{TitleBarOptions, TitleBarStyle, WindowAttributes};
use nsh_core::element::desktop::{WindowRef, current_window};
use nsh_core::geometry::{Points, point};
use nsh_core::platform::Os;
use nsh_core::prelude::*;

use crate::i18n::Text;
use crate::state::panels::panels;
use crate::theme::tw;

/// DesktopWindow.getWindowTitleBarOptions: the desktop bridge is a window capability here.
pub fn window_title_bar_options(platform: Os, desktop: bool) -> Option<TitleBarOptions> {
    if !desktop {
        return None;
    }
    let mut options = TitleBarOptions::default();
    options.style = TitleBarStyle::Hidden;
    // DesktopWindow uses darwin's hiddenInset and traffic-light position; NSH names it Hidden.
    if platform == Os::MacOs {
        options.height = Some(px(52.0));
        options.controls_offset = Some(point(px(16.0), px(20.0)));
    } else {
        options.height = Some(px(40.0));
    }
    Some(options)
}

/// Install once in App. NSH paints and operates native traffic lights/caption buttons itself.
pub fn install_window_titlebar(title: Text) {
    let window = current_window();
    effect(move || {
        let Some(options) = window_title_bar_options(os(), window.state().is_some()) else {
            return;
        };
        let mut attributes = WindowAttributes::default();
        attributes.title = Some(title.resolve().to_string());
        attributes.title_bar = Some(options);
        window.set(attributes);
    });
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorkspaceTitlebarGeometry {
    pub height: Points,
    pub controls_top: Points,
    pub controls_left: Points,
    pub controls_right: Points,
    pub content_left: Points,
    pub native_controls_right: Points,
}

/// index.css's workspace variables and ui/sidebar.tsx's titlebar-content-left calculation.
pub fn workspace_titlebar_geometry(window: &WindowRef) -> WorkspaceTitlebarGeometry {
    let desktop = window.state().is_some();
    let area = window.title_bar();
    let safe = safe_area();
    // .wco applies only while the native overlay is visible; otherwise index.css uses 52px.
    let height = if desktop && os() != Os::MacOs && area.top.0 > 0.0 {
        area.top.0.max(40.0)
    } else {
        52.0
    };
    let controls_left = if desktop && area.left.0 > 0.0 {
        area.left.0 + 12.0
    } else {
        safe.left.0 + 12.0
    };
    let controls_right = if desktop {
        area.right.0 + 12.0
    } else {
        safe.right.0 + 12.0
    };
    WorkspaceTitlebarGeometry {
        height: px(height),
        controls_top: px(0.0),
        controls_left: px(controls_left),
        controls_right: px(controls_right),
        content_left: px(controls_left + 28.0 + 12.0),
        native_controls_right: px(if desktop && area.right.0 > 0.0 {
            area.right.0 + 12.0
        } else {
            0.0
        }),
    }
}

/// Native chrome's drag background, mounted first in App's relative root, behind its controls.
pub fn desktop_titlebar() -> impl IntoElement {
    let window = current_window();
    let reading = window.clone();
    when(
        move || reading.state().is_some(),
        move || {
            let window = window.clone();
            view()
                .test_id("desktop-titlebar")
                .app_class(tw("absolute inset-x-0 top-0"))
                .h(move || window.title_bar().top)
                .window_drag(true)
                .accessible(false)
        },
    )
}

/// Port of apps/web/src/components/WorkspacePageHeader.tsx WorkspacePageHeader.
pub fn workspace_page_header(
    reserve_native_controls: bool,
    children: impl IntoElement,
) -> impl IntoElement {
    let panels = panels();
    let native_window = current_window();
    let reading = native_window.clone();
    let geometry = computed(move || workspace_titlebar_geometry(&reading));
    // The height/inset classes reference subtree CSS environment variables; NSH reads its
    // reactive native geometry instead. Padding transitions await Panels' animation-setting seam.
    view()
        .test_id("workspace-page-header")
        .app_class(tw("flex shrink-0 items-center gap-3"))
        .h(move || geometry.get().height)
        .min_h(move || geometry.get().height)
        .max_h(move || geometry.get().height)
        .pl(move || {
            if !panels.sidebar_open.get() || window().width.0 < 768.0 {
                geometry.get().content_left
            } else {
                px(safe_area().left.0 + 12.0)
            }
        })
        .pr(move || {
            if reserve_native_controls && geometry.get().native_controls_right.0 > 0.0 {
                geometry.get().native_controls_right
            } else {
                px(safe_area().right.0 + 12.0)
            }
        })
        // index.css .drag-region: NSH hit-tests native drag/double-click/system-menu behavior.
        .window_drag(move || native_window.state().is_some())
        .child(children)
}

/// Port of index.css .drag-region's interactive descendants and [-webkit-app-region:no-drag].
pub fn titlebar_controls(children: impl IntoElement) -> impl IntoElement {
    view().window_drag(false).child(children)
}
