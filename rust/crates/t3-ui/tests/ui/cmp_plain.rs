//! CMP-PLAIN and the composer mounts: the thread composer built by its production constructor
//! (`screens::thread::composer::thread_composer`) over a client runtime that is really paired
//! with `t3_client::testing::environment::FakeEnvironment`, or over an unconnected one where the
//! web's disconnected behaviour is the subject.

use std::rc::Rc;
use std::sync::Arc;
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
use t3_ui::screens::composer::send::{PendingMessages, pending_messages};
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
    run_status: Option<String>,
    attachment_mode: Option<String>,
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
    let replay = props.run_status.as_deref() == Some("replay");
    let mut thread_shell = shell();
    let mut detail = projection(props.question);
    if replay {
        for row in thread_shell["threads"].as_array_mut().expect("threads") {
            row["providerInstanceId"] = json!("devReplay");
            row["modelSelection"] = json!({"instanceId":"devReplay","model":"showcase"});
        }
        detail["thread"]["providerInstanceId"] = json!("devReplay");
        detail["thread"]["modelSelection"] = json!({"instanceId":"devReplay","model":"showcase"});
    } else if let Some(status) = props.run_status.as_deref() {
        let at = environment::NOW;
        detail["runs"] = json!([{"id":"run-1","threadId":"thread-a","ordinal":1,
            "providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},
            "providerThreadId":null,"userMessageId":"message-1","rootNodeId":null,
            "activeAttemptId":null,"status":status,"requestedAt":at,"startedAt":at,
            "completedAt":(status == "interrupted").then_some(at),"checkpointId":null,"contextHandoffId":null}]);
        if status == "queued" {
            detail["messages"] = json!([{"id":"message-1","threadId":"thread-a","runId":"run-1",
                "nodeId":null,"role":"user","text":"Follow up after the running turn","attachments":[],
                "streaming":false,"createdBy":"user","creationSource":"web","createdAt":at,"updatedAt":at}]);
        }
    }
    server.set_shell(thread_shell);
    server.set_thread("thread-a", detail);
    server.edit_config(|config| {
        if props.attachment_mode.as_deref() == Some("images-only") {
            config["environment"]["capabilities"]
                .as_object_mut()
                .expect("environment capabilities")
                .remove("fileAttachments");
        } else {
            config["environment"]["capabilities"]["fileAttachments"] = json!({"maxUploadBytes":1024});
        }
        if replay {
            config["settings"]["providerInstances"]["devReplay"] =
                json!({"driver":"acpRegistry","displayName":"Replay (dev)"});
            config["environment"]["capabilities"].as_object_mut().expect("capabilities").remove("modelCatalogRefresh");
            config["providers"] = json!([{
                "instanceId":"devReplay","driver":"acpRegistry","displayName":"Replay (dev)",
                "enabled":true,"installed":true,"version":null,"status":"ready",
                "auth":{"status":"unknown"},"checkedAt":environment::NOW,
                "models":[{"slug":"showcase","name":"Showcase","isCustom":false,"capabilities":null}],
                "slashCommands":[],"skills":[]
            }]);
        }
    });
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
    let attachment_controls = props.attachment_mode.as_deref().map(|mode| {
        let count = if mode == "last-slot" { 99 } else { 100 };
        let drafts = client.composer_drafts();
        let inspect = drafts.clone();
        let status = signal(String::new());
        view()
            .child(
                nsh_base::button()
                    .test_id("seed-files")
                    .on_press(move || {
                        let files: Vec<_> = (0..count).map(|i| json!({
                    "id":format!("marker-{i}"), "name":format!("marker-{i}.txt"),
                    "type":"file", "mimeType":"text/plain", "sizeBytes":1, "file":null
                })).collect();
                        drafts
                            .update(|document| {
                                document["draftsByThreadKey"]["env:thread-a"] =
                                    json!({"prompt":"","images":[],"files":files});
                                Ok(())
                            })
                            .expect("seed a writable draft");
                    })
                    .child(text("Seed files")),
            )
            .child(
                nsh_base::button()
                    .test_id("inspect-files")
                    .on_press(move || {
                        let document = inspect.document();
                        let document = document.borrow();
                        let files = &document["draftsByThreadKey"]["env:thread-a"]["files"];
                        status.set(format!(
                            "Files count {} first marker {}",
                            files.as_array().map_or(0, Vec::len),
                            files[0]["file"].is_null()
                        ));
                    })
                    .child(text("Inspect files")),
            )
            .child(text(move || status.get()))
    });
    let sender: Rc<dyn t3_ui::screens::composer::send::ComposerSendClient> =
        Rc::new(client.clone());
    let tail = pending_messages(
        sender.clone(),
        key(),
        PendingMessages::attach(sender, key()),
    );
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
        .child(attachment_controls)
        .child(tail)
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
    start_with(window, connected, question, None, None)
}

fn start_with(
    window: (f32, f32),
    connected: bool,
    question: bool,
    run_status: Option<&'static str>,
    picker: Option<Arc<dyn nsh_core::picker::PickerBackend>>,
) -> Headless {
    start_with_mode(window, connected, question, run_status, picker, None)
}

