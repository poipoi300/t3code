//! UI-DIALOG: modal interactions and adaptive layout, using the real shared primitives.

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::i18n::t;
use t3_ui::theme::{self, ThemeChoice};
use t3_ui::ui::button::{ButtonContent, ButtonSize, ButtonVariant};
use t3_ui::ui::dialog::dialog;
use t3_ui::ui::dialog::*;

use crate::support::{DESKTOP, PHONE};

// App has no dialog entry point yet; a dedicated root exercises this primitive without
// changing the frozen router or making a test depend on another screen package.
fn start<C: Component<Props = ()>>(window: (f32, f32)) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        clock: nsh_core::time::Clock::manual(std::time::SystemTime::UNIX_EPOCH),
        environment: nsh_core::platform::Environment {
            // The web driver supports both keyboard view focus and a phone pointer idiom.
            os: nsh_core::platform::Os::Web,
            idiom: nsh_core::platform::Idiom::Desktop,
            ..nsh_core::platform::Environment::default()
        },
        ..EngineConfig::new(Root::of::<C>())
    })
    .expect("start dialog fixture");
    app.first_render().expect("render fixture");
    app
}

#[component]
fn Flow() -> impl IntoElement {
    theme::install(&ThemeChoice::default());
    let open = signal(false);
    let enabled = signal(false);
    let accepted = signal(0usize);
    on_key("ctrl+r", move |_| open.set(true));
    dialog(open, move |state| {
        view()
            .w_full()
            .h_full()
            .child(dialog_trigger(state, "Open retry"))
            .child(text(move || {
                if open.get() {
                    t("dialog open")
                } else {
                    t("dialog closed")
                }
            }))
            .child(dialog_popup(state, PopupOptions::default(), move || {
                view()
                    .child(dialog_header(
                        state,
                        dialog_title(state, || "Import failed".into()),
                    ))
                    .child(dialog_panel(
                        state,
                        view()
                            .child(dialog_description(state, || {
                                "Try importing the project again.".into()
                            }))
                            .child(text(move || format!("accepted {}", accepted.get())))
                            .child(
                                nsh_base::button()
                                    .accessibility_label(Some(SharedString::from(t(
                                        "Enable retry",
                                    ))))
                                    .on_press(move || enabled.set(true))
                                    .child(text(t("Enable retry"))),
                            ),
                    ))
                    .child(dialog_footer(
                        state,
                        FooterVariant::Default,
                        view()
                            .child(dialog_close(
                                state,
                                ButtonContent::Label(t("Cancel")),
                                ButtonVariant::Outline,
                                ButtonSize::Default,
                                || false,
                            ))
                            .child(dialog_action(
                                state,
                                ButtonContent::Label(t("Try again")),
                                ActionVariant::Default,
                                ButtonSize::Default,
                                move || !enabled.get(),
                                move || accepted.update(|n| *n += 1),
                            )),
                    ))
            }))
    })
}

#[component]
fn AlertFlow() -> impl IntoElement {
    theme::install(&ThemeChoice::default());
    let open = signal(false);
    alert_dialog(open, move |state| {
        view()
            .w_full()
            .h_full()
            .child(dialog_trigger(state, "Open confirmation"))
            .child(text(move || {
                if open.get() {
                    t("alert open")
                } else {
                    t("alert closed")
                }
            }))
            .child(alert_dialog_popup(
                state,
                AlertPopupOptions::default(),
                move || {
                    view()
                        .child(alert_dialog_header(
                            state,
                            view()
                                .child(alert_dialog_title(state, || "Delete project?".into()))
                                .child(alert_dialog_description(state, || {
                                    "This cannot be undone.".into()
                                })),
                        ))
                        .child(alert_dialog_footer(
                            state,
                            FooterVariant::Bare,
                            alert_dialog_close(
                                state,
                                ButtonContent::Label(t("Keep project")),
                                ButtonVariant::Outline,
                                ButtonSize::Default,
                                || false,
                            ),
                        ))
                },
            ))
    })
}

