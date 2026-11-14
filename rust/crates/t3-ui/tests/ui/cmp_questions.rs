//! CMP-QUESTIONS: mount the panel with a client projection, drive actual controls and timers.

use crate::support::{DESKTOP, PHONE};
use futures::stream::LocalBoxStream;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};
use t3_client::{
    ClientError,
    types::{EnvironmentId, ThreadId, ThreadKey},
};
use t3_contracts::base_schemas::RuntimeRequestId;
use t3_ui::screens::composer::questions::{
    ComposerQuestions, PendingUserInputClient, PendingUserInputView,
    composer_pending_user_input_panel,
};

#[derive(Default)]
struct Changes(RefCell<Vec<futures::channel::mpsc::UnboundedSender<()>>>);

impl Changes {
    fn subscribe(&self) -> LocalBoxStream<'static, ()> {
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        sender.unbounded_send(()).expect("initial projection");
        self.0.borrow_mut().push(sender);
        Box::pin(receiver)
    }
    fn notify(&self) {
        self.0
            .borrow_mut()
            .retain(|sender| sender.unbounded_send(()).is_ok());
    }
}

struct Client {
    view: RefCell<Option<Rc<PendingUserInputView>>>,
    changes: Changes,
    choices: Cell<usize>,
    advances: Cell<usize>,
    dismissals: Cell<usize>,
    operations: RefCell<Vec<String>>,
    reject: bool,
}

impl PendingUserInputClient for Client {
    fn changes(&self, _: &ThreadKey) -> LocalBoxStream<'static, ()> {
        self.changes.subscribe()
    }
    fn view(&self, _: &ThreadKey) -> Option<Rc<PendingUserInputView>> {
        self.view.borrow().clone()
    }
    fn toggle_option(
        &self,
        thread: &ThreadKey,
        request: &RuntimeRequestId,
        question: &str,
        value: &str,
    ) -> Result<(), ClientError> {
        if self.reject {
            return Err(ClientError::InvalidPayload("Request changed".into()));
        }
        self.choices.set(self.choices.get() + 1);
        self.operations.borrow_mut().push(format!(
            "choose {} {} {} {question} {value:?}",
            thread.environment.0, thread.thread.0, request.0
        ));
        self.changes.notify();
        Ok(())
    }
    fn advance(
        &self,
        thread: &ThreadKey,
        request: &RuntimeRequestId,
        question: &str,
    ) -> Result<(), ClientError> {
        self.advances.set(self.advances.get() + 1);
        self.operations.borrow_mut().push(format!(
            "advance {} {} {} {question}",
            thread.environment.0, thread.thread.0, request.0
        ));
        self.changes.notify();
        Ok(())
    }
    fn dismiss(&self, thread: &ThreadKey, request: &RuntimeRequestId) -> Result<(), ClientError> {
        self.dismissals.set(self.dismissals.get() + 1);
        self.operations.borrow_mut().push(format!(
            "dismiss {} {} {}",
            thread.environment.0, thread.thread.0, request.0
        ));
        *self.view.borrow_mut() = None;
        self.changes.notify();
        Ok(())
    }
}

#[derive(Clone, PartialEq, Props)]
struct QuestionHarnessProps {
    multi: bool,
    reject: bool,
}

