//! UI-ALERT: slot decisions and measured layout, independent of CMP-BANNERS.

use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{Headless, LayoutNode};
use t3_ui::i18n::t;
use t3_ui::ui::alert::{self, ControlAlignment, Props, Slots, Variant, composer_banner as banner};
use t3_ui::ui::icon::Icon;

#[component]
fn CustomIndicator() -> impl IntoElement {
    let step = signal(0_u32);
    nsh_base::button()
        .accessibility_label(Some(SharedString::from(t("Advance indicator"))))
        .on_press(move || step.update(|n| *n += 1))
        .w(px(16.0))
        .h(px(16.0))
        .child(
            text(move || SharedString::from(t3_ui::i18n::formats().decimal(f64::from(step.get()))))
                .test_id("indicator-value"),
        )
}

#[component]
fn AlertFixture() -> impl IntoElement {
    t3_ui::theme::install(&t3_ui::theme::ThemeChoice::default());
    let actions = signal(true);
    let narrow = signal(false);
    let attached = signal(true);
    view()
        .w_full()
        .h_full()
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from(t("Toggle action"))))
                .on_press(move || actions.update(|shown| *shown = !*shown))
                .child(text(SharedString::from(t("Toggle action")))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from(t("Use catalogue theme"))))
                .on_press(|| {
                    t3_ui::theme::install(&t3_ui::theme::ThemeChoice {
                        theme: "ocean".into(),
                        ..Default::default()
                    })
                })
                .child(text(SharedString::from(t("Use catalogue theme")))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from(t("Narrow banner"))))
                .on_press(move || narrow.set(true))
                .child(text(SharedString::from(t("Narrow banner")))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from(t("Hide notices"))))
                .on_press(move || attached.set(false))
                .child(text(SharedString::from(t("Hide notices")))),
        )
        .child(view().test_id("tested-alert").child(dynamic(move || {
            let has_actions = actions.get();
            // Rebuild only for slot membership; theme and layout bindings keep the custom child.
            untracked(|| {
                alert::alert(
                    Props {
                        variant: Variant::Warning,
                        control_alignment: ControlAlignment::FirstLine,
                        ..Props::default()
                    },
                    |_context| Slots {
                        icons: vec![CustomIndicator().into_element()],
                        content: vec![
                            alert::alert_title(
                                text(SharedString::from(t("Permission required")))
                                    .test_id("alert-title"),
                            )
                            .into_element(),
                        ],
                        actions: has_actions
                            .then(|| {
                                alert::alert_action(
                                    view()
                                        .w(px(48.0))
                                        .h(px(40.0))
                                        .child(text(SharedString::from(t("Review")))),
                                )
                                .into_element()
                            })
                            .into_iter()
                            .collect(),
                    },
                )
            })
        })))
        .child(
            view()
                .w(move || {
                    px(if narrow.get() {
                        300.0
                    } else {
                        window().width.0
                    })
                })
                .child(banner::root(
                    banner::Props {
                        surface: banner::SurfaceProps {
                            detached: true,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    |context| {
                        banner::row(context, banner::Layout::Approval, |row| banner::RowSlots {
                            icon: Some(banner::lucide_icon(row, Icon::CircleAlert).into_element()),
                            content: Some(
                                banner::content(
                                    row,
                                    text(SharedString::from(t("A tool needs your approval")))
                                        .test_id("approval-copy"),
                                )
                                .into_element(),
                            ),
                            actions: Some(
                                banner::actions(
                                    row,
                                    text(SharedString::from(t("Review")))
                                        .test_id("approval-action"),
                                )
                                .into_element(),
                            ),
                        })
                    },
                )),
        )
        .child(banner::dock(
            banner::AttachmentProps::default(),
            move || attached.get(),
            text(SharedString::from(t("Attached notices"))),
        ))
        .child(banner::attachment(
            banner::AttachmentProps {
                in_column: true,
                last: true,
            },
            banner::root(banner::Props::default(), |context| {
                banner::row(context, banner::Layout::Inline, |row| banner::RowSlots {
                    content: Some(
                        banner::content(row, text(SharedString::from(t("Next attachment"))))
                            .into_element(),
                    ),
                    ..Default::default()
                })
            }),
        ))
}

// No primitive gallery route exists; this fixture mounts the owned components directly.
fn start(window: (f32, f32)) -> Headless {
    let mut sets = t3_ui::registries();
    let mut fixtures = Registry::new();
    fixtures.register::<AlertFixture>();
    fixtures.register::<CustomIndicator>();
    sets.push(fixtures);
    Headless::start(EngineConfig {
        ui_sets: sets,
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<AlertFixture>())
    })
    .expect("primitive fixture starts")
}

