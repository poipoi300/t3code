//! Interaction tests for the real action group and client outbox rows.

use std::rc::Rc;
use std::time::{Duration, SystemTime};

use nsh_core::prelude::*;
use nsh_core::time::Clock;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::ClientStore;
use t3_client::composition::ClientRuntime;
use t3_client::operations::ComposerDispatchMode;
use t3_client::testing::FakePlatform;
use t3_client::thread_views::composer::send::{PendingMessage, SendState};
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
use t3_ui::screens::composer::send::{
    PendingActionState, PrimaryActionState, PrimaryActionsProps, collapsed_composer_stop_action,
    composer_primary_actions, pending_message,
};
use t3_ui::{i18n::t, theme};

use crate::support::{DESKTOP, PHONE};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PropEnum)]
enum Mode {
    #[default]
    Resume,
    Draft,
    Question,
    Collapsed,
    Refused,
}

#[derive(Clone, PartialEq, Props)]
struct HarnessProps {
    mode: Mode,
}

#[component]
fn Harness(props: HarnessProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let mode = props.mode;
    let state = signal(PrimaryActionState {
        compact: true,
        can_operate_thread: true,
        is_running: true,
        can_interrupt: true,
        can_steer: true,
        run_id: Some("first".into()),
        pending_action: (mode == Mode::Question).then_some(PendingActionState {
            question_index: 0,
            is_last_question: true,
            can_advance: true,
            is_responding: false,
            is_complete: true,
        }),
        ..Default::default()
    });
    let interrupts = signal(0u32);
    let submits = signal(0u32);
    let resumes = signal(0u32);
    let submitted_mode = signal(None::<ComposerDispatchMode>);
    let mut actions = PrimaryActionsProps::new(Prop::bind(move || state.get()), move |mode| {
        submits.update(|count| *count += 1);
        submitted_mode.set(Some(mode));
    });
    actions.on_interrupt = Some(Rc::new(move || interrupts.update(|count| *count += 1)));
    actions.on_resume = Some(Rc::new(move || resumes.update(|count| *count += 1)));
    let controls = if mode == Mode::Collapsed {
        collapsed_composer_stop_action(actions, || {
            nsh_base::button()
                .accessibility_label(Some(t("Open composer").into()))
                .child(text(t("Open composer")))
                .into_element()
        })
        .into_element()
    } else {
        composer_primary_actions(actions).into_element()
    };
    let mut result = view()
        .flex_col()
        .gap(px(8.0))
        .p(px(16.0))
        .child(controls)
        .child(text(move || {
            SharedString::from(format!("Interrupts: {}", interrupts.get()))
        }))
        .child(text(move || {
            SharedString::from(format!("Submits: {}", submits.get()))
        }))
        .child(text(move || {
            SharedString::from(format!("Resumes: {}", resumes.get()))
        }))
        .child(text(move || {
            SharedString::from(format!("Mode: {:?}", submitted_mode.get()))
        }));
    for (label, operation) in [
        ("Stop acknowledged", 0),
        ("Lose permission", 1),
        ("Restore permission", 2),
        ("Clear run id", 3),
        ("Different run", 4),
        ("Type draft", 5),
        ("Alternate", 6),
        ("No steering", 7),
        ("Disconnect", 8),
        ("Busy", 9),
    ] {
        result = result.child(
            nsh_base::button()
                .accessibility_label(Some(t(label).into()))
                .on_press(move || {
                    state.update(|state| match operation {
                        0 => {
                            state.is_running = false;
                            state.can_interrupt = false;
                            state.can_resume = true;
                            state.has_sendable_content = mode == Mode::Draft;
                        }
                        1 => {
                            state.can_operate_thread = false;
                            state.can_interrupt = false;
                        }
                        2 => {
                            state.can_operate_thread = true;
                            state.can_interrupt = true;
                        }
                        3 => state.run_id = None,
                        4 => state.run_id = Some("second".into()),
                        5 => state.has_sendable_content = true,
                        6 => state.alternate_modifier = !state.alternate_modifier,
                        7 => state.can_steer = false,
                        8 => state.is_environment_unavailable = true,
                        9 => state.is_send_busy = true,
                        _ => unreachable!(),
                    })
                })
                .child(text(t(label))),
        );
    }
    if mode == Mode::Refused {
        let message = serde_json::from_value(serde_json::json!({
            "environmentId":"env", "threadId":"thread", "messageId":"message", "commandId":"command",
            "text":"My failed message", "attachments":[], "createdAt":"2026-11-14T01:00:00.000Z"
        })).expect("canonical queued message");
        let client = ClientRuntime::new(
            FakePlatform::default(),
            ClientStore::default(),
            OrchestrationV2CreationSource::Web,
            Rc::new(|_| {}),
        );
        result = result.child(pending_message(Rc::new(client), ThreadKey {
            environment: EnvironmentId("env".into()), thread: ThreadId("thread".into()),
        }, PendingMessage { message: Rc::new(message), state: SendState::Restored {
            draft_key: "draft".into(),
            reason: "Failed to dispatch orchestration V2 command: service ProviderAdapterRegistry.get: No orchestration provider adapter is registered for codex.".into(),
        } }));
    }
    result
}

fn start(window: (f32, f32), mode: Mode) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        clock: Clock::manual(SystemTime::UNIX_EPOCH),
        ..EngineConfig::new(Root::new::<Harness>(move || HarnessProps { mode }))
    };
    config.environment.idiom = if window == PHONE {
        nsh_core::platform::Idiom::Phone
    } else {
        nsh_core::platform::Idiom::Desktop
    };
    let mut app = Headless::start(config).expect("component starts");
    app.wait_for("Interrupts: 0").expect("component mounted");
    app
}

