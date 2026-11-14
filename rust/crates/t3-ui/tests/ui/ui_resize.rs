//! UI-RESIZE: the sidebar rail as AppSidebarLayout mounts it (resizable, with the main content's
//! minimum width, a commit that persists, and a double press that resets).
use std::rc::Rc;

use crate::support::DESKTOP;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::{
    theme,
    ui::resizable::{RailResize, ResizeBounds, Side, SidebarRailOptions, sidebar_rail},
};

#[derive(Clone, PartialEq, Props)]
struct FixtureProps {
    resizable: bool,
}

#[component]
fn Fixture(props: FixtureProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let open = signal(true);
    let width = signal(300.0f32);
    let commits = signal(Vec::<i32>::new());
    let toggles = signal(0u32);
    let resets = signal(0u32);
    let resize = props.resizable.then(|| RailResize {
        width,
        bounds: Rc::new(|| ResizeBounds {
            min: 208.0,
            max: 640.0,
        }),
        // AppSidebarLayout: grow only while the main content keeps 640px of the 1280 window.
        should_accept: Some(Rc::new(|current: f32, next: f32| {
            next <= current || 1280.0 - next >= 640.0
        })),
        on_resize: Some(Rc::new(move |final_width: f32| {
            commits.update(|c| c.push(final_width.round() as i32))
        })),
    });
    view()
        .app_class(theme::tw("flex size-full"))
        .child(
            view()
                .app_class(theme::tw("relative h-full shrink-0"))
                .w(move || px(if open.get() { width.get() } else { 0.0 }))
                .child(sidebar_rail(SidebarRailOptions {
                    side: Side::Left,
                    open: (move || open.get()).into_reactive(),
                    offcanvas: true,
                    resize,
                    toggle: Rc::new(move || {
                        toggles.update(|n| *n += 1);
                        open.update(|o| *o = !*o);
                    }),
                    on_double_click: Some(Rc::new(move || {
                        resets.update(|n| *n += 1);
                        width.set(256.0);
                    })),
                })),
        )
        .child(
            view()
                .app_class(theme::tw("flex-1"))
                .child(text(move || format!("width {}", width.get().round())))
                .child(text(move || format!("commits {:?}", commits.get())))
                .child(text(move || format!("toggles {}", toggles.get())))
                .child(text(move || format!("resets {}", resets.get()))),
        )
}

fn start(resizable: bool) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(DESKTOP.0), px(DESKTOP.1)),
        ..EngineConfig::new(Root::new::<Fixture>(move || FixtureProps { resizable }))
    })
    .expect("the engine starts");
    app.wait_for("width 300").expect("mounted");
    app
}

fn tap(app: &mut Headless, selector: &str) {
    app.drag(selector, point(px(0.0), px(0.0))).expect("tap");
}

#[test]
fn dragging_resizes_within_bounds_and_commits_once_per_drag() {
    let mut app = start(true);
    app.wait_for("role:button[Resize Sidebar]")
        .expect("an open resizable rail");
    app.drag("role:button[Resize Sidebar]", point(px(100.0), px(0.0)))
        .expect("drag right");
    app.wait_for("width 400").expect("follows the pointer");
    app.wait_for("commits [400]").expect("one commit");
    // Growing past what leaves the main content 640px is refused; the last accepted width holds.
    app.drag("role:button[Resize Sidebar]", point(px(400.0), px(0.0)))
        .expect("drag far right");
    app.wait_for("commits [400, 640]")
        .expect("clamped by shouldAcceptWidth");
    app.drag("role:button[Resize Sidebar]", point(px(-900.0), px(0.0)))
        .expect("drag far left");
    app.wait_for("commits [400, 640, 208]")
        .expect("clamped to the minimum");
    assert!(app.shows("toggles 0"), "a drag never toggles");
}

#[test]
fn an_open_resizable_rail_ignores_presses_and_a_double_press_resets() {
    let mut app = start(true);
    // The rail takes the pointer as a responder: a tap is a touch that does not move.
    tap(&mut app, "role:button[Resize Sidebar]");
    app.settle();
    assert!(
        app.shows("toggles 0"),
        "pressing an open resizable rail does nothing"
    );
    tap(&mut app, "role:button[Resize Sidebar]");
    app.wait_for("resets 1").expect("double press resets");
    app.wait_for("width 256").expect("default width");
    // useResizeDrag commits on every pointer-up, a press included, as the web does.
    app.wait_for("commits [300, 300]")
        .expect("each tap commits");
}

#[test]
fn a_rail_without_resizing_toggles_the_sidebar() {
    let mut app = start(false);
    tap(&mut app, "role:button[Toggle Sidebar]");
    app.wait_for("toggles 1").expect("toggled");
}