fn node(app: &Headless, id: &str) -> LayoutNode {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|n| n.test_id.as_deref() == Some(id))
        .expect("slot is laid out")
}

fn indicator_value(app: &Headless) -> String {
    node(app, "indicator-value")
        .text
        .expect("indicator is text")
        .text
        .visible_text()
        .into_owned()
}

#[test]
fn custom_icon_element_keeps_its_state_across_theme_and_viewport_changes() {
    let mut app = start(DESKTOP);
    app.wait_for("Permission required").expect("alert mounted");
    app.press("role:button[Advance indicator]")
        .expect("custom icon is interactive");
    assert_eq!(indicator_value(&app), "1");
    app.press("role:button[Use catalogue theme]")
        .expect("switch theme");
    app.press("role:button[Advance indicator]")
        .expect("same custom element");
    assert_eq!(indicator_value(&app), "2");
    app.set_viewport(size(px(PHONE.0), px(PHONE.1)))
        .expect("resize to phone");
    app.press("role:button[Advance indicator]")
        .expect("custom element after resize");
    assert_eq!(indicator_value(&app), "3");
}

#[test]
fn first_line_clearance_follows_action_presence_at_both_sizes() {
    for (window, clearance, top_padding) in [(PHONE, 8.0, 4.0), (DESKTOP, 4.0, 2.0)] {
        let mut app = start(window);
        app.wait_for("Permission required").expect("alert mounted");
        let with_action = node(&app, "tested-alert").bounds;
        let title_with_action = node(&app, "alert-title").bounds;
        app.press("role:button[Toggle action]")
            .expect("remove action slot");
        let without_action = node(&app, "tested-alert").bounds;
        let title_without_action = node(&app, "alert-title").bounds;
        assert_eq!(
            with_action.size.height - without_action.size.height,
            px(clearance)
        );
        assert_eq!(
            title_with_action.origin.y - title_without_action.origin.y,
            px(top_padding)
        );
        app.press("role:button[Toggle action]")
            .expect("restore action slot");
        assert_eq!(node(&app, "tested-alert").bounds, with_action);
    }
}

#[test]
fn approval_actions_follow_the_measured_container_when_it_shrinks() {
    let mut app = start(DESKTOP);
    app.wait_for("A tool needs your approval")
        .expect("approval mounted");
    let content = node(&app, "approval-copy");
    let action = node(&app, "approval-action");
    assert!(action.bounds.origin.x > content.bounds.origin.x);
    assert!(action.bounds.origin.y < content.bounds.origin.y + content.bounds.size.height);
    app.press("role:button[Narrow banner]")
        .expect("shrink container, retaining desktop viewport");
    let content = node(&app, "approval-copy");
    let action = node(&app, "approval-action");
    assert!(action.bounds.origin.y >= content.bounds.origin.y + content.bounds.size.height);
    assert!(action.bounds.origin.x + action.bounds.size.width <= px(300.0));
    app.set_viewport(size(px(PHONE.0), px(PHONE.1)))
        .expect("resize to phone");
    let content = node(&app, "approval-copy");
    let action = node(&app, "approval-action");
    assert!(action.bounds.origin.y >= content.bounds.origin.y + content.bounds.size.height);
}

#[test]
fn hiding_the_notice_dock_keeps_the_following_attachment_visible() {
    for window in [PHONE, DESKTOP] {
        let mut app = start(window);
        app.wait_for("Attached notices").expect("dock shown");
        app.press("role:button[Hide notices]").expect("hide dock");
        assert!(!app.screen().shows("Attached notices"));
        assert!(app.screen().shows("Next attachment"));
    }
}