#[component]
fn FocusOverride() -> impl IntoElement {
    theme::install(&ThemeChoice::default());
    let open = signal(false);
    let target = element_ref();
    let popup_ref = element_ref();
    let value = signal(String::new());
    let ref_info = signal(String::new());
    dialog(open, move |state| {
        view()
            .w_full()
            .h_full()
            .child(dialog_trigger(state, "Open editor"))
            .child(dialog_popup(
                state,
                PopupOptions {
                    initial_focus: Some(target),
                    popup_ref: Some(popup_ref),
                    ..PopupOptions::default()
                },
                move || {
                    view()
                        .child(dialog_title(state, || "Edit name".into()))
                        .child(nsh_base::input().label(t("Other name")))
                        .child(
                            nsh_base::input()
                                .element_ref(&target)
                                .label(t("Name"))
                                .value(move || value.get())
                                .on_change(move |text: &str| value.set(text.into())),
                        )
                        .child(
                            nsh_base::button()
                                .accessibility_label(Some(SharedString::from(t(
                                    "Inspect popup ref",
                                ))))
                                .on_press(move || {
                                    ref_info.set(
                                        popup_ref
                                            .node()
                                            .map(|node| node.to_string())
                                            .unwrap_or_default(),
                                    )
                                })
                                .child(text(t("Inspect popup ref"))),
                        )
                        .child(text(move || format!("popup ref {}", ref_info.get())))
                },
            ))
    })
}

#[component]
fn ReplacedAlert() -> impl IntoElement {
    theme::install(&ThemeChoice::default());
    let first = signal(false);
    let next = signal(false);
    let dismissed = signal(0);
    let replacement_was_current = signal(false);
    view()
        .w_full()
        .h_full()
        .child(text(move || format!("dismissed {}", dismissed.get())))
        .child(text(move || {
            if replacement_was_current.get() {
                t("replacement was current")
            } else {
                t("no replacement observed")
            }
        }))
        .child(alert_dialog(first, move |state| {
            state.on_replaced(move || {
                replacement_was_current.set(next.get_untracked());
                dismissed.update(|count| *count += 1);
            });
            view()
                .child(dialog_trigger(state, "Open older alert"))
                .child(alert_dialog_popup(
                    state,
                    AlertPopupOptions::default(),
                    move || {
                        view()
                            .child(alert_dialog_title(state, || "Older alert".into()))
                            .child(alert_dialog_close(
                                state,
                                ButtonContent::Label(t("Answer older")),
                                ButtonVariant::Outline,
                                ButtonSize::Default,
                                || false,
                            ))
                            .child(
                                nsh_base::button()
                                    .accessibility_label(Some(SharedString::from(t(
                                        "Replace alert",
                                    ))))
                                    .on_press(move || {
                                        state.replace_with(|| next.set(true));
                                        state.replace_with(|| next.set(true));
                                    })
                                    .child(text(t("Replace alert"))),
                            )
                    },
                ))
        }))
        .child(alert_dialog(next, move |state| {
            alert_dialog_popup(state, AlertPopupOptions::default(), move || {
                view()
                    .child(alert_dialog_title(state, || "Newer alert".into()))
                    .child(alert_dialog_close(
                        state,
                        ButtonContent::Label(t("Answer newer")),
                        ButtonVariant::Outline,
                        ButtonSize::Default,
                        || false,
                    ))
            })
        }))
}

