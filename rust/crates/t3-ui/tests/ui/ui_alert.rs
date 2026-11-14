//! UI-ALERT: primitive interaction and measured-layout tests, independent of CMP-BANNERS.

use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{Headless, LayoutNode};
use t3_ui::i18n::t;
use t3_ui::ui::alert::{self, ControlAlignment, Props, Slots, Variant, composer_banner as banner};
use t3_ui::ui::icon::Icon;

#[native_module("t3/ui-alert-test")]
fn native(registry: &mut Registry) {
    registry.register::<AlertFixture>();
}

#[component]
fn AlertFixture() -> impl IntoElement {
    t3_ui::theme::install(&t3_ui::theme::ThemeChoice::default());
    let enabled = signal(false);
    let dismissed = signal(false);
    let reviews = signal(0);
    let narrow = signal(false);
    let attached = signal(true);
    view()
        .w_full()
        .h_full()
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from(t("Enable actions"))))
                .on_press(move || enabled.set(true))
                .child(text(SharedString::from(t("Enable actions")))),
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
        .child(text(move || {
            SharedString::from(if dismissed.get() {
                t("Dismissed")
            } else {
                t("Not dismissed")
            })
        }))
        .child(text(move || {
            SharedString::from(t3_ui::i18n::tr(
                "Reviews: {count}",
                &[("count", t3_ui::i18n::Value::Int(reviews.get()))],
            ))
        }))
        .child(alert::alert(
            Props {
                variant: Variant::Warning,
                control_alignment: ControlAlignment::FirstLine,
                ..Props::default()
            },
            |context| Slots {
                icons: vec![Icon::CircleAlert],
                content: vec![
                    alert::alert_title(text(SharedString::from(t("Permission required"))))
                        .into_element(),
                    alert::alert_description(
                        context,
                        text(SharedString::from(t(
                            "Enable actions before dismissing this notice.",
                        ))),
                    )
                    .into_element(),
                ],
                actions: vec![
                    alert::alert_action(banner::dismiss(
                        t("Dismiss notice"),
                        move || !enabled.get(),
                        move || dismissed.set(true),
                    ))
                    .into_element(),
                ],
            },
        ))
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
                        banner::row_button(
                            context,
                            banner::Layout::Approval,
                            || t("Review approval"),
                            move || !enabled.get(),
                            move || reviews.update(|n| *n += 1),
                            |row| banner::RowSlots {
                                icon: Some(banner::icon(row, Icon::CircleAlert).into_element()),
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
                            },
                        )
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

// The application has no primitive gallery route; mount the owned primitive as a test root.
fn start(window: (f32, f32)) -> Headless {
    let mut sets = t3_ui::registries();
    sets.push(registry());
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

#[test]
fn actions_remain_inert_until_enabled_at_both_sizes() {
    for window in [PHONE, DESKTOP] {
        let mut app = start(window);
        app.wait_for("Permission required").expect("notice mounted");
        assert!(
            !app.screen()
                .enabled("role:button[Dismiss notice]")
                .expect("dismiss button")
        );
        let _ = app.press("role:button[Dismiss notice]");
        let _ = app.press("role:button[Review approval]");
        assert!(app.screen().shows("Not dismissed"));
        assert!(app.screen().shows("Reviews: 0"));
        app.press("role:button[Enable actions]").expect("enable");
        app.press("role:button[Review approval]").expect("review");
        app.wait_for("Reviews: 1").expect("one review");
        app.press("role:button[Dismiss notice]").expect("dismiss");
        app.wait_for("Dismissed").expect("dismiss callback ran");
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
        .expect("shrink the container, keeping the desktop viewport");
    let content = node(&app, "approval-copy");
    let action = node(&app, "approval-action");
    assert!(action.bounds.origin.y >= content.bounds.origin.y + content.bounds.size.height);
    assert!(action.bounds.origin.x + action.bounds.size.width <= 300.0);
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