#[component]
fn QuestionHarness(props: QuestionHarnessProps) -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let questions = serde_json::from_value::<Vec<t3_contracts::orchestration_v2::OrchestrationV2UserInputQuestion>>(json!([{
        "id":"scope", "header":"Migration", "question":"How should we move?", "multiSelect":props.multi,
        "options":[{"label":"Incremental", "description":"One module at a time", "value":"first", "preview":"Title: Login fails\n\nBody: A quote in the password breaks login."}, {"label":"All at once", "description":"Migrate together", "value":"second"}]
    }])).expect("canonical questions");
    let client = Rc::new(Client {
        view: RefCell::new(Some(Rc::new(PendingUserInputView {
            request_id: RuntimeRequestId("request".into()),
            questions: questions.into(),
            question_index: 0,
            selected_option_values: vec![],
            using_custom_answer: false,
            disabled: false,
            response_disabled: false,
            is_responding: false,
            dismissible: true,
        }))),
        changes: Default::default(),
        choices: Cell::new(0),
        advances: Cell::new(0),
        dismissals: Cell::new(0),
        operations: RefCell::new(vec![]),
        reject: props.reject,
    });
    let binding = ComposerQuestions::attach(
        client.clone(),
        ThreadKey {
            environment: EnvironmentId("environment".into()),
            thread: ThreadId("thread".into()),
        },
    );
    let revision = signal(0usize);
    let mut changes = client.changes.subscribe();
    spawn(async move {
        while futures::StreamExt::next(&mut changes).await.is_some() {
            revision.update(|revision| *revision += 1);
        }
    });
    let blocked = client.clone();
    let replacing = client.clone();
    let counts = client.clone();
    let settled = signal(false);
    let key_context = signal(0u8);
    for digit in 1..=9 {
        let key = digit.to_string();
        on_key(&key.clone(), move |_| {
            let context = key_context.get_untracked();
            let mut event = t3_client::view_logic::shell::shortcuts::DispatchEvent {
                key: t3_client::view_logic::shell::keybindings::KeyEvent {
                    key: key.clone(),
                    editable_focus: context == 1,
                    ..Default::default()
                },
                default_prevented: false,
                propagation_stopped: false,
                inside_confirmation: false,
            };
            binding.handle_key(&mut event, context == 2, context == 3);
        });
    }
    view()
        .w_full()
        .p(px(16.0))
        .child(composer_pending_user_input_panel(binding))
        .child(
            [
                (1, "Editable"),
                (2, "Background"),
                (3, "Side chat"),
                (0, "Unblocked"),
            ]
            .into_iter()
            .fold(view(), |column, (context, label)| {
                column.child(
                    nsh_base::button()
                        .accessibility_label(Some(label.into()))
                        .child(text(String::from(label)))
                        .on_press(move || key_context.set(context)),
                )
            }),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some("Offline".into()))
                .child(text(String::from("Offline")))
                .on_press(move || {
                    if let Some(current) = blocked.view.borrow_mut().as_mut() {
                        Rc::make_mut(current).response_disabled = true;
                        Rc::make_mut(current).disabled = true;
                    }
                    blocked.changes.notify();
                }),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some("Replace request".into()))
                .child(text(String::from("Replace request")))
                .on_press(move || {
                    if let Some(current) = replacing.view.borrow_mut().as_mut() {
                        Rc::make_mut(current).request_id = RuntimeRequestId("new-request".into());
                    }
                    replacing.changes.notify();
                }),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some("Settle timers".into()))
                .child(text(String::from("Settle timers")))
                .on_press(move || {
                    set_timeout(Duration::from_millis(400), move || settled.set(true));
                }),
        )
        .child(text(move || {
            if settled.get() {
                String::from("Timers settled")
            } else {
                String::new()
            }
        }))
        .child(text(move || {
            revision.get();
            format!(
                "Choices {} Advances {} Dismissals {}",
                counts.choices.get(),
                counts.advances.get(),
                counts.dismissals.get()
            )
        }))
        .child(text(move || {
            binding
                .error
                .get()
                .map(|error| error.resolve().to_string())
                .unwrap_or_default()
        }))
        .child(text(move || {
            revision.get();
            format!("Operations {}", client.operations.borrow().join(" | "))
        }))
}

fn start(window: (f32, f32), multi: bool, reject: bool) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<QuestionHarness>(move || QuestionHarnessProps {
            multi,
            reject,
        }))
    })
    .expect("question harness");
    app.wait_for("One module at a time")
        .expect("options from projection");
    app
}

