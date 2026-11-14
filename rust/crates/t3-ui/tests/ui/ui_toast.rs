//! UI-TOAST: buffered delivery, interaction, scope and lifetime through the real NSH hosts.

use std::rc::Rc;
use std::time::{Duration, SystemTime};

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_ui::i18n::t;
use t3_ui::ui::toast::{
    self, Action, Position, ThreadToastData, ToastManager, ToastOptions, Variant,
};

use crate::support::{DESKTOP, PHONE};

fn key(environment: &str) -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId(environment.into()),
        thread: ThreadId("same-id".into()),
    }
}

fn error_options(details: &'static str) -> ToastOptions {
    ToastOptions {
        title: "Request failed".into(),
        description: Some("Permission denied".into()),
        variant: Variant::Error,
        timeout: None,
        data: ThreadToastData {
            expandable_content: Some(Rc::new(move || text(t(details)).into_element())),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[native_module("t3/tests/ui-toast")]
fn fixture(registry: &mut Registry) {
    registry.register::<ToastFixture>();
}

#[component]
fn ToastFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let manager = ToastManager::default();
    let active = signal(Some(key("a")));
    let count = signal(0);
    let anchor = element_ref();
    let buffered = manager.add(ToastOptions {
        title: "Raised before the first render".into(),
        timeout: None,
        ..Default::default()
    });
    let clear = manager.clone();
    let error = manager.clone();
    let raised_error = signal(None::<toast::ToastId>);
    let update_error = manager.clone();
    let timed = manager.clone();
    let scoped = manager.clone();
    let action = manager.clone();
    view()
        .w_full()
        .h_full()
        .child(
            view()
                .app_class(t3_ui::theme::tw("p-8 gap-4"))
                .child(
                    nsh_base::button()
                        .accessibility_label(Some(SharedString::from(t("Clear buffered toast"))))
                        .on_press(move || {
                            clear.close(buffered);
                        })
                        .child(text(t("Clear buffered toast"))),
                )
                .child(
                    nsh_base::button()
                        .accessibility_label(Some(SharedString::from(t("Raise error"))))
                        .on_press(move || {
                            raised_error
                                .set(Some(error.add(error_options("Server error details"))));
                        })
                        .child(text(t("Raise error"))),
                )
                .child(
                    nsh_base::button()
                        .accessibility_label(Some(SharedString::from(t("Update error"))))
                        .on_press(move || {
                            if let Some(id) = raised_error.get_untracked() {
                                update_error.update(id, error_options("Updated server details"));
                            }
                        })
                        .child(text(t("Update error"))),
                )
                .child(
                    nsh_base::button()
                        .accessibility_label(Some(SharedString::from(t("Raise timed toast"))))
                        .on_press(move || {
                            timed.add(ToastOptions {
                                title: "Timed notification".into(),
                                timeout: Some(Duration::from_secs(2)),
                                action: Some(Action {
                                    label: "Hold notification".into(),
                                    disabled: true,
                                    on_press: Rc::new(move || count.set(100)),
                                }),
                                data: ThreadToastData {
                                    secondary_action: Some(Action {
                                        label: "Keep open".into(),
                                        disabled: false,
                                        on_press: Rc::new(move || {
                                            count.update(|value| *value += 1)
                                        }),
                                    }),
                                    ..Default::default()
                                },
                                ..Default::default()
                            });
                        })
                        .child(text(t("Raise timed toast"))),
                )
                .child(
                    nsh_base::button()
                        .accessibility_label(Some(SharedString::from(t("Raise scoped toast"))))
                        .on_press(move || {
                            scoped.add(ToastOptions {
                                title: "Scoped notification".into(),
                                timeout: None,
                                data: ThreadToastData {
                                    thread_ref: Some(key("a")),
                                    ..Default::default()
                                },
                                ..Default::default()
                            });
                        })
                        .child(text(t("Raise scoped toast"))),
                )
                .child(
                    nsh_base::button()
                        .accessibility_label(Some(SharedString::from(t("Other environment"))))
                        .on_press(move || active.set(Some(key("b"))))
                        .child(text(t("Other environment"))),
                )
                .child(
                    nsh_base::button()
                        .accessibility_label(Some(SharedString::from(t("Return to thread"))))
                        .on_press(move || active.set(Some(key("a"))))
                        .child(text(t("Return to thread"))),
                )
                .child(
                    nsh_base::button()
                        .accessibility_label(Some(SharedString::from(t("Raise action toast"))))
                        .on_press(move || {
                            action.add(toast::stacked_thread_toast(ToastOptions {
                                title: "Thread archived".into(),
                                timeout: None,
                                action: Some(Action {
                                    label: "Undo".into(),
                                    disabled: false,
                                    on_press: Rc::new(move || count.update(|value| *value += 1)),
                                }),
                                data: ThreadToastData {
                                    on_close: Some(Rc::new(move || {
                                        count.update(|value| *value += 10)
                                    })),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }));
                        })
                        .child(text(t("Raise action toast"))),
                )
                .child(
                    nsh_base::button()
                        .element_ref(&anchor)
                        .accessibility_label(Some(SharedString::from(t("Copy anchor"))))
                        .on_press(move || {
                            toast::show_anchored_copy_success_toast(Some(&anchor));
                        })
                        .child(text(t("Copy anchor"))),
                )
                .child(text(move || {
                    SharedString::from(t3_ui::i18n::tr(
                        "Actions: {count}",
                        &[("count", t3_ui::i18n::Value::Int(count.get()))],
                    ))
                })),
        )
        .child(toast::toasts(manager, Position::TopRight, move || {
            active.get()
        }))
        .child(toast::anchored_toasts(
            toast::anchored_toast_manager(),
            move || active.get(),
        ))
}

fn start(window: (f32, f32)) -> Headless {
    let mut sets = t3_ui::registries();
    sets.push(registry());
    Headless::start(EngineConfig {
        ui_sets: sets,
        viewport: size(px(window.0), px(window.1)),
        clock: nsh_core::time::Clock::manual(SystemTime::UNIX_EPOCH),
        ..EngineConfig::new(Root::of::<ToastFixture>())
    })
    .expect("toast fixture")
}

fn clear(app: &mut Headless) {
    app.wait_for("Raised before the first render")
        .expect("buffer delivered");
    app.press("role:button[Clear buffered toast]")
        .expect("clear");
    app.wait_for_gone("Raised before the first render", Duration::from_secs(1))
        .expect("closed");
}

#[test]
fn buffered_toasts_survive_until_a_host_mounts_at_both_sizes() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window);
        clear(&mut app);
        app.press("role:button[Raise action toast]").expect("raise");
        app.wait_for("Thread archived").expect("toast");
        app.press("role:button[Undo]").expect("action");
        app.wait_for("Actions: 1")
            .expect("action ran exactly once without close callback");
        app.wait_for_gone("Thread archived", Duration::from_secs(1))
            .expect("primary action closes");
        app.press("role:button[Raise action toast]")
            .expect("raise again");
        app.wait_for("Thread archived").expect("toast");
        app.press("role:button[Dismiss notification]")
            .expect("dismiss");
        app.wait_for("Actions: 11")
            .expect("explicit dismissal calls onClose");
    }
}

