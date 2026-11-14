//! CMP-PLAIN and the composer mounts: the thread composer built by its production constructor
//! (`screens::thread::composer::thread_composer`) over a client runtime that is really paired
//! with `t3_client::testing::environment::FakeEnvironment`, or over an unconnected one where the
//! web's disconnected behaviour is the subject.

use std::rc::Rc;
use std::time::Duration;

use futures::{FutureExt, StreamExt};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::{Value, json};
use t3_client::ClientStore;
use t3_client::composition::{ClientRuntime, Spawner};
use t3_client::testing::environment::{self, FakeEnvironment};
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_contracts::orchestration_v2::{
    OrchestrationV2CreationSource, OrchestrationV2ShellStreamItem, OrchestrationV2ThreadStreamItem,
};
use t3_ui::keys::bindings::{Keybindings, keyboard_events};
use t3_ui::runtime::{Rt, backend::Unavailable, host::Choice};
use t3_ui::screens::thread::composer::thread_composer;
use t3_ui::state::nav::{Navigator, Route};

use crate::support::{DESKTOP, PHONE};

const DISPATCH: &str = "orchestration.dispatchCommand";
const SEARCH: &str = "projects.searchEntries";

fn key() -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId("env".into()),
        thread: ThreadId("thread-a".into()),
    }
}

fn shell() -> Value {
    environment::shell(&[
        ("thread-a", "Composer thread"),
        ("thread-b", "Parser rewrite"),
    ])
}

fn projection(question: bool) -> Value {
    if !question {
        return environment::projection("thread-a", "Composer thread", json!([]), json!([]));
    }
    let at = environment::NOW;
    environment::projection(
        "thread-a",
        "Composer thread",
        json!([{"id":"request","nodeId":"node","providerTurnId":null,"nativeRequestRef":null,
            "kind":"user_input","status":"pending",
            "responseCapability":{"type":"live","providerSessionId":"session"},
            "createdAt":at,"resolvedAt":null}]),
        json!([{"type":"user_input_request","id":"input-item","threadId":"thread-a","runId":null,
            "nodeId":"node","providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,
            "parentItemId":null,"ordinal":0,"status":"running","title":null,"startedAt":null,
            "completedAt":null,"updatedAt":at,"requestId":"request",
            "questions":[{"id":"scope","header":"Scope","question":"Which scope should change?",
                "options":[{"label":"Parser only","description":"Touch the parser","value":"parser"},
                           {"label":"Everything","description":"Touch every crate","value":"all"}]}]}]),
    )
}

/// The store as an unconnected client keeps it after an earlier session.
fn seed(store: &ClientStore, question: bool) {
    let environment = key().environment;
    let session = store.begin_session(environment.clone());
    let shell_items = [
        json!({"kind": "snapshot", "snapshot": shell()}),
        json!({"kind": "synchronized"}),
    ]
    .into_iter()
    .map(|item| serde_json::from_value::<OrchestrationV2ShellStreamItem>(item).expect("shell item"))
    .collect();
    store.apply_shell(&environment, session, shell_items);
    let thread_items = [
        json!({"kind": "snapshot", "snapshotSequence": 1, "projection": projection(question)}),
        json!({"kind": "synchronized"}),
    ]
    .into_iter()
    .map(|item| {
        serde_json::from_value::<OrchestrationV2ThreadStreamItem>(item).expect("thread item")
    })
    .collect();
    store
        .apply_thread(key(), session, thread_items)
        .expect("thread applies");
}

#[derive(Clone, PartialEq, Props)]
struct FixtureProps {
    connected: bool,
    question: bool,
}

