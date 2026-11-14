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

fn timed_options(count: Signal<i64>, visible_only: bool) -> ToastOptions {
    ToastOptions {
        title: "Timed notification".into(),
        timeout: (!visible_only).then_some(Duration::from_secs(2)),
        action: Some(Action {
            label: "Hold notification".into(),
            disabled: true,
            on_press: Rc::new(move || count.set(100)),
        }),
        data: ThreadToastData {
            dismiss_after_visible: visible_only.then_some(Duration::from_secs(2)),
            expandable_content: Some(Rc::new(|| {
                nsh_base::input()
                    .test_id("toast-focus-input")
                    .label(SharedString::from(t("Notification details")))
                    .w(px(140.0))
                    .h(px(28.0))
                    .into_element()
            })),
            secondary_action: Some(Action {
                label: "Keep open".into(),
                disabled: false,
                on_press: Rc::new(move || count.update(|value| *value += 1)),
            }),
            ..Default::default()
        },
        ..Default::default()
    }
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
    let visible_timed = manager.clone();
    let scoped = manager.clone();
    let action = manager.clone();
    let page_focused = signal(false);
    let page_escapes = signal(0_i64);
    effect(move || {
        if page_focused.get() {
            on_key_with(
                "escape",
                KeyOptions {
                    when: nsh_core::keymap::When::Always,
                    ..Default::default()
                },
                move |_| page_escapes.update(|count| *count += 1),
            );
        }
    });
    view()
        .w_full()
        .h_full()
        .child(
            view()
                .app_class(t3_ui::theme::tw("p-8 pt-64 gap-4"))
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
                            timed.add(timed_options(count, false));
                        })
                        .child(text(t("Raise timed toast"))),
                )
                .child(
                    nsh_base::button()
                        .accessibility_label(Some(SharedString::from(t(
                            "Raise visible-time toast",
                        ))))
                        .on_press(move || {
                            visible_timed.add(timed_options(count, true));
                        })
                        .child(text(t("Raise visible-time toast"))),
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
                }))
                .child(
                    nsh_base::input()
                        .test_id("outside-focus")
                        .label(SharedString::from(t("Page input")))
                        .on_focus(move || page_focused.set(true))
                        .on_blur(move || page_focused.set(false))
                        .w(px(140.0))
                        .h(px(28.0)),
                )
                .child(text(move || {
                    t3_ui::i18n::tr(
                        "Page escapes: {count}",
                        &[("count", t3_ui::i18n::Value::Int(page_escapes.get()))],
                    )
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
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        clock: nsh_core::time::Clock::manual(SystemTime::UNIX_EPOCH),
        ..EngineConfig::new(Root::of::<ToastFixture>())
    };
    if window == DESKTOP {
        config.environment.os = nsh_core::platform::Os::Windows;
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    }
    Headless::start(config).expect("toast fixture")
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
    app.press("role:button[Copy error]")
        .expect("copy error text");
    assert!(
        app.screen()
            .sent(&nsh_core::Command::SetClipboard("Permission denied".into()))
    );
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
fn the_next_toast_is_interactive_while_the_newest_one_is_still_leaving() {
    let mut app = start(DESKTOP);
    clear(&mut app);
    app.press("role:button[Raise error]").expect("older toast");
    app.wait_for("Request failed").expect("older toast mounted");
    app.press("role:button[Raise action toast]")
        .expect("newer toast");
    app.wait_for("Thread archived")
        .expect("newer toast mounted");
    app.hover("Thread archived").expect("expand the stack");
    app.press("role:button[Copy error]")
        .expect("older toast expands with the stack");
    app.press("role:button[Undo]").expect("close newest toast");
    app.wait_for("Actions: 1").expect("action completed");
    app.hover("role:button[Return to thread]")
        .expect("collapse the stack during exit");
    assert_eq!(
        app.screen()
            .layout_nodes()
            .iter()
            .filter(|node| node.test_id.as_deref() == Some("toast-title"))
            .count(),
        2,
        "the departing toast is still mounted"
    );
    let copies = app
        .screen()
        .commands()
        .iter()
        .filter(|(_, command)| matches!(command, nsh_core::Command::SetClipboard(_)))
        .count();
    app.press("role:button[Copy error]")
        .expect("remaining toast is frontmost before exit finishes");
    assert_eq!(
        app.screen()
            .commands()
            .iter()
            .filter(|(_, command)| *command
                == nsh_core::Command::SetClipboard("Permission denied".into()))
            .count(),
        copies + 1
    );
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
    for trigger in ["Raise timed toast", "Raise visible-time toast"] {
        let mut app = start(DESKTOP);
        clear(&mut app);
        app.press(&format!("role:button[{trigger}]"))
            .expect("raise");
        app.wait_for("Timed notification").expect("timed toast");
        app.hover("id:toast-title").expect("hover");
        app.run_for(Duration::from_secs(10))
            .expect("manual clock while hovered");
        assert!(app.screen().shows("Timed notification"));
        assert!(app.screen().shows("Hold notification"));
        let _ = app.press("role:button[Hold notification]");
        assert!(app.screen().shows("Actions: 0"));
        app.hover("role:button[Return to thread]")
            .expect("leave toast");
        app.run_for(Duration::from_secs(3))
            .expect("remaining timeout");
        assert!(!app.screen().shows("Timed notification"));
    }
}

#[test]
fn focused_toast_controls_pause_dismissal_until_focus_leaves_the_stack() {
    let mut app = start(DESKTOP);
    clear(&mut app);
    app.press("role:button[Raise visible-time toast]")
        .expect("raise");
    app.wait_for("Timed notification").expect("timed toast");
    app.press("role:button[Keep open]")
        .expect("secondary action");
    app.wait_for("Actions: 1")
        .expect("secondary action does not close");
    app.press("role:button[Show details]")
        .expect("show focusable details");
    app.type_text("id:toast-focus-input", "hold")
        .expect("focus a control inside the toast");
    app.hover("role:button[Return to thread]")
        .expect("pointer outside toast");
    app.run_for(Duration::from_secs(10))
        .expect("manual clock while focused");
    assert!(app.screen().shows("Timed notification"));
    app.type_text("id:outside-focus", "leave")
        .expect("focus leaves the toast");
    app.run_for(Duration::from_secs(3))
        .expect("remaining timeout");
    assert!(!app.screen().shows("Timed notification"));
}

#[test]
fn escape_reaches_the_page_input_and_only_closes_a_toast_with_focus_inside() {
    let mut app = start(DESKTOP);
    clear(&mut app);
    app.press("role:button[Raise visible-time toast]")
        .expect("raise toast");
    app.wait_for("Timed notification").expect("toast mounted");
    app.type_text("id:outside-focus", "page")
        .expect("focus page input");
    app.press_key("Escape").expect("input owns Escape");
    app.wait_for("Page escapes: 1")
        .expect("focused input received Escape");
    assert!(app.screen().shows("Timed notification"));
    app.press("role:button[Show details]")
        .expect("expand focusable details");
    app.type_text("id:toast-focus-input", "toast")
        .expect("focus inside toast");
    app.press_key("Escape").expect("toast owns Escape");
    app.wait_for_gone("Timed notification", Duration::from_secs(1))
        .expect("focused toast closed");
    assert!(app.screen().shows("Page escapes: 1"));
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
    app.press("role:button[Return to thread]")
        .expect("outside feedback");
    app.run_for(Duration::from_secs(2))
        .expect("copy feedback lifetime");
    assert!(!app.screen().shows("Copied!"));
}

/// Rule 4 of fidelity review 2: a slot holding content is shown as given, even when the content
/// equals a catalogue key ("Settings" is "Paramètres" in French); a key slot is translated.
#[component]
fn ContentToastFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    t3_ui::i18n::set_language("fr", &[]);
    let manager = ToastManager::default();
    manager.add(ToastOptions {
        title: t3_ui::i18n::Text::content("Settings"),
        description: Some(t3_ui::i18n::Text::content("Settings")),
        timeout: None,
        action: Some(Action {
            label: t3_ui::i18n::Text::content("Settings"),
            disabled: false,
            on_press: Rc::new(|| {}),
        }),
        ..Default::default()
    });
    manager.add(ToastOptions {
        title: "Settings".into(),
        timeout: None,
        ..Default::default()
    });
    view()
        .w_full()
        .h_full()
        .child(toast::toasts(manager, Position::TopRight, || None))
}

#[test]
fn content_slots_are_shown_verbatim_and_key_slots_translated() {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(PHONE.0), px(PHONE.1)),
        ..EngineConfig::new(Root::of::<ContentToastFixture>())
    })
    .expect("content toast fixture");
    app.wait_for("Paramètres").expect("the key title is translated");
    app.wait_for("role:button[Settings]")
        .expect("the content action label is shown as given");
    let screen = app.screen();
    let texts = screen.texts();
    let count = |wanted: &str| texts.iter().filter(|text| **text == wanted).count();
    // Content title, content description and content action label; only the key translates.
    assert_eq!(count("Settings"), 3, "{texts:?}");
    assert_eq!(count("Paramètres"), 1, "{texts:?}");
}