#[component]
fn Nested() -> impl IntoElement {
    theme::install(&ThemeChoice::default());
    let open = signal(false);
    dialog(open, move |parent| {
        view()
            .w_full()
            .h_full()
            .child(dialog_trigger(parent, "Open parent"))
            .child(text(move || {
                if open.get() {
                    t("parent open")
                } else {
                    t("parent closed")
                }
            }))
            .child(dialog_popup(parent, PopupOptions::default(), move || {
                let child_open = signal(false);
                view()
                    .child(dialog_header(
                        parent,
                        dialog_title(parent, || "Parent dialog".into()),
                    ))
                    .child(dialog(child_open, move |child| {
                        view()
                            .child(dialog_trigger(child, "Open nested"))
                            .child(dialog_popup(child, PopupOptions::default(), move || {
                                view()
                                    .child(dialog_title(child, || "Nested dialog".into()))
                                    .child(dialog_close(
                                        child,
                                        ButtonContent::Label(t("Close nested")),
                                        ButtonVariant::Outline,
                                        ButtonSize::Default,
                                        || false,
                                    ))
                            }))
                    }))
            }))
    })
}

#[component]
fn ActionFocus() -> impl IntoElement {
    action_focus_flow(false)
}

#[component]
fn CancelFocus() -> impl IntoElement {
    action_focus_flow(true)
}

fn action_focus_flow(cancel_initial_focus: bool) -> impl IntoElement {
    theme::install(&ThemeChoice::default());
    let open = signal(false);
    let disabled = signal(false);
    let accepted = signal(0);
    let replaced = signal(false);
    let cancel = element_ref();
    on_key("ctrl+d", move |_| disabled.set(true));
    dialog(open, move |state| {
        view()
            .w_full()
            .h_full()
            .child(dialog_trigger(state, "Open action"))
            .child(text(move || {
                if open.get() {
                    t("action open")
                } else {
                    t("action closed")
                }
            }))
            .child(dialog_popup(
                state,
                PopupOptions {
                    initial_focus: cancel_initial_focus.then_some(cancel),
                    ..PopupOptions::default()
                },
                move || {
                    vec![
                        dynamic(move || {
                            let replaced = replaced.get();
                            untracked(|| {
                                if replaced {
                                    dialog_title(state, || "Import finished".into()).into_element()
                                } else {
                                    dialog_title(state, || "Retry import?".into()).into_element()
                                }
                            })
                        })
                        .into_element(),
                        dynamic(move || {
                            let replaced = replaced.get();
                            untracked(|| {
                                if replaced {
                                    dialog_description(state, || "The project is ready.".into())
                                        .into_element()
                                } else {
                                    dialog_description(state, || {
                                        "The project could not be imported.".into()
                                    })
                                    .into_element()
                                }
                            })
                        })
                        .into_element(),
                        dialog_close_with_ref(
                            state,
                            ButtonContent::Label(t("Cancel")),
                            ButtonVariant::Outline,
                            ButtonSize::Default,
                            || false,
                            cancel,
                        )
                        .into_element(),
                        dialog_action(
                            state,
                            ButtonContent::Label(t("Retry")),
                            ActionVariant::Default,
                            ButtonSize::Default,
                            move || disabled.get(),
                            move || accepted.update(|n| *n += 1),
                        )
                        .into_element(),
                        text(move || format!("retries {}", accepted.get())).into_element(),
                        nsh_base::button()
                            .accessibility_label(Some(SharedString::from(t("Replace content"))))
                            .on_press(move || replaced.set(true))
                            .child(text(t("Replace content")))
                            .into_element(),
                    ]
                },
            ))
    })
}

fn layout_flow(options: PopupOptions) -> impl IntoElement {
    theme::install(&ThemeChoice::default());
    let open = signal(false);
    dialog(open, move |state| {
        view()
            .w_full()
            .h_full()
            .child(dialog_trigger(state, "Open layout"))
            .child(dialog_popup(state, options, move || {
                if options.variant == Variant::Media {
                    view()
                        .w(px(2000.0))
                        .h(px(2000.0))
                        .child(dialog_title(state, || "Layout example".into()))
                        .into_element()
                } else {
                    dialog_header(state, dialog_title(state, || "Layout example".into()))
                        .into_element()
                }
            }))
    })
}

#[component]
fn MediaFlow() -> impl IntoElement {
    layout_flow(PopupOptions {
        variant: Variant::Media,
        bottom_stick_on_mobile: false,
        show_close_button: false,
        ..PopupOptions::default()
    })
}