/// The production composer under a client runtime, with what a test can observe: mounting
/// (leaving the thread), the client's draft and question state, the server's received calls,
/// and how often the timeline's store selector recomputes.
#[component]
fn ComposerFixture(props: FixtureProps) -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let store = ClientStore::default();
    provide_context(Rt::new(
        store.clone(),
        Rc::new(Unavailable("fixture".into())),
        Choice::Demo,
    ));
    provide_context(Navigator::new(Route::Home));
    let keys = Keybindings::attach(store.clone(), signal(vec![]), signal(None));
    provide_context(keys);
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        });
    });
    let server = FakeEnvironment::new("env", spawner.clone());
    server.set_shell(shell());
    server.set_thread("thread-a", projection(props.question));
    let client = ClientRuntime::new(
        server.platform(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    if props.connected {
        let pairing = client.pair(server.pairing());
        let opening = client.clone();
        spawner(
            async move {
                if pairing.await.is_ok() {
                    opening.open_thread(&key());
                }
            }
            .boxed_local(),
        );
    } else {
        seed(&store, props.question);
    }
    let ready = signal(false);
    {
        let client = client.clone();
        let mut changes = store.subscribe();
        spawn(async move {
            while changes.next().await.is_some() {
                let session = client
                    .client(&key().environment)
                    .and_then(|connected| connected.session())
                    .is_some();
                let thread = client
                    .store()
                    .snapshot()
                    .environments
                    .get(&key().environment)
                    .and_then(|environment| environment.threads.get(&key().thread))
                    .is_some_and(|thread| thread.projection.is_some());
                ready.set(session && thread);
            }
        });
    }
    let draft = signal(String::new());
    let selected = signal(Vec::<String>::new());
    {
        let drafts = client.composer_drafts();
        let mut changes = drafts.changes();
        spawn(async move {
            while changes.next().await.is_some() {
                draft.set(drafts.view(&key()).prompt.to_string());
            }
        });
        let questions = client.pending_user_input(&key());
        let mut changes = questions.changes();
        spawn(async move {
            while changes.next().await.is_some() {
                selected.set(
                    questions
                        .view()
                        .map(|view| view.selected_option_values.clone())
                        .unwrap_or_default(),
                );
            }
        });
    }
    let timeline = t3_ui::runtime::rt().store.timeline(key());
    let timeline_runs = signal(0_u32);
    effect(move || {
        timeline.get();
        untracked(|| timeline_runs.update(|runs| *runs += 1));
    });
    let dispatches = signal(Vec::<Value>::new());
    let searches = signal(0_usize);
    let mounted = signal(true);
    let content = view()
        .w_full()
        .h_full()
        .child(
            nsh_base::button()
                .test_id("leave")
                .on_press(move || mounted.set(false))
                .child(text("Leave thread")),
        )
        .child(
            nsh_base::button()
                .test_id("return")
                .on_press(move || mounted.set(true))
                .child(text("Return to thread")),
        )
        .child(
            nsh_base::button()
                .test_id("read-server")
                .on_press(move || {
                    dispatches.set(server.calls(DISPATCH));
                    searches.set(server.calls(SEARCH).len());
                })
                .child(text("Read server")),
        )
        .child(text(move || {
            if ready.get() {
                "Session ready"
            } else {
                "No session"
            }
        }))
        .child(text(move || format!("Client draft {:?}", draft.get())))
        .child(text(move || format!("Client answer {:?}", selected.get())))
        .child(text(move || {
            format!("Dispatches {}", dispatches.with(Vec::len))
        }))
        .child(text(move || {
            format!(
                "Last dispatch {}",
                dispatches.with(|calls| calls.last().map(Value::to_string).unwrap_or_default())
            )
        }))
        .child(text(move || format!("Searches {}", searches.get())))
        .child(text(move || {
            format!("Timeline derivations {}", timeline_runs.get())
        }))
        .child(dynamic(move || {
            mounted.get().then(|| {
                let client = client.clone();
                untracked(move || thread_composer(client, key()))
            })
        }));
    t3_ui::ui::toast::toast_provider(
        keyboard_events(keys, content),
        t3_ui::ui::toast::Position::TopRight,
    )
}

fn start(window: (f32, f32), connected: bool, question: bool) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<ComposerFixture>(move || FixtureProps {
            connected,
            question,
        }))
    };
    if question {
        // The panel's 200 ms single-select auto-advance runs on the engine clock.
        config.clock = nsh_core::time::Clock::manual(std::time::SystemTime::UNIX_EPOCH);
    }
    if window == DESKTOP {
        config.environment.os = nsh_core::platform::Os::Linux;
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    }
    let mut app = Headless::start(config).expect("composer fixture starts");
    app.wait_for("id:composer-editor")
        .expect("the composer mounts");
    if connected {
        app.wait_for("Session ready")
            .expect("paired and the thread is open");
    }
    app
}

fn read_server(app: &mut Headless) {
    app.press("id:read-server")
        .expect("read what the server received");
}

/// Re-reads the server until a dispatch matching `pattern` (a regex over the last one) arrives.
fn wait_for_dispatch(app: &mut Headless, pattern: &str) {
    let selector = format!("text:/Last dispatch .*{pattern}/");
    for _ in 0..50 {
        read_server(app);
        if app.screen().matches(&selector).expect("selector") {
            return;
        }
        app.settle();
    }
    panic!(
        "no dispatch matched {pattern}; on screen: {:?}",
        app.screen().texts()
    );
}

#[test]
fn a_draft_survives_leaving_and_returning_to_the_thread() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, false, false);
        app.type_text("id:composer-editor", "keep this draft")
            .expect("type into the editor");
        app.wait_for("Client draft \"keep this draft\"")
            .expect("the client's draft store holds the text");
        app.press("id:leave").expect("leave");
        app.wait_for_gone("id:composer-editor", Duration::from_secs(5))
            .expect("unmounted");
        app.press("id:return").expect("return");
        app.wait_for("id:composer-editor").expect("remounted");
        app.wait_for("keep this draft")
            .expect("the editor restores the client's draft");
    }
}

#[test]
fn enter_sends_once_through_the_client_and_clears_the_draft() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, true, false);
        app.type_text("id:composer-editor", "ship it")
            .expect("type");
        app.wait_for("Client draft \"ship it\"").expect("saved");
        app.press_key("enter").expect("send with Enter");
        app.wait_for("Client draft \"\"")
            .expect("the client clears the sent draft");
        read_server(&mut app);
        app.wait_for("Dispatches 1")
            .expect("one dispatch reached the server");
        app.wait_for("text:/Last dispatch .*ship it/")
            .expect("carrying the typed text");
        app.press_key("enter").expect("Enter on the empty composer");
        read_server(&mut app);
        assert!(
            app.screen().shows("Dispatches 1"),
            "an empty composer sends nothing"
        );
    }
}

