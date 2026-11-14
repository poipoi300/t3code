//! UI-PANEL: the primitive in a host; PNL-HOST has not wired it into App yet.

use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::i18n::t;
use t3_ui::state::layout::{LayoutClass, layout_class};
use t3_ui::ui::panel::{panel, right_panel_sheet};

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
    let compact = computed(move || layout_class() == LayoutClass::Compact);
    view().w_full().h_full().child(dynamic(move || {
        if compact.get() {
            nsh_base::nav_stack()
                .flex_1()
                .child(nsh_base::nav_page().title(t("Thread")).child(base()))
                .child(panel(move || open.get(), 0, close, content))
                .into_element()
        } else {
            view()
                .flex_1()
                .flex_row()
                .child(base())
                .child(panel(move || open.get(), 0, close, content))
                .into_element()
        }
    }))
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
fn compact_panel_is_a_pushed_page_and_native_back_closes_it() {
    let mut app = start_host(PHONE);
    app.wait_for("Open panel").expect("thread page");
    app.press("role:button[Open panel]").expect("push");
    app.wait_for("Panel content").expect("panel page");
    assert!(
        !app.screen()
            .matches("role:dialog[Workspace panel]")
            .expect("dialog selector")
    );
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
    right_panel_sheet(
        move || open.get(),
        200,
        move || open.set(false),
        move || {
            nsh_base::button()
                .accessibility_label(Some(SharedString::from(t("Close panel"))))
                .on_press(move || open.set(false))
                .child(text(t("Panel content")))
        },
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