#[component]
fn CenteredFlow() -> impl IntoElement {
    layout_flow(PopupOptions {
        bottom_stick_on_mobile: false,
        show_close_button: false,
        ..PopupOptions::default()
    })
}

#[component]
fn ControlledFlow() -> impl IntoElement {
    theme::install(&ThemeChoice::default());
    let open = signal(false);
    let allow_close = signal(false);
    dialog(open, move |state| {
        state.on_open_change(move |requested| {
            if requested || allow_close.get() {
                open.set(requested);
            }
        });
        view()
            .w_full()
            .h_full()
            .child(dialog_trigger(state, "Open controlled"))
            .child(dialog_popup(state, PopupOptions::default(), move || {
                vec![
                    dialog_title(state, || "Unsaved changes".into()).into_element(),
                    nsh_base::button()
                        .accessibility_label(Some(SharedString::from(t("Allow close"))))
                        .on_press(move || allow_close.set(true))
                        .child(text(t("Allow close")))
                        .into_element(),
                ]
            }))
    })
}

#[component]
fn RetainedText() -> impl IntoElement {
    theme::install(&ThemeChoice::default());
    let open = signal(false);
    let title = signal(String::from("Original title"));
    let description = signal(String::from("Original explanation"));
    dialog(open, move |state| {
        state.on_open_change(move |requested| {
            batch(|| {
                open.set(requested);
                if !requested {
                    title.set(String::new());
                    description.set(String::new());
                }
            })
        });
        view()
            .w_full()
            .h_full()
            .child(dialog_trigger(state, "Open retained text"))
            .child(text(move || {
                if open.get() {
                    t("text open")
                } else {
                    t("text closed")
                }
            }))
            .child(dialog_popup(state, PopupOptions::default(), move || {
                vec![
                    dialog_title(state, move || title.get()).into_element(),
                    dialog_description(state, move || description.get()).into_element(),
                ]
            }))
    })
}

fn bounds(app: &Headless, id: &str) -> nsh_core::Bounds<nsh_core::Points> {
    let node = app.screen().id(id).expect("element exists");
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|item| item.id == node)
        .expect("element laid out")
        .bounds
}

fn gone(app: &mut Headless, selector: &str) {
    app.wait_for_gone(selector, std::time::Duration::from_secs(5))
        .expect("modal is removed after its transition");
}

#[test]
fn disabled_actions_are_inert_and_enabled_actions_do_not_implicitly_close() {
    let mut app = start::<Flow>(DESKTOP);
    app.press("role:button[Open retry]").expect("open");
    app.wait_for("accepted 0").expect("content");
    assert!(
        !app.screen()
            .enabled("role:button[Try again]")
            .expect("disabled")
    );
    let _ = app.press("role:button[Try again]");
    assert!(app.screen().shows("accepted 0"));
    app.press("role:button[Enable retry]").expect("enable");
    app.press("role:button[Try again]").expect("retry");
    app.wait_for("accepted 1").expect("action runs once");
    assert_eq!(app.screen().count("role:dialog[Import failed]"), Ok(1));
    app.press("role:button[Cancel]").expect("cancel");
    gone(&mut app, "role:dialog");
    app.wait_for("dialog closed").expect("exit completes");
    assert_eq!(app.screen().count("role:dialog"), Ok(0));
}

#[test]
fn a_controlled_caller_can_reject_dismissal_without_losing_the_form() {
    let mut app = start::<ControlledFlow>(DESKTOP);
    app.press("role:button[Open controlled]").expect("open");
    app.press_key("escape").expect("request close");
    assert_eq!(app.screen().count("role:dialog[Unsaved changes]"), Ok(1));
    app.press("role:button[Close]")
        .expect("request close with button");
    assert_eq!(app.screen().count("role:dialog[Unsaved changes]"), Ok(1));
    app.press("role:button[Allow close]")
        .expect("allow dismissal");
    app.press_key("escape").expect("close after acceptance");
    gone(&mut app, "role:dialog");
}

