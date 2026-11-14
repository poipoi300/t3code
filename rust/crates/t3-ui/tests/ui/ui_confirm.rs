//! UI-CONFIRM: real host input, focus, FIFO exit, and disposal behavior.

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::i18n::t;
use t3_ui::theme::{self, ThemeChoice};
use t3_ui::ui::confirm::{
    ConfirmDialogInitialFocus, ConfirmDialogOptions, ConfirmDialogVariant, confirm_dialog_host,
    request_confirm_dialog,
};

use crate::support::{DESKTOP, PHONE};

// App's persistent host is a frozen integration seam; this root uses the public host and request API.
fn start(window: (f32, f32)) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        clock: nsh_core::time::Clock::manual(std::time::SystemTime::UNIX_EPOCH),
        environment: nsh_core::platform::Environment {
            os: nsh_core::platform::Os::Web,
            idiom: nsh_core::platform::Idiom::Desktop,
            ..nsh_core::platform::Environment::default()
        },
        ..EngineConfig::new(Root::of::<Flow>())
    })
    .expect("start confirmation fixture");
    app.first_render().expect("render fixture");
    app
}

fn enqueue(message: &str, options: Option<ConfirmDialogOptions>, answers: Signal<Vec<bool>>) {
    if let Some(answer) = request_confirm_dialog(message, options) {
        spawn(async move {
            let answer = answer.await;
            answers.update(|answers| answers.push(answer));
        });
    } else {
        answers.update(|answers| answers.push(false));
    }
}

fn trigger(label: &str, on_press: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(Some(SharedString::from(t(label))))
        .on_press(on_press)
        .child(text(t(label)))
}

#[component]
fn Flow() -> impl IntoElement {
    theme::install(&ThemeChoice::default());
    let answers = signal(Vec::<bool>::new());
    let mounted = signal(true);
    on_key("ctrl+l", move |_| t3_ui::i18n::set_language("fr", &[]));
    on_key("ctrl+h", move |_| {
        mounted.update(|mounted| *mounted = !*mounted)
    });
    view()
        .w_full()
        .h_full()
        .child(trigger("Queue two", move || {
            enqueue("Delete the project?\nThis cannot be undone.", None, answers);
            enqueue("Delete the worktree too?", None, answers);
        }))
        .child(trigger("Default confirmation", move || {
            enqueue("Continue?", None, answers);
        }))
        .child(trigger("Literal message", move || {
            enqueue("Continue?\nSettings", None, answers);
        }))
        .child(trigger("Focus confirm", move || {
            enqueue(
                "Continue?",
                Some(ConfirmDialogOptions {
                    initial_focus: Some(ConfirmDialogInitialFocus::Confirm),
                    ..ConfirmDialogOptions::default()
                }),
                answers,
            );
        }))
        .child(trigger("Destructive confirmation", move || {
            enqueue(
                "Delete the thread?",
                Some(ConfirmDialogOptions {
                    variant: ConfirmDialogVariant::Destructive,
                    initial_focus: Some(ConfirmDialogInitialFocus::Confirm),
                }),
                answers,
            );
        }))
        .child(text(move || format!("answers {:?}", answers.get())))
        .child(when(mounted, || untracked(confirm_dialog_host)))
}

fn gone(app: &mut Headless) {
    app.wait_for_gone("role:alertdialog", std::time::Duration::from_secs(5))
        .expect("exit removes the final modal");
}

#[test]
fn the_real_host_advances_the_fifo_after_exit_on_phone_and_desktop() {
    for window in [PHONE, DESKTOP] {
        let mut app = start(window);
        app.press("role:button[Queue two]").expect("request two");
        app.wait_for("Delete the project?").expect("first request");
        assert_eq!(app.screen().count("role:alertdialog"), Ok(1));
        assert!(!app.screen().shows("Delete the worktree too?"));
        assert_eq!(
            app.screen()
                .accessibility_description("role:alertdialog[Delete the project?]"),
            Ok(Some("This cannot be undone.".into()))
        );
        app.press("role:button[Cancel]").expect("cancel first");
        app.wait_for("answers [false]")
            .expect("first future resolves");
        app.wait_for("Delete the worktree too?")
            .expect("second follows modal exit");
        assert_eq!(app.screen().count("role:alertdialog"), Ok(1));
        assert!(!app.screen().shows("Delete the project?"));
        app.press("role:button[Confirm]").expect("confirm second");
        app.wait_for("answers [false, true]")
            .expect("second future resolves");
        gone(&mut app);
    }
}