fn start_with_mode(
    window: (f32, f32),
    connected: bool,
    question: bool,
    run_status: Option<&'static str>,
    picker: Option<Arc<dyn nsh_core::picker::PickerBackend>>,
    attachment_mode: Option<&'static str>,
) -> Headless {
    let mut config = EngineConfig {
        picker,
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<ComposerFixture>(move || FixtureProps {
            connected,
            question,
            run_status: run_status.map(str::to_owned),
            attachment_mode: attachment_mode.map(str::to_owned),
        }))
    };
    if question {
        // The panel's 200 ms single-select auto-advance runs on the engine clock.
        config.clock = nsh_core::time::Clock::manual(std::time::SystemTime::UNIX_EPOCH);
    }
    if window.0 == DESKTOP.0 {
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
        app.wait_for("role:button[Attach files]")
            .expect("the composer has opened its writable draft");
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
        app.wait_for("role:button[Attach files]")
            .expect("the composer has opened its writable draft");
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

#[test]
fn resume_continues_the_interrupted_run_through_the_client() {
    let mut app = start_with(DESKTOP, true, false, Some("interrupted"), None);
    app.press("role:button[Resume thread]").expect("resume");
    wait_for_dispatch(&mut app, r#""manualContinuationOfRunId":"run-1""#);
    app.wait_for("text:/Last dispatch .*Continue where you left off/")
        .expect("the web's continuation text");
}

#[test]
fn a_queued_follow_up_is_visible_after_the_server_projects_it() {
    let mut app = start_with(DESKTOP, true, false, Some("queued"), None);
    app.wait_for("Follow up after the running turn")
        .expect("queued prompt");
}

#[test]
fn submitting_with_an_at_trigger_open_closes_the_menu_and_allows_a_new_trigger() {
    let mut app = start(DESKTOP, true, false);
    app.type_text("id:composer-editor", "Send with @Parser")
        .expect("type");
    app.wait_for("Parser rewrite").expect("suggestion menu");
    app.press("role:button[Submit message]")
        .expect("send by button");
    app.wait_for("Client draft \"\"").expect("sent");
    app.wait_for_gone("role:option", Duration::from_secs(5))
        .expect("closed");
    app.type_text("id:composer-editor", "@Parser")
        .expect("new trigger");
    app.wait_for("Parser rewrite")
        .expect("suggestions for the new draft");
}

struct TestPicker(Vec<nsh_core::picker::PickedFile>);

impl nsh_core::picker::PickerBackend for TestPicker {
    fn pick_images(&self, _: nsh_core::picker::PickRequest) -> nsh_core::picker::PickFuture {
        unreachable!("Attach files uses the multi-file picker")
    }

    fn pick_files(
        &self,
        request: nsh_core::picker::FilePickRequest,
    ) -> nsh_core::picker::FilePickFuture {
        assert_eq!(
            request.limit as usize,
            t3_contracts::chat_attachment::PROVIDER_SEND_TURN_MAX_ATTACHMENTS
        );
        Box::pin(std::future::ready(Ok(
            nsh_core::picker::FilePickSelection::new(self.0.clone()),
        )))
    }
}

#[test]
fn native_picker_attaches_readable_files_and_shows_the_size_error() {
    use nsh_core::picker::PickedFile;

    let picker = Arc::new(TestPicker(vec![
        PickedFile::new(
            "note.txt",
            "text/plain",
            nsh_core::ImageData::new(b"note".to_vec()),
        ),
        PickedFile::too_large("large.txt", "text/plain", 11 * 1024 * 1024),
    ]));
    let mut app = start_with(DESKTOP, true, false, None, Some(picker));
    app.press("role:button[Attach files]").expect("attach");
    app.wait_for("note.txt").expect("picked file on the strip");
    app.wait_for("'large.txt' exceeds the 1 KB attachment limit.")
        .expect("size rejection is visible");
}

#[test]
fn cancelling_the_picker_keeps_attach_available_and_shows_no_error() {
    let picker = Arc::new(TestPicker(Vec::new()));
    let mut app = start_with(PHONE, true, false, None, Some(picker));
    app.press("role:button[Attach files]").expect("attach");
    app.wait_until(
        "the attachment picker is usable again",
        Duration::from_secs(5),
        |screen| screen.enabled("role:button[Attach files]").unwrap_or(false),
    )
    .expect("the picker returned");
    assert!(
        !app.screen()
            .matches("role:alert")
            .expect("no attachment message")
    );
}

fn review_picker(mode: Option<&'static str>, files: Vec<nsh_core::picker::PickedFile>) -> Headless {
    let picker = Arc::new(TestPicker(files));
    // The production strip lists every reattach marker above the picker button.
    let full_draft = matches!(mode, Some("last-slot" | "reattach"));
    let window = if full_draft {
        (DESKTOP.0, 5000.0)
    } else {
        DESKTOP
    };
    let mut app = start_with_mode(window, true, false, None, Some(picker), mode);
    app.wait_until("writable composer", Duration::from_secs(5), |screen| {
        screen.enabled("role:button[Attach files]").unwrap_or(false)
    })
    .expect("attachment intake attached to the draft");
    if full_draft {
        app.press("id:seed-files").expect("seed reattach markers");
    }
    app.press("role:button[Attach files]").expect("attach");
    app
}

fn picked_file(name: &str, mime: &str, bytes: Vec<u8>) -> nsh_core::picker::PickedFile {
    let mut file = nsh_core::picker::PickedFile::new(name, mime, nsh_core::ImageData::new(bytes));
    file.name = name.into();
    file
}

#[test]
fn review_picker_allows_images_without_file_staging() {
    let mut app = review_picker(
        Some("images-only"),
        vec![
            picked_file("image.gif", "image/gif", b"GIF89a\x01\0\x01\0".to_vec()),
            picked_file("blocked.txt", "text/plain", b"x".to_vec()),
        ],
    );
    app.wait_for("role:button[Preview image.gif]")
        .expect("images do not need file staging");
    app.wait_for("This server does not support file attachments.")
        .expect("the staging restriction still applies to documents");
}

#[test]
fn review_picker_uses_the_image_limit_for_images() {
    let mut jpeg = vec![0; 3 * 1024 * 1024];
    jpeg[..3].copy_from_slice(&[0xff, 0xd8, 0xff]);
    let mut app = review_picker(
        None,
        vec![
            picked_file("photo.jpg", "image/jpeg", jpeg),
            picked_file("too-big.txt", "text/plain", vec![b'x'; 2048]),
        ],
    );
    app.wait_for("role:button[Preview photo.jpg]")
        .expect("an image may exceed the document limit");
    app.wait_for("'too-big.txt' exceeds the 1 KB attachment limit.")
        .expect("documents still use the server limit");
}

#[test]
fn review_picker_reports_an_overfull_draft() {
    let mut app = review_picker(
        Some("last-slot"),
        vec![
            picked_file("fits.txt", "text/plain", b"x".to_vec()),
            picked_file("over.txt", "text/plain", b"x".to_vec()),
        ],
    );
    app.wait_for("fits.txt").expect("one more attachment fits");
    app.wait_for("You can attach up to 100 files per message.")
        .expect("the rest of the selection is reported");
}

#[test]
fn review_picker_replaces_a_reattach_marker_at_capacity() {
    let mut app = review_picker(
        Some("reattach"),
        vec![picked_file("marker-0.txt", "text/plain", b"x".to_vec())],
    );
    app.wait_until("picker completes", Duration::from_secs(5), |screen| {
        screen.enabled("role:button[Attach files]").unwrap_or(false)
    })
    .expect("picker returns");
    app.press("id:inspect-files").expect("inspect draft");
    app.wait_for("Files count 100 first marker false")
        .expect("replacement uses the existing slot");
}

struct FailingPicker;

impl nsh_core::picker::PickerBackend for FailingPicker {
    fn pick_images(&self, _: nsh_core::picker::PickRequest) -> nsh_core::picker::PickFuture {
        unreachable!("Attach files uses the multi-file picker")
    }
    fn pick_files(&self, _: nsh_core::picker::FilePickRequest) -> nsh_core::picker::FilePickFuture {
        Box::pin(std::future::ready(Err(
            nsh_core::picker::FilePickError::Dialog("no document portal".into()),
        )))
    }
}

#[test]
fn review_picker_displays_a_dialog_failure() {
    let mut app = start_with(DESKTOP, true, false, None, Some(Arc::new(FailingPicker)));
    app.press("role:button[Attach files]").expect("attach");
    app.wait_for("The file picker failed: no document portal")
        .expect("dialog failure is visible");
}

#[test]
fn review_picker_displays_unreadable_files_and_keeps_readable_ones() {
    let mut app = review_picker(
        None,
        vec![
            picked_file("readable.txt", "text/plain", b"x".to_vec()),
            nsh_core::picker::PickedFile::unreadable("missing.txt"),
        ],
    );
    app.wait_for("readable.txt")
        .expect("readable selection kept");
    app.wait_for("'missing.txt' is empty or could not be read.")
        .expect("read failure is visible");
}

#[test]
fn review_picker_reports_more_than_one_hundred_selected_files() {
    let mut app = review_picker(
        None,
        vec![picked_file("note.txt", "text/plain", b"x".to_vec()); 101],
    );
    app.wait_for("note.txt")
        .expect("the selection was retained");
    app.wait_for("You can attach up to 100 files per message.")
        .expect("selection limit is visible");
}

#[test]
fn the_model_picker_suppresses_unsupported_refresh_errors() {
    let mut app = start_with(DESKTOP, true, false, Some("replay"), None);
    app.wait_for("role:button[Showcase]")
        .expect("the seeded selection names the model");
    app.press("role:button[Showcase]").expect("open the picker");
    app.settle();
    assert!(
        !app.screen()
            .matches("text:/unsupported client operation/")
            .expect("selector")
    );
}
