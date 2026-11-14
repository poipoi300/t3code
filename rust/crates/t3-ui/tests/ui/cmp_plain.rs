//! CMP-PLAIN and the composer mounts: the thread composer built by its production constructor
//! (`screens::thread::composer::thread_composer`) over a `FakePlatform` client runtime whose store
//! holds a seeded shell and thread. The runtime has no connected session (a UI test cannot reach
//! the client's in-process fake server), so delivery itself is not exercised here.

use std::rc::Rc;

use futures::StreamExt;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::{Value, json};
use t3_client::ClientStore;
use t3_client::composition::{ClientRuntime, Spawner};
use t3_client::testing::FakePlatform;
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_contracts::orchestration_v2::{
    OrchestrationV2CreationSource, OrchestrationV2ShellStreamItem, OrchestrationV2ThreadStreamItem,
};
use t3_ui::keys::bindings::{Keybindings, keyboard_events};
use t3_ui::runtime::{Rt, backend::Unavailable, host::Choice};
use t3_ui::screens::thread::composer::thread_composer;
use t3_ui::state::nav::{Navigator, Route};

use crate::support::{DESKTOP, PHONE};

const AT: &str = "2026-11-13T12:00:00.000Z";

fn key() -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId("env".into()),
        thread: ThreadId("thread-a".into()),
    }
}

fn thread_json(id: &str, title: &str) -> Value {
    json!({
        "id": id, "projectId": "project", "title": title,
        "providerInstanceId": "codex",
        "modelSelection": {"instanceId": "codex", "model": "gpt-5.4"},
        "runtimeMode": "full-access", "interactionMode": "default",
        "branch": null, "worktreePath": null, "activeProviderThreadId": null,
        "lineage": {"rootThreadId": id, "parentThreadId": null, "relationshipToParent": null},
        "forkedFrom": null, "createdBy": "user", "creationSource": "web",
        "createdAt": AT, "updatedAt": AT,
        "archivedAt": null, "settledOverride": null, "settledAt": null,
        "lastVisitedAt": null, "deletedAt": null
    })
}

fn shell() -> Value {
    let row = |id: &str, title: &str| {
        let mut thread = thread_json(id, title);
        // No project: without a session the client's `@` query would stop at path search
        // (see the hand-off); a project-less thread has no workspace to search.
        thread["projectId"] = json!("no-project");
        thread.as_object_mut().expect("thread").extend(
            json!({
                "latestRunId": null, "activeRunId": null, "status": "idle",
                "pendingRuntimeRequest": null, "latestVisibleMessage": null,
                "latestUserMessageAt": null, "hasActionableProposedPlan": false,
                "itemCount": 0, "visibleItemCount": 0
            })
            .as_object()
            .cloned()
            .unwrap_or_default(),
        );
        thread
    };
    json!({
        "schemaVersion": 1, "snapshotSequence": 1,
        "threads": [row("thread-a", "Composer thread"), row("thread-b", "Parser rewrite")],
        "archivedThreads": [],
        "projects": [{
            "id": "project", "title": "Project", "workspaceRoot": "/work/project",
            "repositoryIdentity": null, "defaultModelSelection": null, "scripts": [],
            "createdAt": AT, "updatedAt": AT
        }]
    })
}

fn projection(question: bool) -> Value {
    let (requests, items) = if question {
        (
            json!([{"id":"request","nodeId":"node","providerTurnId":null,"nativeRequestRef":null,
                "kind":"user_input","status":"pending",
                "responseCapability":{"type":"live","providerSessionId":"session"},
                "createdAt":AT,"resolvedAt":null}]),
            json!([{"type":"user_input_request","id":"input-item","threadId":"thread-a","runId":null,
                "nodeId":"node","providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,
                "parentItemId":null,"ordinal":0,"status":"running","title":null,"startedAt":null,
                "completedAt":null,"updatedAt":AT,"requestId":"request",
                "questions":[{"id":"scope","header":"Scope","question":"Which scope should change?",
                    "options":[{"label":"Parser only","description":"Touch the parser","value":"parser"},
                               {"label":"Everything","description":"Touch every crate","value":"all"}]}]}]),
        )
    } else {
        (json!([]), json!([]))
    };
    json!({
        "thread": thread_json("thread-a", "Composer thread"),
        "runs": [], "attempts": [], "nodes": [], "subagents": [], "providerSessions": [],
        "providerThreads": [], "providerTurns": [], "runtimeRequests": requests, "messages": [],
        "plans": [], "turnItems": items, "checkpointScopes": [], "checkpoints": [],
        "contextHandoffs": [], "contextTransfers": [], "visibleTurnItems": [],
        "updatedAt": AT
    })
}

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
    question: bool,
}

