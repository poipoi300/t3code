//! UI-PANEL: the adaptive panel primitive in a host.

use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::i18n::t;
use t3_ui::ui::panel::{panel, panel_layout, right_panel_sheet};

#[derive(Clone, PartialEq, Props)]
struct HostFrameProps {
    open: Prop<bool>,
    base: Option<Slot>,
    children: Prop<Children>,
}

#[derive(Clone, Debug, PartialEq, Event)]
enum HostFrameEvent {
    Close,
}

#[component]
fn HostFrame(props: HostFrameProps, cx: Scope<HostFrameEvent>) -> impl IntoElement {
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_row()
        .child(view().flex_1().slot(props.base, |slot| slot))
        .child(panel(
            panel_layout(|| false),
            move || props.open.get(),
            0,
            move || cx.emit(HostFrameEvent::Close),
            props.children,
        ))
}

#[component]
fn Host() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let open = signal(false);
    let decline = signal(false);
    let requests = signal(0);
    let draft = signal(String::new());
    let close = move || {
        requests.update(|count| *count += 1);
        if !decline.get_untracked() {
            open.set(false);
        }
    };
    let content = move || {
        view()
            .flex_1()
            .test_id("panel-content")
            .child(text(t("Panel content")))
            .child(text(move || format!("panel requests:{}", requests.get())))
            .child(
                nsh_base::input()
                    .test_id("panel-draft")
                    .label(SharedString::from(t("Panel draft")))
                    .value(move || draft.get())
                    .on_change(move |value: &str| draft.set(value.to_owned())),
            )
            .child(
                nsh_base::button()
                    .accessibility_label(Some(SharedString::from(t("Close panel"))))
                    .child(text(t("Close panel")))
                    .on_press(close),
            )
    };
    let base = move || {
        view()
            .flex_1()
            .child(text(t("Thread content")))
            .child(text(move || format!("requests:{}", requests.get())))
            .child(
                nsh_base::button()
                    .accessibility_label(Some(SharedString::from(t("Open panel"))))
                    .child(text(t("Open panel")))
                    .on_press(move || open.set(true)),
            )
            .child(
                nsh_base::button()
                    .accessibility_label(Some(SharedString::from(t("Decline dismissal"))))
                    .child(text(t("Decline dismissal")))
                    .on_press(move || decline.set(true)),
            )
    };
    view().w_full().h_full().child(
        host_frame()
            .open(open)
            .on_close(close)
            .base(base())
            .child(content()),
    )
}

fn start_host(viewport: (f32, f32)) -> Headless {
    Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(viewport.0), px(viewport.1)),
        ..EngineConfig::new(Root::of::<Host>())
    })
    .expect("panel host starts")
}

#[test]
fn medium_sheet_has_the_web_accessible_name_and_escape_requests_close() {
    let mut app = start_host((800.0, 800.0));
    app.wait_for("Open panel").expect("host");
    app.press("role:button[Open panel]").expect("open");
    app.wait_for("role:dialog[Workspace panel]")
        .expect("RightPanelSheet.browser.test.tsx: names the open right-panel dialog");
    let nodes = app.screen().layout_nodes();
    let popup = nodes
        .iter()
        .find(|node| node.test_id.as_deref() == Some("workspace-panel-popup"))
        .expect("sheet popup");
    assert_eq!(popup.bounds.size.width, px(336.0));
    assert_eq!(popup.bounds.right(), px(800.0));
    app.type_text("id:panel-draft", "Unsaved review")
        .expect("edit");
    app.press_key("Escape").expect("dismiss sheet");
    app.wait_for("text:requests:1").expect("close callback");
    assert!(!app.screen().shows("Panel content"));
    app.press("role:button[Open panel]").expect("reopen");
    app.wait_for("Panel content").expect("sheet returns");
    assert_eq!(
        app.screen().input_text("id:panel-draft").expect("draft"),
        Some("Unsaved review")
    );
    for (keyboard, bottom) in [(300.0, 500.0), (0.0, 800.0)] {
        app.set_environment("keyboard", |environment| {
            environment.keyboard_height = px(keyboard);
        })
        .expect("software keyboard changes");
        let popup = app
            .screen()
            .layout_nodes()
            .into_iter()
            .find(|node| node.test_id.as_deref() == Some("workspace-panel-popup"))
            .expect("sheet popup above keyboard");
        assert_eq!(popup.bounds.bottom(), px(bottom));
    }
}