fn assert_disabled(app: &mut Headless, selector: &str) {
    app.wait_for(selector).expect("disabled control is visible");
    assert!(!app.screen().enabled(selector).expect("control exists"));
    assert!(
        app.press(selector).is_err(),
        "headless refuses disabled pointer targets"
    );
}

#[test]
fn repeat_stop_cannot_submit_the_draft_resume_or_answer_that_takes_its_place() {
    for window in [DESKTOP, PHONE] {
        for mode in [Mode::Resume, Mode::Draft, Mode::Question] {
            let mut app = start(window, mode);
            app.press("role:button[Stop generation]").expect("stop");
            app.wait_for("role:button[Stopping]").expect("pending");
            assert_disabled(&mut app, "role:button[Stopping]");
            assert!(app.screen().shows("Interrupts: 1"));
            app.press("role:button[Stop acknowledged]")
                .expect("acknowledge");
            let target = match mode {
                Mode::Draft => "role:button[Submit message]",
                Mode::Question => "role:button[Submit]",
                _ => "role:button[Resume thread]",
            };
            app.wait_for(target).expect("replacement");
            app.press(target).expect("guarded click");
            assert!(app.screen().shows("Submits: 0"));
            assert!(app.screen().shows("Resumes: 0"));
            app.fast_forward(Duration::from_millis(499))
                .expect("manual clock");
            app.press(target).expect("still guarded");
            assert!(app.screen().shows("Submits: 0"));
            assert!(app.screen().shows("Resumes: 0"));
            app.fast_forward(Duration::from_millis(1))
                .expect("guard expires");
            app.press(target).expect("replacement action");
            app.wait_for(if mode == Mode::Resume {
                "Resumes: 1"
            } else {
                "Submits: 1"
            })
            .expect("exactly one action");
        }
    }
}

#[test]
fn stopping_retries_after_ten_seconds_but_permission_and_a_missing_id_do_not_reset_it() {
    let mut app = start(DESKTOP, Mode::Resume);
    app.press("role:button[Stop generation]").expect("stop");
    app.press("role:button[Lose permission]")
        .expect("permission loss");
    app.press("role:button[Restore permission]")
        .expect("permission restored");
    app.press("role:button[Clear run id]").expect("id clears");
    assert_disabled(&mut app, "role:button[Stopping]");
    assert!(app.screen().shows("Interrupts: 1"));
    app.fast_forward(Duration::from_millis(9_999))
        .expect("manual clock");
    assert_disabled(&mut app, "role:button[Stopping]");
    assert!(app.screen().shows("Interrupts: 1"));
    app.fast_forward(Duration::from_millis(1)).expect("timeout");
    app.press("role:button[Stop generation]").expect("retry");
    app.wait_for("Interrupts: 2").expect("second stop");
    app.press("role:button[Different run]")
        .expect("another run");
    app.press("role:button[Stop generation]")
        .expect("new run can stop");
    app.wait_for("Interrupts: 3").expect("third stop");
}

#[test]
fn queue_and_steer_dispatch_the_clients_mode_and_busy_disconnected_actions_are_inert() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, Mode::Draft);
        app.press("role:button[Type draft]").expect("draft");
        app.press("role:button[Steer message]").expect("steer");
        app.wait_for("Mode: Some(Steer)").expect("client intent");
        app.press("role:button[Alternate]")
            .expect("alternate modifier");
        app.press("role:button[Queue message]").expect("queue");
        app.wait_for("Mode: Some(Queue)")
            .expect("client queue intent");
        app.press("role:button[Alternate]")
            .expect("default modifier");
        app.press("role:button[No steering]")
            .expect("unsupported steering");
        app.press("role:button[Queue message]")
            .expect("still queue");
        app.wait_for("Submits: 3").expect("three actions");
        app.press("role:button[Busy]").expect("in flight");
        assert_disabled(&mut app, "role:button[Submitting message]");
        assert!(app.screen().shows("Submits: 3"));
        app.press("role:button[Disconnect]").expect("disconnect");
        assert_disabled(&mut app, "role:button[Environment disconnected]");
        assert!(app.screen().shows("Submits: 3"));
    }
}

#[test]
fn collapsed_stop_restores_its_original_control_each_time_content_is_typed() {
    let mut app = start(PHONE, Mode::Collapsed);
    app.press("role:button[Stop generation]")
        .expect("collapsed stop");
    app.press("role:button[Stop acknowledged]")
        .expect("acknowledge");
    app.wait_for("role:button[Open composer]")
        .expect("original child restored");
    app.press("role:button[Restore permission]")
        .expect("interruptible again");
    app.wait_for("role:button[Stop generation]")
        .expect("stop returns");
    app.press("role:button[Type draft]").expect("content");
    app.wait_for("role:button[Open composer]")
        .expect("original child restored again");
}

#[test]
fn a_refused_message_shows_both_typed_error_parts_and_keeps_a_copy_action() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, Mode::Refused);
        app.wait_for("My failed message")
            .expect("failed text visible");
        app.wait_for("Failed to dispatch orchestration V2 command: service ProviderAdapterRegistry.get: No orchestration provider adapter is registered for codex.")
            .expect("U1 server message and detail");
        app.press("role:button[Copy message]")
            .expect("copy remains available");
        assert!(
            app.screen()
                .sent(&nsh_core::Command::SetClipboard("My failed message".into()))
        );
        assert!(
            app.screen()
                .matches("role:button[Dismiss]")
                .expect("dismiss action")
        );
        assert!(
            !app.screen()
                .matches("role:button[Retry]")
                .expect("no unsafe resend")
        );
    }
}