/// The production composer under a fake client runtime, with what a user and a test can observe:
/// mount/unmount (leaving the thread), and the client's own draft and question state.
#[component]
fn ComposerFixture(props: FixtureProps) -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let store = ClientStore::default();
    seed(&store, props.question);
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
    let client = ClientRuntime::new(
        FakePlatform::default(),
        store,
        OrchestrationV2CreationSource::Web,
        spawner,
    );
    let mounted = signal(true);
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
    let outgoing = {
        let client = client.clone();
        move || {
            let drafts = client.composer_drafts();
            let id = drafts.editor_draft_key(&key());
            let document = drafts.document();
            let raw = document.borrow()["draftsByThreadKey"][&id].clone();
            t3_client::thread_views::composer::context_records::outgoing_message_context(&raw, &[])
                .ok()
                .flatten()
                .map(|context| serde_json::to_string(&context).expect("context"))
                .unwrap_or_default()
        }
    };
    let outgoing_text = signal(String::new());
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
                .test_id("outgoing")
                .on_press(move || outgoing_text.set(outgoing()))
                .child(text("Read outgoing context")),
        )
        .child(text(move || format!("Client draft {:?}", draft.get())))
        .child(text(move || format!("Client answer {:?}", selected.get())))
        .child(text(move || format!("Outgoing {}", outgoing_text.get())))
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

fn start(window: (f32, f32), question: bool) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<ComposerFixture>(move || FixtureProps {
            question,
        }))
    };
    if window == DESKTOP {
        config.environment.os = nsh_core::platform::Os::Linux;
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    }
    let mut app = Headless::start(config).expect("composer fixture starts");
    app.wait_for("id:composer-editor")
        .expect("the composer mounts");
    app
}

#[test]
fn a_draft_survives_leaving_and_returning_to_the_thread() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, false);
        app.type_text("id:composer-editor", "keep this draft")
            .expect("type into the editor");
        app.wait_for("Client draft \"keep this draft\"")
            .expect("the client's draft store holds the text");
        app.press("id:leave").expect("leave");
        app.wait_for_gone("id:composer-editor", std::time::Duration::from_secs(5))
            .expect("unmounted");
        app.press("id:return").expect("return");
        app.wait_for("id:composer-editor").expect("remounted");
        app.wait_for("keep this draft")
            .expect("the editor restores the client's draft");
    }
}

#[test]
fn at_opens_thread_suggestions_and_choosing_one_inserts_a_context_reference() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, false);
        app.type_text("id:composer-editor", "see @Parser")
            .expect("type a mention trigger");
        app.wait_for("Parser rewrite")
            .expect("the client's thread suggestion is listed");
        app.press_key("enter")
            .expect("choose the active suggestion");
        app.wait_for_gone("role:option", std::time::Duration::from_secs(5))
            .expect("the menu closes after the client accepts the selection");
        app.press("id:outgoing").expect("read the outgoing context");
        app.wait_for(r#"text:/"kind":"thread"/"#)
            .expect("the draft sends the chosen thread as a context reference");
        app.wait_for(r#"text:/"threadId":"thread-b"/"#)
            .expect("for the chosen thread");
    }
}

#[test]
fn a_pending_question_is_docked_and_digits_typed_in_the_editor_stay_in_the_answer() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, true);
        app.wait_for("Which scope should change?")
            .expect("the question panel is in the banner dock");
        app.wait_for("Parser only").expect("its options");
        app.wait_for("text:/Type your own answer, or leave this blank to use the selected option/")
            .expect("the editor is the custom-answer field");
        // Without a session the client disables the request (`!canOperate`), so the panel's
        // digit shortcut is not installed; typed digits are the answer's text.
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
        let mut app = start(window, false);
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
