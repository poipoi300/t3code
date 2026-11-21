use super::*;
use crate::runtime::{Rt, host::Choice, services::thread_ops};
use crate::state::nav::{Navigator, Route};
use futures::FutureExt;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::{Value as Json, json};
use t3_client::{
    ClientStore,
    composition::{ClientRuntime, Spawner},
    testing::environment,
    types::{SyncStatus, ThreadId},
};
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;

const AT: &str = "2026-11-13T12:00:00.000Z";

fn sample(input: &str) -> Json {
    serde_json::from_str(input.lines().next().expect("sample")).expect("valid JSON")
}

fn projection() -> Json {
    let mut p = environment::projection("thread", "Two turns", json!([]), json!([]));
    let items: Vec<_> = ["first", "second"]
        .into_iter()
        .enumerate()
        .map(|(i, id)| {
            json!({"id":id,"type":"user_message","threadId":"thread","runId":id,"nodeId":null,
            "providerThreadId":"pt","providerTurnId":null,"nativeItemRef":null,"parentItemId":null,
            "ordinal":i+1,"status":"completed","title":null,"startedAt":AT,"completedAt":AT,
            "updatedAt":AT,"messageId":id,"text":format!("{id} prompt"),"inputIntent":"turn_start",
            "attachments":[],"createdBy":"user","creationSource":"web"})
        })
        .collect();
    p["turnItems"] = json!(items);
    p["visibleTurnItems"] = json!(items.iter().enumerate().map(|(position,item)| json!({
        "position":position,"visibility":"local","sourceThreadId":"thread","sourceItemId":item["id"],"item":item
    })).collect::<Vec<_>>());
    p["thread"]["activeProviderThreadId"] = json!("pt");
    p["providerThreads"] = json!([{"id":"pt","driver":"codex","providerInstanceId":"codex",
        "providerSessionId":"s","appThreadId":"thread","ownerNodeId":null,"nativeThreadRef":null,
        "nativeConversationHeadRef":null,"status":"idle","firstRunOrdinal":1,"lastRunOrdinal":1,
        "handoffIds":[],"forkedFrom":null,"createdAt":AT,"updatedAt":AT}]);
    p["runs"] = json!([{"id":"first","threadId":"thread","ordinal":1,"providerInstanceId":"codex",
        "modelSelection":p["thread"]["modelSelection"],"providerThreadId":"pt","userMessageId":"first",
        "rootNodeId":null,"activeAttemptId":"attempt","status":"completed","requestedAt":AT,
        "startedAt":AT,"completedAt":AT,"checkpointId":"checkpoint","contextHandoffId":null}]);
    p["providerTurns"] = json!([{"id":"turn","providerThreadId":"pt","nodeId":"node",
        "runAttemptId":"attempt","nativeTurnRef":null,"ordinal":1,"status":"completed","startedAt":AT,"completedAt":AT}]);
    p["checkpoints"] = json!([{"id":"checkpoint","scopeId":"scope","threadId":"thread","nodeId":"node",
        "runId":"first","parentCheckpointId":null,"ordinalWithinScope":1,"appRunOrdinal":1,"ref":"ref",
        "status":"ready","capturedAt":AT,"files":[{"path":"a.rs","kind":"modified","additions":2,"deletions":1}],
        "filesTotal":{"count":2,"additions":5,"deletions":1}}]);
    p["messages"] = json!([{"id":"reply","threadId":"thread","runId":"first","nodeId":null,
        "role":"assistant","text":"reply","attachments":[],"streaming":false,"createdBy":"agent",
        "creationSource":"web","createdAt":AT,"updatedAt":AT}]);
    p
}

#[component]
fn TwoTurns() -> impl IntoElement {
    fixture(false)
}
#[component]
fn ReadOnly() -> impl IntoElement {
    fixture(true)
}

fn fixture(read_only: bool) -> impl IntoElement {
    crate::theme::install(&Default::default());
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        })
    });
    let server = environment::FakeEnvironment::new("env", spawner.clone());
    if read_only {
        server.set_permissions(&["orchestration:read"]);
    }
    server.set_shell(environment::shell(&[("thread", "Two turns")]));
    server.set_thread("thread", projection());
    server.edit_config(|config| {
        let mut provider = sample(include_str!(
            "../../../../../fixtures/samples/ServerProvider.jsonl"
        ));
        provider["instanceId"] = json!("codex");
        provider["supportsConversationRollback"] = json!(true);
        config["providers"] = json!([provider]);
    });
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        server.platform(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let key = ThreadKey {
        environment: EnvironmentId("env".into()),
        thread: ThreadId("thread".into()),
    };
    provide_context(Rt::new(
        store.clone(),
        Rc::new(client.clone()),
        Choice::Demo,
    ));
    provide_context(Navigator::new(Route::Thread(key.clone())));
    provide_context(thread_ops::timeline_rewind(&client));
    provide_context(thread_ops::approval_services(&client));
    let pair = client.pair(server.pairing());
    let open = key.clone();
    spawner(
        async move {
            pair.await.expect("paired");
            client.open_thread(&open);
        }
        .boxed_local(),
    );
    let services = TimelineServices::new(key.clone());
    let input = rewind_inputs(key.clone(), &services);
    let scope = key.clone();
    view()
        .w_full()
        .h_full()
        .child(
            nsh_base::button()
                .test_id("cache")
                .on_press(move || {
                    let generation = store.snapshot().environments[&scope.environment].session;
                    store.update_thread(&scope, generation, |thread| {
                        thread.status = SyncStatus::Cached;
                        true
                    });
                })
                .child(text("Use cached thread")),
        )
        .child(text(move || {
            serde_json::to_string(&input.get().turn_diff_summaries).expect("summaries")
        }))
        .child(timeline(key))
}

fn start(root: Root) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: crate::registries(),
        viewport: size(px(1280.), px(850.)),
        ..EngineConfig::new(root)
    })
    .expect("timeline");
    app.wait_for("second prompt").expect("second turn");
    app
}

#[test]
fn read_only_timeline_retains_the_complete_checkpoint_summary() {
    let app = start(Root::of::<ReadOnly>());
    assert_eq!(app.screen().count("role:button[Edit from here]"), Ok(0));
    let summaries = app
        .screen()
        .texts()
        .into_iter()
        .find_map(|s| serde_json::from_str::<Vec<Json>>(s).ok())
        .expect("summary");
    assert_eq!(
        summaries,
        vec![
            json!({"checkpointId":"checkpoint","scopeId":"scope","runId":"first",
        "checkpointTurnCount":1,"checkpointRef":"ref","status":"ready","completedAt":AT,
        "assistantMessageId":"reply","files":[{"path":"a.rs","kind":"modified","additions":2,"deletions":1}],
        "filesTotal":{"count":2,"additions":5,"deletions":1}})
        ]
    );
}

#[test]
fn second_turn_edit_uses_the_previous_checkpoint_even_with_cached_history() {
    let mut app = start(Root::of::<TwoTurns>());
    let edit = "role:button[Edit from here]";
    app.wait_for(edit).expect("previous turn checkpoint");
    assert_eq!(
        app.screen().count(edit),
        Ok(1),
        "no root checkpoint for the first prompt"
    );
    app.press("id:cache")
        .expect("cached history with the same operate grant");
    assert_eq!(app.screen().count(edit), Ok(1));
    app.press(edit).expect("edit cached history");
    app.wait_for("text:/Reconnect .* before reverting checkpoints/")
        .expect("reconnect instruction");
}