#[test]
fn error_details_expand_and_collapse_without_dismissing_the_notification() {
    let mut app = start(PHONE);
    clear(&mut app);
    app.press("role:button[Raise error]").expect("raise error");
    app.wait_for("Permission denied").expect("description");
    assert!(!app.screen().shows("Server error details"));
    app.press("role:button[Show details]").expect("expand");
    app.wait_for("Server error details").expect("details");
    app.press("role:button[Update error]")
        .expect("update same notification");
    app.wait_for("Updated server details")
        .expect("expansion survives an update");
    app.press("role:button[Hide details]").expect("collapse");
    app.wait_for_gone("Updated server details", Duration::from_secs(1))
        .expect("details gone");
    assert!(app.screen().shows("Request failed"));
}

#[test]
fn same_thread_ids_in_other_environments_do_not_expose_scoped_notifications() {
    let mut app = start(DESKTOP);
    clear(&mut app);
    app.press("role:button[Raise scoped toast]").expect("raise");
    app.wait_for("Scoped notification")
        .expect("own environment");
    app.press("role:button[Other environment]").expect("switch");
    app.wait_for_gone("Scoped notification", Duration::from_secs(1))
        .expect("hidden");
    app.press("role:button[Return to thread]").expect("return");
    app.wait_for("Scoped notification")
        .expect("scope preserved");
}