#[test]
fn default_and_destructive_requests_focus_cancel_and_only_default_can_opt_into_confirm() {
    let mut app = start(DESKTOP);
    for (trigger, title, expected) in [
        ("Default confirmation", "Continue?", "answers [false]"),
        (
            "Destructive confirmation",
            "Delete the thread?",
            "answers [false, false]",
        ),
        ("Focus confirm", "Continue?", "answers [false, false, true]"),
    ] {
        app.press(&format!("role:button[{trigger}]"))
            .expect("request");
        app.wait_for(title).expect("modal");
        app.press_key("enter")
            .expect("activate initially focused button");
        app.wait_for(expected)
            .expect("focus selects the intended answer");
        gone(&mut app);
    }
}

#[test]
fn backdrop_is_inert_and_escape_cancels() {
    let mut app = start(DESKTOP);
    app.press("role:button[Default confirmation]")
        .expect("request");
    app.wait_for("Continue?").expect("modal");
    app.press_at("id:alert-dialog-backdrop", point(px(2.0), px(2.0)))
        .expect("outside press");
    assert_eq!(app.screen().count("role:alertdialog[Continue?]"), Ok(1));
    assert!(app.screen().shows("answers []"));
    app.press_key("escape").expect("cancel");
    app.wait_for("answers [false]")
        .expect("future resolves cancelled");
    gone(&mut app);
}

#[test]
fn caller_content_is_literal_even_when_it_matches_a_translation_key() {
    let mut app = start(DESKTOP);
    app.press_key("ctrl+l").expect("switch to French");
    app.press("role:button[Literal message]").expect("request");
    app.wait_for("Continue?").expect("literal title");
    assert_eq!(
        app.screen()
            .accessibility_description("role:alertdialog[Continue?]"),
        Ok(Some("Settings".into()))
    );
    assert!(app.screen().shows("Settings"));
    assert!(!app.screen().shows("Paramètres"));
    app.press_key("escape").expect("cancel");
    app.wait_for("answers [false]").expect("cancelled");
    gone(&mut app);
}

#[test]
fn unmounting_the_real_host_cancels_its_active_and_queued_futures_and_can_remount() {
    let mut app = start(DESKTOP);
    app.press("role:button[Queue two]").expect("request two");
    app.wait_for("Delete the project?").expect("first request");
    app.press_key("ctrl+h").expect("unmount host");
    app.wait_for("answers [false, false]")
        .expect("both requests cancelled");
    gone(&mut app);
    app.press("role:button[Default confirmation]")
        .expect("request without host");
    app.wait_for("answers [false, false, false]")
        .expect("no host fails closed");
    assert_eq!(app.screen().count("role:alertdialog"), Ok(0));
    app.press_key("ctrl+h").expect("remount host");
    app.press("role:button[Default confirmation]")
        .expect("new request");
    app.wait_for("Continue?").expect("fresh modal");
    app.press("role:button[Confirm]").expect("confirm");
    app.wait_for("answers [false, false, false, true]")
        .expect("new future resolves");
    gone(&mut app);
}

#[test]
fn coarse_pointer_focuses_the_popup_and_explicit_controls_still_answer() {
    let mut app = start(PHONE);
    app.set_environment("coarse pointer", |environment| {
        environment.idiom = nsh_core::platform::Idiom::Phone;
    })
    .expect("phone pointer");
    app.press("role:button[Focus confirm]")
        .expect("request with focus opt-in");
    app.wait_for("Continue?").expect("modal");
    app.press_key("enter").expect("popup receives Enter");
    assert!(app.screen().shows("answers []"));
    assert_eq!(app.screen().count("role:alertdialog[Continue?]"), Ok(1));
    app.press("role:button[Confirm]").expect("explicit confirm");
    app.wait_for("answers [true]").expect("answered");
    gone(&mut app);
}