#[test]
fn escape_and_backdrop_dismiss_a_dialog_and_it_can_reopen() {
    for window in [PHONE, DESKTOP] {
        let mut app = start::<Flow>(window);
        app.press("role:button[Open retry]").expect("open");
        app.wait_for("Import failed").expect("popup");
        assert_eq!(
            app.screen()
                .accessibility_description("role:dialog[Import failed]"),
            Ok(Some("Try importing the project again.".into()))
        );
        app.press_key("escape").expect("escape");
        gone(&mut app, "role:dialog");
        app.wait_for("dialog closed")
            .expect("removed after transition");
        app.press("role:button[Open retry]").expect("reopen");
        app.wait_for("Import failed").expect("new popup");
        app.press_at("id:dialog-backdrop", point(px(2.0), px(2.0)))
            .expect("outside press");
        gone(&mut app, "role:dialog");
        app.wait_for("dialog closed").expect("backdrop closes");
    }
}

#[test]
fn alerts_reject_backdrop_dismissal_but_allow_escape_and_explicit_cancel() {
    let mut app = start::<AlertFlow>(DESKTOP);
    app.press("role:button[Open confirmation]").expect("open");
    app.wait_for("Delete project?").expect("popup");
    assert_eq!(
        app.screen().count("role:alertdialog[Delete project?]"),
        Ok(1)
    );
    assert_eq!(app.screen().count("role:button[Close]"), Ok(0));
    app.press_at("id:alert-dialog-backdrop", point(px(2.0), px(2.0)))
        .expect("outside press");
    assert_eq!(app.screen().count("role:alertdialog"), Ok(1));
    app.press_key("escape").expect("escape");
    gone(&mut app, "role:alertdialog");
    app.wait_for("alert closed").expect("escape closes");
    app.press("role:button[Open confirmation]").expect("reopen");
    app.press("role:button[Keep project]")
        .expect("explicit cancel");
    gone(&mut app, "role:alertdialog");
    app.wait_for("alert closed").expect("cancel closes");
}

#[test]
fn popup_changes_between_centered_dialog_and_bottom_sheet_on_resize() {
    let mut app = start::<Flow>(DESKTOP);
    app.press("role:button[Open retry]").expect("open");
    app.wait_for("Import failed").expect("popup");
    let desktop = bounds(&app, "id:dialog-popup");
    assert!(
        (desktop.size.width.0 - 512.0).abs() < 1.0,
        "desktop width: {desktop:?}"
    );
    assert!(
        (desktop.origin.y.0 * 2.0 + desktop.size.height.0 - DESKTOP.1).abs() < 1.0,
        "desktop centered: {desktop:?}"
    );
    app.set_viewport(size(px(PHONE.0), px(PHONE.1)))
        .expect("resize");
    let phone = bounds(&app, "id:dialog-popup");
    assert!(
        (phone.size.width.0 - PHONE.0).abs() < 1.0,
        "sheet width: {phone:?}"
    );
    assert!(
        (phone.origin.y.0 + phone.size.height.0 - PHONE.1).abs() < 1.0,
        "sheet reaches bottom: {phone:?}"
    );
}

#[test]
fn media_bounds_follow_the_viewport_on_resize() {
    let mut app = start::<MediaFlow>(DESKTOP);
    app.press("role:button[Open layout]").expect("open media");
    for window in [DESKTOP, PHONE] {
        app.set_viewport(size(px(window.0), px(window.1)))
            .expect("resize media");
        let popup = bounds(&app, "id:dialog-popup");
        let max_width = (window.0 * 0.92).min(window.0 - 32.0);
        assert!(
            (popup.size.width.0 - max_width).abs() < 1.0,
            "media width: {popup:?}"
        );
        assert!(
            (popup.size.height.0 - window.1 * 0.92).abs() < 1.0,
            "media height: {popup:?}"
        );
    }
    app.press_key("escape")
        .expect("dismiss without a built-in close button");
    gone(&mut app, "role:dialog");
}