#[test]
fn single_selection_advances_and_dismiss_does_not_toggle_the_collapsible() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, false, false);
        app.wait_for("Body: A quote in the password breaks login.")
            .expect("option preview visible");
        app.press("role:button[Incremental]").expect("select");
        app.wait_for("Choices 1 Advances 1 Dismissals 0")
            .expect("real 200 ms auto-advance");
        app.wait_for("Operations choose environment thread request scope \"first\" | advance environment thread request scope").expect("exact scoped option value and question advanced");
        app.press("role:button[Hide the question and its options]")
            .expect("collapse");
        app.wait_for("role:button[Show the question and its options]")
            .expect("collapsed");
        app.press("role:button[Show the question and its options]")
            .expect("expand");
        app.wait_for("One module at a time")
            .expect("options restored");
        app.press("role:button[Dismiss question without answering]")
            .expect("dismiss");
        app.wait_for("Choices 1 Advances 1 Dismissals 1")
            .expect("dismiss reaches service once");
        app.wait_for("Operations choose environment thread request scope \"first\" | advance environment thread request scope | dismiss environment thread request").expect("exact request dismissed");
        assert!(!app.screen().shows("One module at a time"));
    }
}

#[test]
fn multi_select_rejection_disable_and_replacement_never_auto_advance() {
    for window in [DESKTOP, PHONE] {
        for mode in ["multi", "reject", "disable", "replace"] {
            let mut app = start(window, mode == "multi", mode == "reject");
            app.press("role:button[Incremental]").expect("choose");
            match mode {
                "disable" => {
                    app.press("role:button[Offline]").expect("disconnect");
                    assert!(
                        app.press("role:button[All at once]")
                            .expect_err("offline choice")
                            .contains("disabled")
                    );
                }
                "replace" => app
                    .press("role:button[Replace request]")
                    .expect("new request"),
                "reject" => {
                    app.wait_for("Request changed")
                        .expect("service refusal retained");
                }
                _ => {}
            }
            app.press("role:button[Settle timers]")
                .expect("event after auto-advance deadline");
            app.wait_for("Timers settled").expect("timers processed");
            assert!(
                app.screen().shows(if mode == "reject" {
                    "Choices 0 Advances 0 Dismissals 0"
                } else {
                    "Choices 1 Advances 0 Dismissals 0"
                }),
                "{mode} must not advance"
            );
            if mode == "replace" {
                app.press_key("2").expect("replacement request digit");
                app.wait_for("Choices 2 Advances 1 Dismissals 0")
                    .expect("replacement mounts a live keyboard interaction");
                app.wait_for("Operations choose environment thread request scope \"first\" | choose environment thread new-request scope \"second\" | advance environment thread new-request scope").expect("only the new request advances");
            }
        }
    }
}

#[test]
fn digit_shortcuts_share_selection_and_respect_focus_collapse_and_pending_state() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, false, false);
        for context in ["Editable", "Background", "Side chat"] {
            app.press(&format!("role:button[{context}]"))
                .expect("key context");
            app.press_key("1").expect("digit event");
            assert!(app.screen().shows("Choices 0 Advances 0 Dismissals 0"));
        }
        app.press("role:button[Unblocked]").expect("key context");
        app.press("role:button[Hide the question and its options]")
            .expect("collapse");
        app.press_key("1").expect("collapsed digit");
        assert!(app.screen().shows("Choices 0 Advances 0 Dismissals 0"));
        app.press("role:button[Show the question and its options]")
            .expect("expand");
        app.press_key("9").expect("absent option");
        assert!(app.screen().shows("Choices 0 Advances 0 Dismissals 0"));
        app.press_key("1").expect("choose by number");
        app.wait_for("Choices 1 Advances 1 Dismissals 0")
            .expect("same auto-advance as click");
        app.wait_for("Operations choose environment thread request scope \"first\" | advance environment thread request scope").expect("digit preserves the native value and scope");
        app.press("role:button[Offline]").expect("disable");
        app.press_key("2").expect("disabled digit");
        assert!(app.screen().shows("Choices 1 Advances 1 Dismissals 0"));
    }
}