#[test]
fn hovered_toasts_keep_their_remaining_timeout_and_disabled_actions_stay_inert() {
    let mut app = start(DESKTOP);
    clear(&mut app);
    app.press("role:button[Raise timed toast]").expect("raise");
    app.wait_for("Timed notification").expect("timed toast");
    app.hover("id:toast-title").expect("hover");
    app.run_for(Duration::from_secs(10))
        .expect("manual clock while hovered");
    assert!(app.screen().shows("Timed notification"));
    let _ = app.press("role:button[Hold notification]");
    assert!(app.screen().shows("Actions: 0"));
    app.hover("role:button[Return to thread]")
        .expect("leave toast");
    app.run_for(Duration::from_secs(3))
        .expect("remaining timeout");
    assert!(!app.screen().shows("Timed notification"));
}

#[test]
fn focused_toast_controls_pause_dismissal_until_focus_leaves_the_stack() {
    let mut app = start(DESKTOP);
    clear(&mut app);
    app.press("role:button[Raise timed toast]").expect("raise");
    app.wait_for("Timed notification").expect("timed toast");
    app.press("role:button[Keep open]")
        .expect("focus secondary action");
    app.wait_for("Actions: 1")
        .expect("secondary action does not close");
    app.hover("role:button[Return to thread]")
        .expect("pointer outside toast");
    app.run_for(Duration::from_secs(10))
        .expect("manual clock while focused");
    assert!(app.screen().shows("Timed notification"));
    app.press("role:button[Return to thread]")
        .expect("focus outside");
    app.run_for(Duration::from_secs(3))
        .expect("remaining timeout");
    assert!(!app.screen().shows("Timed notification"));
}

#[test]
fn background_time_does_not_consume_a_visible_notification_timeout() {
    let mut app = start(PHONE);
    clear(&mut app);
    app.press("role:button[Raise timed toast]").expect("raise");
    app.wait_for("Timed notification").expect("timed toast");
    app.set_environment("background", |env| {
        env.lifecycle = nsh_core::platform::Lifecycle::Background
    })
    .expect("background");
    app.run_for(Duration::from_secs(10))
        .expect("background time");
    assert!(app.screen().shows("Timed notification"));
    app.set_environment("foreground", |env| {
        env.lifecycle = nsh_core::platform::Lifecycle::Active
    })
    .expect("foreground");
    app.run_for(Duration::from_secs(3))
        .expect("visible budget expires");
    assert!(!app.screen().shows("Timed notification"));
}

#[test]
fn anchored_copy_feedback_is_tooltip_shaped_and_expires() {
    let mut app = start(PHONE);
    clear(&mut app);
    app.press("role:button[Copy anchor]")
        .expect("copy success helper");
    app.wait_for("Copied!").expect("anchored feedback");
    assert!(!app.screen().shows("Dismiss notification"));
    app.hover("role:button[Return to thread]")
        .expect("outside feedback");
    app.run_for(Duration::from_secs(2))
        .expect("copy feedback lifetime");
    assert!(!app.screen().shows("Copied!"));
}