#[test]
fn opting_out_of_the_mobile_sheet_keeps_a_phone_dialog_centered() {
    let mut app = start::<CenteredFlow>(PHONE);
    app.press("role:button[Open layout]")
        .expect("open centered dialog");
    let popup = bounds(&app, "id:dialog-popup");
    assert!(
        (popup.origin.y.0 * 2.0 + popup.size.height.0 - PHONE.1).abs() < 1.0,
        "phone centered: {popup:?}"
    );
    app.press_key("escape").expect("dismiss");
    gone(&mut app, "role:dialog");
}

#[test]
fn caller_initial_focus_receives_typing() {
    for coarse in [false, true] {
        let mut app = start::<FocusOverride>(DESKTOP);
        if coarse {
            app.set_environment("coarse pointer", |env| {
                env.idiom = nsh_core::platform::Idiom::Phone
            })
            .expect("set pointer");
        }
        app.press("role:button[Open editor]").expect("open");
        app.wait_for("Edit name").expect("popup");
        app.press_key("a").expect("type into focused input");
        assert_eq!(app.screen().input_text("label:Name"), Ok(Some("a")));
        assert_eq!(app.screen().input_text("label:Other name"), Ok(Some("")));
        let popup = app.screen().id("id:dialog-popup").expect("popup node");
        app.press("role:button[Inspect popup ref]")
            .expect("inspect caller ref");
        app.wait_for(&format!("popup ref {popup}"))
            .expect("caller ref names the popup");
    }
}

#[test]
fn initial_focus_prefers_an_enabled_action_and_skips_a_disabled_one() {
    let mut app = start::<ActionFocus>(DESKTOP);
    app.press("role:button[Open action]").expect("open");
    app.press_key("enter").expect("activate focused action");
    app.wait_for("retries 1").expect("action received focus");
    app.press("role:button[Cancel]").expect("cancel");
    gone(&mut app, "role:dialog");
    app.wait_for("action closed").expect("closed");
    app.press_key("ctrl+d").expect("disable action");
    app.press("role:button[Open action]").expect("reopen");
    app.press_key("enter").expect("activate fallback cancel");
    gone(&mut app, "role:dialog");
    app.wait_for("action closed")
        .expect("disabled action was skipped");
    let mut app = start::<CancelFocus>(DESKTOP);
    app.press("role:button[Open action]")
        .expect("open with caller Cancel ref");
    app.press_key("enter")
        .expect("activate caller focus override");
    gone(&mut app, "role:dialog");
    app.wait_for("action closed")
        .expect("caller Cancel overrides enabled action");
}

#[test]
fn replacing_an_unanswered_alert_notifies_its_owner_once_after_presenting_the_next() {
    let mut app = start::<ReplacedAlert>(DESKTOP);
    app.press("role:button[Open older alert]")
        .expect("open older");
    app.press("role:button[Answer older]")
        .expect("answer older");
    gone(&mut app, "role:alertdialog");
    app.wait_for("dismissed 0")
        .expect("an answer is not replacement");
    app.press("role:button[Open older alert]")
        .expect("open again");
    app.press("role:button[Replace alert]").expect("replace");
    app.wait_for("Newer alert").expect("new alert is presented");
    app.wait_for("dismissed 1")
        .expect("replaced alert notified once");
    app.wait_for("replacement was current")
        .expect("callback follows replacement");
    app.press("role:button[Answer newer]")
        .expect("close replacement");
    gone(&mut app, "role:alertdialog");
    app.wait_for("dismissed 1").expect("no second notification");
}

#[test]
fn coarse_pointer_focuses_the_popup_instead_of_activating_an_action() {
    let mut app = start::<ActionFocus>(PHONE);
    app.set_environment("coarse pointer", |env| {
        env.idiom = nsh_core::platform::Idiom::Phone
    })
    .expect("set pointer");
    app.press("role:button[Open action]").expect("open");
    app.press_key("enter").expect("popup receives Enter");
    assert_eq!(app.screen().count("role:dialog[Retry import?]"), Ok(1));
    assert!(app.screen().shows("retries 0"));
}

