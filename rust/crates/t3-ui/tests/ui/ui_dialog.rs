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
        environment: Environment {
            os: nsh_core::platform::Os::Windows,
            idiom: nsh_core::platform::Idiom::Desktop,
            ..Environment::default()
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
                },
            ))
    })
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
    theme::install(&ThemeChoice::default());
    let open = signal(false);
    let disabled = signal(false);
    let accepted = signal(0);
    let replaced = signal(false);
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
            .child(dialog_popup(state, PopupOptions::default(), move || {
                let action = element_ref();
                state.register_action(action, move || disabled.get());
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
                    dialog_close(
                        state,
                        ButtonContent::Label(t("Cancel")),
                        ButtonVariant::Outline,
                        ButtonSize::Default,
                        || false,
                    )
                    .into_element(),
                    // A caller's render override supplies the ref until the shared Button accepts one.
                    nsh_base::button()
                        .element_ref(&action)
                        .accessibility_label(Some(SharedString::from(t("Retry"))))
                        .disabled(move || disabled.get())
                        .on_press(move || accepted.update(|n| *n += 1))
                        .child(text(t("Retry")))
                        .into_element(),
                    text(move || format!("retries {}", accepted.get())).into_element(),
                    nsh_base::button()
                        .accessibility_label(Some(SharedString::from(t("Replace content"))))
                        .on_press(move || replaced.set(true))
                        .child(text(t("Replace content")))
                        .into_element(),
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
    app.wait_for("dialog closed").expect("exit completes");
    assert_eq!(app.screen().count("role:dialog"), Ok(0));
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
        app.wait_for("dialog closed")
            .expect("removed after transition");
        app.press("role:button[Open retry]").expect("reopen");
        app.wait_for("Import failed").expect("new popup");
        app.press_at("id:dialog-backdrop", point(px(2.0), px(2.0)))
            .expect("outside press");
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
    app.wait_for("alert closed").expect("escape closes");
    app.press("role:button[Open confirmation]").expect("reopen");
    app.press("role:button[Keep project]")
        .expect("explicit cancel");
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
    }
}

#[test]
fn initial_focus_prefers_an_enabled_action_and_skips_a_disabled_one() {
    let mut app = start::<ActionFocus>(DESKTOP);
    app.press("role:button[Open action]").expect("open");
    app.press_key("enter").expect("activate focused action");
    app.wait_for("retries 1").expect("action received focus");
    app.press("role:button[Cancel]").expect("cancel");
    app.wait_for("action closed").expect("closed");
    app.press_key("ctrl+d").expect("disable action");
    app.press("role:button[Open action]").expect("reopen");
    app.press_key("enter").expect("activate fallback cancel");
    app.wait_for("action closed")
        .expect("disabled action was skipped");
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
fn a_nested_dialog_closes_without_closing_its_parent() {
    let mut app = start::<Nested>(DESKTOP);
    app.press("role:button[Open parent]").expect("open parent");
    app.press("role:button[Open nested]").expect("open nested");
    app.wait_for("Nested dialog").expect("child");
    app.press("role:button[Close nested]").expect("close child");
    app.wait_for("Open nested").expect("parent restored");
    app.press_key("escape").expect("close parent");
    app.wait_for("parent closed").expect("both removed");
}

#[test]
fn reopening_during_exit_does_not_unmount_the_reopened_popup() {
    let mut app = start::<Flow>(DESKTOP);
    app.press("role:button[Open retry]").expect("open");
    app.press("role:button[Cancel]").expect("begin exit");
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
    app.wait_for("dialog closed").expect("eventual exit");
}