#[test]
fn at_suggests_a_thread_and_the_chosen_chip_is_sent_as_a_context_reference() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, true, false);
        app.type_text("id:composer-editor", "see @Parser")
            .expect("type a mention trigger");
        app.wait_for("Parser rewrite")
            .expect("the client's thread suggestion is listed");
        app.press_key("enter")
            .expect("choose the active suggestion");
        app.wait_for_gone("role:option", Duration::from_secs(5))
            .expect("the menu closes after the client accepts the selection");
        app.press_key("enter").expect("send");
        app.wait_for("Client draft \"\"").expect("sent");
        read_server(&mut app);
        app.wait_for("Dispatches 1").expect("one dispatch");
        app.wait_for(r#"text:/Last dispatch .*"kind":"thread".*"threadId":"thread-b"/"#)
            .expect("the chosen thread travels as a context reference");
    }
}

#[test]
fn a_digit_answers_the_pending_question_through_the_client() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, true, true);
        app.wait_for("Which scope should change?")
            .expect("the question panel is in the banner dock");
        // Digits are the panel's while focus is outside an editable field.
        read_server(&mut app);
        app.press_key("1").expect("digit shortcut");
        app.wait_for("Client answer [\"parser\"]")
            .expect("the client's question session records the answer");
        app.run_for(Duration::from_millis(300))
            .expect("auto-advance");
        read_server(&mut app);
        app.wait_for(r#"text:/Last dispatch .*"scope":"parser"/"#)
            .expect("the answer is sent to the server");
    }
}

#[test]
fn digits_typed_in_the_answer_editor_stay_in_the_answer() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, true, true);
        app.wait_for("Which scope should change?")
            .expect("question");
        app.type_text("id:composer-editor", "1")
            .expect("type a digit");
        app.wait_for("Client draft \"1\"")
            .expect("the digit went to the question's answer draft");
        assert!(
            app.screen().shows("Client answer []"),
            "no option was chosen"
        );
    }
}

#[test]
fn sending_while_disconnected_takes_the_webs_toast_and_keeps_the_draft() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, false, false);
        app.type_text("id:composer-editor", "not yet")
            .expect("type");
        app.wait_for("Client draft \"not yet\"").expect("saved");
        app.press_key("enter").expect("send with Enter");
        app.wait_for("Not connected: message not sent")
            .expect("the web's warning toast");
        app.wait_for("Reconnecting to the environment. Try again once it is connected.")
            .expect("its description");
        assert!(
            app.screen().shows("Client draft \"not yet\""),
            "the draft is kept"
        );
        assert!(
            !app.screen()
                .shows("Not connected. Your message was not sent."),
            "no invented failure line"
        );
    }
}

/// Typing latency: keystrokes leave the timeline's selector alone and query the server only
/// when the suggestion's input changed.
#[test]
fn keystrokes_neither_recompute_the_timeline_nor_repeat_unchanged_queries() {
    let mut app = start(DESKTOP, true, false);
    let timeline = app
        .screen()
        .texts()
        .into_iter()
        .find(|text| text.starts_with("Timeline derivations"))
        .map(|text| text.to_string())
        .expect("counter shown");
    read_server(&mut app);
    app.wait_for("Searches 0")
        .expect("no query before a trigger");
    app.type_text("id:composer-editor", "@Par")
        .expect("trigger");
    app.wait_for("Parser rewrite").expect("results");
    read_server(&mut app);
    let after_trigger = app
        .screen()
        .texts()
        .into_iter()
        .find(|text| text.starts_with("Searches "))
        .map(|text| text.to_string())
        .expect("search count");
    assert_ne!(after_trigger, "Searches 0", "path queries are counted");
    app.type_text("id:composer-editor", " then plain words without a trigger")
        .expect("type past the trigger");
    app.wait_for("text:/Client draft .*without a trigger/")
        .expect("typed");
    read_server(&mut app);
    assert!(
        app.screen().shows(&after_trigger),
        "no query ran once the trigger closed"
    );
    assert!(
        app.screen().shows(&timeline),
        "typing never recomputed the timeline selector"
    );
}

/// The send button's press rebuilds the action group (busy); its send must still run.
#[test]
fn the_send_button_sends_once_and_the_composer_settles() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, true, false);
        app.type_text("id:composer-editor", "button send")
            .expect("type");
        app.wait_for("Client draft \"button send\"").expect("saved");
        app.press("role:button[Submit message]")
            .expect("press send");
        wait_for_dispatch(&mut app, "button send");
        app.wait_for("Client draft \"\"").expect("the draft clears");
        app.wait_until(
            "the send button settles",
            Duration::from_secs(5),
            |screen| {
                screen
                    .matches("role:button[Submit message]")
                    .expect("selector")
            },
        )
        .expect("not busy");
        read_server(&mut app);
        assert!(app.screen().shows("Dispatches 1"), "sent once");
    }
}