#[test]
fn controlled_dismissal_can_be_declined() {
    for viewport in [(800.0, 800.0), PHONE] {
        let mut app = start_host(viewport);
        app.wait_for("Open panel").expect("host");
        app.press("role:button[Decline dismissal]")
            .expect("decline");
        app.press("role:button[Open panel]").expect("open");
        app.wait_for("Panel content").expect("panel");
        app.type_text("id:panel-draft", "Keep this draft")
            .expect("edit");
        if viewport == PHONE {
            app.back().expect("request dismissal");
        } else {
            app.press_key("Escape")
                .expect("request dismissal from editor");
        }
        app.wait_for("text:panel requests:1")
            .expect("back callback");
        app.press("role:button[Close panel]")
            .expect("second request");
        app.wait_for("text:panel requests:2")
            .expect("button callback");
        assert!(app.screen().shows("Panel content"));
    }
}

#[test]
fn wide_panel_is_beside_the_thread_and_close_is_reversible() {
    let mut app = start_host(DESKTOP);
    app.wait_for("Open panel").expect("host");
    app.press("role:button[Open panel]").expect("open");
    app.wait_for("Panel content").expect("pane");
    assert!(
        !app.screen()
            .matches("role:dialog[Workspace panel]")
            .expect("dialog selector")
    );
    let nodes = app.screen().layout_nodes();
    let panel = nodes
        .iter()
        .find(|node| node.test_id.as_deref() == Some("panel-content"))
        .expect("pane");
    assert!(panel.bounds.origin.x.0 > 0.0, "pane sits beside the thread");
    app.press("role:button[Close panel]").expect("close");
    app.wait_for("text:requests:1").expect("closed");
    assert!(!app.screen().shows("Panel content"));
    app.press("role:button[Open panel]").expect("reopen");
    app.wait_for("Panel content").expect("pane returns");
}

#[test]
fn phone_panel_is_a_sheet_and_native_back_closes_it() {
    let mut app = start_host(PHONE);
    app.wait_for("Open panel").expect("thread page");
    app.press("role:button[Open panel]").expect("open");
    app.wait_for("role:dialog[Workspace panel]")
        .expect("phone sheet");
    app.back().expect("native back");
    app.wait_for("text:requests:1")
        .expect("route owner hears dismissal");
    app.wait_for("Open panel").expect("thread restored");
    assert!(!app.screen().shows("Panel content"));
}

#[test]
fn changing_layout_preserves_parent_owned_edits_and_does_not_request_close() {
    let mut app = start_host(DESKTOP);
    app.wait_for("Open panel").expect("host");
    app.press("role:button[Open panel]").expect("open");
    app.wait_for("Panel content").expect("pane");
    app.type_text("id:panel-draft", "Keep this comment")
        .expect("edit");
    for viewport in [(800.0, 800.0), PHONE, DESKTOP] {
        app.set_viewport(size(px(viewport.0), px(viewport.1)))
            .expect("resize");
        app.wait_for("Panel content").expect("adaptive panel");
        assert_eq!(
            app.screen().input_text("id:panel-draft").expect("draft"),
            Some("Keep this comment")
        );
    }
    app.wait_for("text:requests:0")
        .expect("resize never dismisses");
}

#[component]
fn AnimatedSheet() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let open = signal(true);
    animated_frame()
        .open(open)
        .on_close(move || open.set(false))
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from(t("Close panel"))))
                .on_press(move || open.set(false))
                .child(text(t("Panel content"))),
        )
}

#[derive(Clone, PartialEq, Props)]
struct AnimatedFrameProps {
    open: Prop<bool>,
    children: Prop<Children>,
}

#[derive(Clone, Debug, PartialEq, Event)]
enum AnimatedFrameEvent {
    Close,
}

#[component]
fn AnimatedFrame(props: AnimatedFrameProps, cx: Scope<AnimatedFrameEvent>) -> impl IntoElement {
    right_panel_sheet(
        move || props.open.get(),
        200,
        move || cx.emit(AnimatedFrameEvent::Close),
        props.children,
    )
}