#[test]
fn escape_restores_keyboard_focus_to_the_trigger() {
    let mut app = start::<ActionFocus>(DESKTOP);
    app.press_key("tab").expect("focus trigger");
    app.press_key("enter").expect("open with keyboard");
    app.wait_for("Retry import?").expect("popup");
    app.press_key("escape").expect("close");
    gone(&mut app, "role:dialog");
    app.wait_for("action closed").expect("exit");
    app.press_key("enter").expect("activate restored trigger");
    app.wait_for("Retry import?")
        .expect("reopened from restored focus");
}

#[test]
fn replacing_text_slots_updates_the_modal_accessible_name_and_description() {
    let mut app = start::<ActionFocus>(DESKTOP);
    app.press("role:button[Open action]").expect("open");
    app.wait_for("Retry import?").expect("initial title");
    app.press("role:button[Replace content]")
        .expect("replace text slots");
    app.wait_for("Import finished").expect("new title");
    assert_eq!(app.screen().count("role:dialog[Import finished]"), Ok(1));
    assert_eq!(
        app.screen()
            .accessibility_description("role:dialog[Import finished]"),
        Ok(Some("The project is ready.".into()))
    );
}

#[test]
fn text_remains_visible_during_exit_after_the_caller_clears_its_data() {
    let mut app = start::<RetainedText>(DESKTOP);
    app.press("role:button[Open retained text]").expect("open");
    let close = app.screen().id("role:button[Close]").expect("close button");
    app.engine()
        .control()
        .dispatch_event(close, EventKind::Press, EventPayload::None);
    app.wait_for("text closed")
        .expect("caller cleared its data");
    assert!(app.screen().shows("Original title"));
    assert!(app.screen().shows("Original explanation"));
    assert_eq!(app.screen().count("role:dialog[Original title]"), Ok(1));
    gone(&mut app, "role:dialog");
}

#[test]
fn a_nested_dialog_closes_without_closing_its_parent() {
    let mut app = start::<Nested>(DESKTOP);
    app.press("role:button[Open parent]").expect("open parent");
    app.press("role:button[Open nested]").expect("open nested");
    app.wait_for("Nested dialog").expect("child");
    app.press("role:button[Close nested]").expect("close child");
    gone(&mut app, "role:dialog[Nested dialog]");
    app.wait_for("Open nested").expect("parent restored");
    app.press_key("escape").expect("close parent");
    gone(&mut app, "role:dialog");
    app.wait_for("parent closed").expect("both removed");
}

#[test]
fn reopening_during_exit_does_not_unmount_the_reopened_popup() {
    let mut app = start::<Flow>(DESKTOP);
    app.press("role:button[Open retry]").expect("open");
    let cancel = app
        .screen()
        .id("role:button[Cancel]")
        .expect("cancel button");
    // Headless::press settles animations; dispatch this press before advancing the manual clock.
    app.engine()
        .control()
        .dispatch_event(cancel, EventKind::Press, EventPayload::None);
    app.wait_for("dialog closed").expect("exit has begun");
    assert_eq!(
        app.screen().count("role:dialog"),
        Ok(1),
        "content retained for exit"
    );
    assert!(
        !app.screen()
            .enabled("role:button[Try again]")
            .expect("closing action")
    );
    app.press_key("ctrl+r").expect("reopen during exit");
    app.fast_forward(std::time::Duration::from_millis(250))
        .expect("finish interrupted transition");
    app.wait_for("Import failed").expect("popup remains");
    app.press("role:button[Enable retry]").expect("enable");
    app.press("role:button[Try again]").expect("retry");
    app.wait_for("accepted 1").expect("still interactive");
    app.press("role:button[Close]").expect("close");
    gone(&mut app, "role:dialog");
    app.wait_for("dialog closed").expect("eventual exit");
}