#[test]
fn animated_exit_finishes_without_a_timer_owned_by_the_screen() {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(800.0), px(800.0)),
        clock: nsh_engine::Clock::manual(std::time::SystemTime::UNIX_EPOCH),
        ..EngineConfig::new(Root::of::<AnimatedSheet>())
    })
    .expect("animated sheet");
    app.wait_for("Panel content").expect("open");
    app.press("role:button[Close panel]").expect("close");
    app.wait_for_gone("Panel content", std::time::Duration::from_secs(2))
        .expect("animation completes");
}

#[derive(Clone, Copy)]
struct RetentionOpen(Signal<bool>);

#[derive(Clone, PartialEq, Props)]
struct RetentionFrameProps {
    open: Prop<bool>,
    children: Prop<Children>,
}

#[derive(Clone, Debug, PartialEq, Event)]
enum RetentionFrameEvent {
    Close,
}

#[component]
fn RetentionFrame(props: RetentionFrameProps, cx: Scope<RetentionFrameEvent>) -> impl IntoElement {
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_row()
        .child(view().flex_1().child(text(t("Thread content"))))
        .child(panel(
            panel_layout(|| false),
            move || props.open.get(),
            0,
            move || cx.emit(RetentionFrameEvent::Close),
            props.children,
        ))
}

#[component]
fn RetentionContent() -> impl IntoElement {
    let open = use_context::<RetentionOpen>().expect("retention host").0;
    let expanded = signal(false);
    view()
        .flex_1()
        .min_h(px(0.0))
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from(t("Expand file"))))
                .on_press(move || expanded.set(true))
                .child(text(t("Expand file"))),
        )
        .child(when(expanded, || text(t("Expanded file details"))))
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from(t("Hide panel"))))
                .on_press(move || open.set(false))
                .child(text(t("Hide panel"))),
        )
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.0))
                .test_id("retention-scroll")
                .child(view().child((0..80).map(|index| {
                    view()
                        .h(px(40.0))
                        .test_id(&format!("retention-row-{index}"))
                        .child(text(index.to_string()))
                }))),
        )
}

#[component]
fn RetentionHost() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let open = signal(false);
    provide_context(RetentionOpen(open));
    view()
        .w_full()
        .h_full()
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from(t("Open retained panel"))))
                .on_press(move || open.set(true))
                .child(text(t("Open retained panel"))),
        )
        .child(
            retention_frame()
                .open(open)
                .on_close(move || open.set(false))
                .child(retention_content()),
        )
}

fn retained_scroll(app: &Headless) -> (nsh_core::NodeId, Points) {
    let nodes = app.screen().layout_nodes();
    let scroller = nodes
        .iter()
        .find(|node| node.test_id.as_deref() == Some("retention-scroll"))
        .expect("retained scroll view");
    assert!(
        scroller.bounds.size.height > px(0.0),
        "the scroll viewport is visible and sized"
    );
    let first = nodes
        .iter()
        .find(|node| node.test_id.as_deref() == Some("retention-row-0"))
        .expect("first file row");
    (
        scroller.id,
        scroller.bounds.origin.y - first.bounds.origin.y,
    )
}

#[test]
fn retains_native_scroll_and_child_local_expansion_across_close_and_layout_changes() {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(800.0), px(800.0)),
        ..EngineConfig::new(Root::of::<RetentionHost>())
    })
    .expect("retention host starts");
    app.wait_for("Open retained panel").expect("closed panel");
    app.press("role:button[Open retained panel]").expect("open");
    app.wait_for("Expand file").expect("file view");
    app.press("role:button[Expand file]").expect("expand");
    app.scroll("id:retention-scroll", px(400.0))
        .expect("scroll files");
    let original = retained_scroll(&app);
    assert_eq!(original.1, px(400.0));

    for viewport in [(800.0, 800.0), DESKTOP, PHONE, (800.0, 800.0)] {
        app.set_viewport(size(px(viewport.0), px(viewport.1)))
            .expect("change layout");
        app.wait_for("Expanded file details")
            .expect("expansion survives layout change");
        assert_eq!(
            retained_scroll(&app),
            original,
            "layout retains native scroll node and offset"
        );
        app.press("role:button[Hide panel]").expect("close");
        app.wait_for("Open retained panel").expect("closed");
        assert!(!app.screen().shows("Expanded file details"));
        app.press("role:button[Open retained panel]")
            .expect("reopen");
        app.wait_for("Expanded file details")
            .expect("expansion survives reopening");
        assert_eq!(
            retained_scroll(&app),
            original,
            "reopen retains native scroll node and offset"
        );
    }
}
