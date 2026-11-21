//! The real user row requests confirmation, then calls a client service, never a raw rollback.

use crate::support::{DESKTOP, PHONE};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use std::rc::Rc;
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRowsInput, derive_messages_timeline_rows,
};
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_client::view_logic::session::TimelineEntry;
use t3_contracts::{
    base_schemas::{RunId, TurnItemId},
    orchestration_v2_id_table::IdOrIndex,
};
use t3_ui::runtime::tasks::TaskOwner;
use t3_ui::screens::thread::{
    rewind::*,
    rows::assistant::ForkRequest,
    rows::user::{UserRowContext, UserRowState, user_message_row},
    timeline::TimelineServices,
};
use t3_ui::state::nav::{Navigator, Route};
use t3_ui::ui::confirm::confirm_dialog_host;

fn key(id: &str) -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId("environment".into()),
        thread: ThreadId(id.into()),
    }
}

#[derive(Clone)]
struct Client {
    allowed: Signal<bool>,
    files: Signal<bool>,
    fail: Signal<bool>,
    calls: Signal<Vec<(String, u64, bool)>>,
    forks: Signal<usize>,
}

impl TimelineRewindClient for Client {
    fn state(&self, _: &ThreadKey) -> RewindState {
        RewindState { reverting: false }
    }
    fn availability(&self, _: &ThreadKey, _: &RewindTarget) -> RewindAvailability {
        RewindAvailability {
            can_rewind: self.allowed.get_untracked(),
            can_restore_files: self.files.get_untracked(),
            reason: (!self.allowed.get_untracked()).then(|| RewindError {
                message: "Interrupt the current turn before reverting checkpoints.".into(),
                detail: None,
            }),
        }
    }
    fn changes(&self, _: &ThreadKey) -> LocalBoxStream<'static, ()> {
        futures::stream::pending().boxed_local()
    }
    fn rewind(
        &self,
        thread: ThreadKey,
        target: RewindTarget,
        files: bool,
    ) -> LocalBoxFuture<'static, Result<String, RewindError>> {
        assert_eq!(thread, key("parent"));
        self.calls
            .update(|calls| calls.push((target.message_id.0, target.turn_count, files)));
        futures::future::ready(if self.fail.get_untracked() {
            Err(RewindError {
                message: "Failed to revert thread state.".into(),
                detail: Some("Settings".into()),
            })
        } else {
            Ok("Restored prompt from client".into())
        })
        .boxed_local()
    }
    fn fork(
        &self,
        viewed: ThreadKey,
        source: t3_client::types::ThreadId,
        _: t3_contracts::base_schemas::TurnItemId,
        run: RunId,
    ) -> LocalBoxFuture<'static, Result<ThreadKey, RewindError>> {
        assert_eq!(viewed, key("parent"));
        assert_eq!(source, key("ancestor").thread);
        assert_eq!(run.0, "run");
        self.forks.update(|count| *count += 1);
        futures::future::ready(if self.fail.get_untracked() {
            Err(RewindError {
                message: "Failed to fork this response.".into(),
                detail: None,
            })
        } else {
            Ok(key("fork"))
        })
        .boxed_local()
    }
}

fn target_row() -> Rc<t3_client::thread_views::messages_timeline::MessagesTimelineRow> {
    let entry: TimelineEntry = serde_json::from_value(json!({"id":"prompt","kind":"message","createdAt":"2026-11-13T12:00:00Z","message":{"id":"prompt","role":"user","text":"Original prompt","runId":"run","inputIntent":"turn_start","streaming":false,"createdAt":"2026-11-13T12:00:00Z","updatedAt":"2026-11-13T12:00:00Z"}})).expect("canonical message");
    derive_messages_timeline_rows(&MessagesTimelineRowsInput {
        timeline_entries: vec![Rc::new(entry)],
        supports_conversation_rollback: true,
        thread_start_checkpoint_available: true,
        available_revert_turn_counts: Some([0].into()),
        ..Default::default()
    })
    .into_iter()
    .find(|row| RewindTarget::from_row(row).is_some())
    .expect("client rewindable user row")
}

fn control(id: &'static str, action: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .test_id(id)
        .child(text(id.to_owned()))
        .on_press(action)
}

fn fork_control(rewind: TimelineRewind) -> impl IntoElement {
    let owner = TaskOwner::here();
    control("fork", move || {
        if let Some(fork) = rewind.fork_action() {
            let answer = fork(ForkRequest {
                source_thread_id: IdOrIndex::Id(key("ancestor").thread),
                source_item_id: IdOrIndex::Id(TurnItemId("response".into())),
                run_id: IdOrIndex::Id(RunId("run".into())),
            });
            owner.spawn(async move {
                let _ = answer.await;
            });
        }
    })
}

#[component]
fn RewindFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    t3_ui::i18n::set_language("en", &[]);
    provide_context(t3_ui::theme::text_view_parts());
    provide_context(t3_ui::runtime::Rt::new(
        t3_client::ClientStore::default(),
        Rc::new(t3_ui::runtime::backend::Unavailable("fixture".into())),
        t3_ui::runtime::host::Choice::Demo,
    ));
    let nav = Navigator::new(Route::Thread(key("parent")));
    provide_context(nav);
    let client = Client {
        allowed: signal(true),
        files: signal(false),
        fail: signal(false),
        calls: signal(Vec::new()),
        forks: signal(0),
    };
    let calls = client.calls;
    let forks = client.forks;
    let allowed = client.allowed;
    let files = client.files;
    let fail = client.fail;
    provide_context(RewindService(Rc::new(client)));
    let services = TimelineServices::new(key("parent"));
    let rewind = services.rewind;
    let row = target_row();
    let target = RewindTarget::from_row(&row).expect("target");
    let projection = computed(move || Some(row.clone()));
    let user = UserRowContext::new(UserRowState::new(), &services);
    on_key("ctrl+d", move |_| allowed.set(false));
    on_key("ctrl+o", move |_| nav.open(Route::Thread(key("other"))));
    view()
        .w_full()
        .h_full()
        .child(user_message_row(projection, user))
        .child(control("deny", move || allowed.set(false)))
        .child(control("files", move || files.set(true)))
        .child(control("fail", move || fail.set(true)))
        .child(control("restore-files", move || {
            rewind.request_revert(target.clone())
        }))
        .child(fork_control(rewind))
        .child(text(move || {
            SharedString::from(format!("Calls: {}", calls.get().len()))
        }))
        .child(text(move || {
            SharedString::from(format!("Forks: {}", forks.get()))
        }))
        .child(text(move || {
            SharedString::from(format!("Busy: {}", rewind.reverting.get()))
        }))
        .child(text(move || SharedString::from(nav.top().path())))
        .child(text(move || {
            rewind
                .error
                .get()
                .map_or_else(SharedString::default, |error| error.message().resolve())
        }))
        .child(text(move || {
            SharedString::from(format!("Payload: {:?}", calls.get()))
        }))
        .child(text(move || {
            SharedString::from(format!("Restored: {:?}", rewind.restored_prompt.get()))
        }))
        .child(confirm_dialog_host())
}

#[component]
fn MissingServiceFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    t3_ui::i18n::set_language("en", &[]);
    provide_context(t3_ui::theme::text_view_parts());
    provide_context(t3_ui::runtime::Rt::new(
        t3_client::ClientStore::default(),
        Rc::new(t3_ui::runtime::backend::Unavailable("fixture".into())),
        t3_ui::runtime::host::Choice::Demo,
    ));
    let nav = Navigator::new(Route::Thread(key("parent")));
    provide_context(nav);
    let services = TimelineServices::new(key("parent"));
    let rewind = services.rewind;
    let row = target_row();
    let target = RewindTarget::from_row(&row).expect("target");
    let projection = computed(move || Some(row.clone()));
    let user = UserRowContext::new(UserRowState::new(), &services);
    view()
        .w_full()
        .h_full()
        .child(user_message_row(projection, user))
        .child(control("direct-rewind", move || {
            rewind.request_revert(target.clone())
        }))
        .child(fork_control(rewind))
        .child(text(move || SharedString::from(nav.top().path())))
        .child(text(move || {
            SharedString::from(format!("Busy: {}", rewind.reverting.get()))
        }))
        .child(confirm_dialog_host())
}

fn fixture(window: (f32, f32)) -> Headless {
    start(Root::of::<RewindFixture>(), window)
}

fn start(root: Root, window: (f32, f32)) -> Headless {
    let mut sets = t3_ui::registries();
    let mut extra = Registry::new();
    extra.register::<RewindFixture>();
    extra.register::<MissingServiceFixture>();
    sets.push(extra);
    let mut app = Headless::start(EngineConfig {
        ui_sets: sets,
        viewport: size(px(window.0), px(window.1)),
        environment: nsh_core::platform::Environment {
            os: if window == PHONE {
                nsh_core::platform::Os::Android
            } else {
                nsh_core::platform::Os::Windows
            },
            idiom: if window == PHONE {
                nsh_core::platform::Idiom::Phone
            } else {
                nsh_core::platform::Idiom::Desktop
            },
            reduce_motion: true,
            ..Default::default()
        },
        ..EngineConfig::new(root)
    })
    .expect("rewind fixture");
    app.wait_for("Original prompt").expect("user row");
    app
}

#[test]
fn absent_service_disables_the_user_action_and_direct_calls_do_nothing() {
    let mut app = start(Root::of::<MissingServiceFixture>(), DESKTOP);
    let _ = app.press("role:button[Edit from here]");
    app.press("id:direct-rewind").expect("direct request");
    app.press("id:fork").expect("fork request");
    assert!(app.screen().shows("Busy: false"));
    assert!(app.screen().shows("/environment/parent"));
    assert!(!app.screen().shows("Edit from here?"));
}

#[test]
fn cancel_keeps_the_thread_and_confirm_forwards_the_exact_user_target_once() {
    for size in [PHONE, DESKTOP] {
        let mut app = fixture(size);
        app.press("role:button[Edit from here]").expect("edit");
        app.wait_for("Edit from here?").expect("root host");
        assert!(app.screen().shows("Calls: 0"));
        app.press("role:button[Cancel]").expect("cancel");
        app.wait_for("Busy: false").expect("cancel settled");
        assert!(app.screen().shows("Calls: 0"));
        assert!(app.screen().shows("/environment/parent"));
        app.press("role:button[Edit from here]")
            .expect("edit again");
        app.wait_for("Edit from here?").expect("confirm again");
        app.press("role:button[Revert and keep changes]")
            .expect("confirm");
        app.wait_for("Calls: 1").expect("client called");
        app.wait_for("Busy: false").expect("completed");
        app.wait_for("Restored: Some(\"Restored prompt from client\")")
            .expect("client outcome reaches the editor binding");
        assert!(app.screen().shows("Payload: [(\"prompt\", 0, false)]"));
    }
}

#[test]
fn client_denials_and_unavailable_file_restoration_never_dispatch() {
    let mut app = fixture(DESKTOP);
    app.press("id:restore-files").expect("open confirmation");
    app.wait_for("Edit from here?").expect("confirmation");
    let _ = app.press("role:button[Revert files too]");
    assert!(app.screen().shows("Calls: 0"));
    app.press("role:button[Cancel]").expect("dismiss");
    app.wait_for_gone("Edit from here?", std::time::Duration::from_secs(5))
        .expect("dismissed");
    app.press("id:deny").expect("permission changed");
    app.press("role:button[Edit from here]")
        .expect("guarded action");
    app.wait_for("Interrupt the current turn before reverting checkpoints.")
        .expect("client reason");
    assert!(!app.screen().shows("Edit from here?"));
    assert!(app.screen().shows("Calls: 0"));
}

#[test]
fn confirmation_rechecks_availability_and_failures_release_the_busy_state() {
    let mut app = fixture(DESKTOP);
    app.press("role:button[Edit from here]").expect("edit");
    app.wait_for("Edit from here?").expect("confirm");
    app.press_key("ctrl+d")
        .expect("revoke client authorization while modal open");
    app.press("role:button[Revert and keep changes]")
        .expect("answer");
    app.wait_for("Busy: false").expect("guard resolved");
    assert!(app.screen().shows("Calls: 0"));
    let mut app = fixture(DESKTOP);
    app.press("id:fail").expect("client error");
    app.press("role:button[Edit from here]").expect("edit");
    app.wait_for("Edit from here?").expect("confirm");
    app.press("role:button[Revert and keep changes]")
        .expect("answer");
    app.wait_for("Failed to revert thread state.")
        .expect("error surfaced");
    app.wait_for("Busy: false").expect("error releases state");
    assert!(app.screen().shows("/environment/parent"));
}

#[test]
fn a_rewind_result_never_replaces_the_new_threads_visible_prompt() {
    let mut app = fixture(DESKTOP);
    app.press("role:button[Edit from here]").expect("edit");
    app.wait_for("Edit from here?").expect("confirm");
    app.press_key("ctrl+o")
        .expect("open another thread while deciding");
    app.press("role:button[Revert and keep changes]")
        .expect("confirm original target");
    app.wait_for("Calls: 1").expect("original thread operation");
    assert!(app.screen().shows("/environment/other"));
    assert!(app.screen().shows("Restored: None"));
    assert!(!app.screen().shows("Restored prompt from client"));
}

#[test]
fn edit_from_here_offers_worktree_restore_as_a_separate_choice() {
    for files in [false, true] {
        let mut app = fixture(DESKTOP);
        if files {
            app.press("id:files").expect("isolated ready checkpoint");
        }
        app.press("role:button[Edit from here]").expect("edit");
        app.wait_for("Edit from here?").expect("confirmation");
        app.wait_for("role:button[Revert files too]")
            .expect("separate file action");
        assert_eq!(
            app.screen()
                .enabled("role:button[Revert files too]")
                .expect("file action state"),
            files,
        );
        let choice = if files {
            "Revert files too"
        } else {
            "Revert and keep changes"
        };
        app.press(&format!("role:button[{choice}]"))
            .expect("confirm");
        app.wait_for("Calls: 1").expect("one rewind");
        assert!(
            app.screen()
                .shows(&format!("Payload: [(\"prompt\", 0, {files})]"))
        );
    }
}

#[test]
fn isolated_files_are_explicit_and_fork_opens_only_the_clients_ready_target() {
    let mut app = fixture(PHONE);
    app.press("id:files")
        .expect("client reports isolated ready checkpoint");
    app.press("id:restore-files").expect("request files");
    app.wait_for("Edit from here?").expect("confirm files");
    app.press("role:button[Revert files too]").expect("answer");
    app.wait_for("Calls: 1").expect("dispatch");
    assert!(app.screen().shows("Payload: [(\"prompt\", 0, true)]"));
    app.press("id:fork").expect("fork");
    app.wait_for("/environment/fork")
        .expect("client returned ready scoped thread");
    let mut app = fixture(DESKTOP);
    app.press("id:fail").expect("fork fails");
    app.press("id:fork").expect("fork");
    app.wait_for("Failed to fork this response.")
        .expect("error");
    assert!(app.screen().shows("/environment/parent"));
}

/// ChatView's row actions over a connected client. The item has no recorded provider
/// session, for which the web permits a completed response to be forked.
#[component]
fn ConnectedTimelineFixture() -> impl IntoElement {
    use t3_client::{
        ClientStore,
        composition::{ClientRuntime, Spawner},
        testing::environment,
    };
    use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
    use t3_ui::runtime::{Rt, host::Choice, services::thread_ops};

    t3_ui::theme::install(&Default::default());
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        })
    });
    let parent = key("parent");
    let server = environment::FakeEnvironment::new("environment", spawner.clone());
    server.set_shell(environment::shell(&[("parent", "Timeline actions")]));
    server.edit_config(|config| {
        let mut provider: serde_json::Value = serde_json::from_str(
            include_str!("../../../../fixtures/samples/ServerProvider.jsonl")
                .lines()
                .next()
                .expect("sample"),
        )
        .expect("provider");
        provider["instanceId"] = json!("codex");
        provider["supportsConversationRollback"] = json!(true);
        config["providers"] = json!([provider]);
    });
    let at = "2026-11-13T12:00:00Z";
    let prompt = json!({
        "id": "user-item", "type": "user_message", "threadId": "parent", "runId": "run",
        "nodeId": null, "providerThreadId": "pt", "providerTurnId": null,
        "nativeItemRef": null, "parentItemId": null, "ordinal": 1,
        "status": "completed", "title": null, "startedAt": at, "completedAt": at,
        "updatedAt": at, "messageId": "prompt", "text": "Original prompt",
        "inputIntent": "turn_start", "attachments": [], "createdBy": "user",
        "creationSource": "web",
    });
    let answer = json!({
        "id": "assistant-item", "type": "assistant_message", "threadId": "parent",
        "runId": "run", "nodeId": null, "providerThreadId": "pt",
        "providerTurnId": null, "nativeItemRef": null, "parentItemId": null,
        "ordinal": 2, "status": "completed", "title": null, "startedAt": at,
        "completedAt": at, "updatedAt": at, "messageId": "answer",
        "text": "A finished response", "streaming": false,
    });
    let mut projection = environment::projection(
        "parent",
        "Timeline actions",
        json!([]),
        json!([prompt, answer]),
    );
    projection["visibleTurnItems"] = json!([
        {"position":0,"visibility":"local","sourceThreadId":"parent","sourceItemId":"user-item","item":prompt},
        {"position":1,"visibility":"local","sourceThreadId":"parent","sourceItemId":"assistant-item","item":answer}
    ]);
    projection["messages"] = json!([{
        "id":"prompt", "threadId":"parent", "runId":"run", "nodeId":null, "role":"user",
        "text":"Original prompt", "attachments":[], "streaming":false, "createdBy":"user",
        "creationSource":"web", "createdAt":at, "updatedAt":at,
    }]);
    let mut active_session: serde_json::Value = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2ProviderSession.jsonl")
            .lines()
            .next()
            .expect("sample"),
    )
    .expect("session");
    active_session["id"] = json!("active-session");
    active_session["capabilities"]["threads"]["canForkThread"] = json!(false);
    active_session["capabilities"]["context"]["supportsFullThreadHandoff"] = json!(true);
    active_session["capabilities"]["checkpointing"]["providerCanRollbackConversation"] =
        json!(true);
    projection["providerSessions"] = json!([active_session]);
    projection["thread"]["activeProviderThreadId"] = json!("active-pt");
    projection["providerThreads"] = json!([{
        "id": "pt", "driver": "codex", "providerInstanceId": "codex",
        "providerSessionId": "session", "appThreadId": "parent", "ownerNodeId": null,
        "nativeThreadRef": null, "nativeConversationHeadRef": null, "status": "idle",
        "firstRunOrdinal": 1, "lastRunOrdinal": 1, "handoffIds": [], "forkedFrom": null,
        "createdAt": at, "updatedAt": at,
    }]);
    let mut active_thread = projection["providerThreads"][0].clone();
    active_thread["id"] = json!("active-pt");
    active_thread["providerSessionId"] = json!("active-session");
    projection["providerThreads"]
        .as_array_mut()
        .expect("provider threads")
        .push(active_thread);
    projection["checkpoints"] = json!([{
        "id": "root", "threadId": "parent", "scopeId": "scope", "runId": null,
        "nodeId": "root", "parentCheckpointId": null, "ordinalWithinScope": 0,
        "appRunOrdinal": null, "ref": "root", "status": "ready", "files": [],
        "capturedAt": at,
    }]);
    server.set_thread("parent", projection);
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        server.platform(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    provide_context(Rt::new(
        store.clone(),
        Rc::new(client.clone()),
        Choice::Demo,
    ));
    provide_context(Navigator::new(Route::Thread(parent.clone())));
    provide_context(thread_ops::timeline_rewind(&client));
    provide_context(thread_ops::approval_services(&client));
    let pair = client.pair(server.pairing());
    let open = parent.clone();
    spawner(
        async move {
            pair.await.expect("paired");
            client.open_thread(&open);
        }
        .boxed_local(),
    );
    let receipts = signal(String::new());
    let scope = parent.clone();
    let session_store = store.clone();
    let session_thread = parent.clone();
    let update_store = store.clone();
    let update_thread = parent.clone();
    on_key("ctrl+u", move |_| {
        let generation = update_store.snapshot().environments[&update_thread.environment].session;
        update_store.update_thread(&update_thread, generation, |thread| {
            let p = Rc::make_mut(thread.projection.as_mut().expect("loaded thread"));
            p.thread.updated_at = t3_contracts::wire::WireDateTime("2026-11-13T12:00:01Z".into());
            true
        });
    });
    view()
        .w_full()
        .h_full()
        .child(control("receipts", move || {
            let calls = server.calls("orchestration.dispatchCommand");
            receipts.set(serde_json::to_string(&calls).expect("dispatch receipts"));
        }))
        .child(text(move || receipts.get()))
        .child(control("record-session", move || {
            let mut s: serde_json::Value = serde_json::from_str(
                include_str!("../../../../fixtures/samples/OrchestrationV2ProviderSession.jsonl")
                    .lines()
                    .next()
                    .expect("sample"),
            )
            .expect("session");
            s["id"] = json!("session");
            s["capabilities"]["threads"]["canForkThread"] = json!(false);
            s["capabilities"]["context"]["supportsFullThreadHandoff"] = json!(false);
            let generation =
                session_store.snapshot().environments[&session_thread.environment].session;
            session_store.update_thread(&session_thread, generation, |thread| {
                let p = Rc::make_mut(thread.projection.as_mut().expect("loaded thread"));
                p.provider_sessions
                    .push(serde_json::from_value(s).expect("configured session"));
                true
            });
        }))
        .child(control("page-history", move || {
            let generation = store.snapshot().environments[&scope.environment].session;
            store.update_thread(&scope, generation, |thread| {
                thread.history.has_more = true;
                true
            });
        }))
        .child(t3_ui::screens::thread::timeline::timeline(parent))
        .child(confirm_dialog_host())
}

#[test]
fn completed_response_without_item_session_dispatches_a_fork_for_its_run() {
    let mut app = start(Root::of::<ConnectedTimelineFixture>(), DESKTOP);
    app.press("role:button[Fork from this response]")
        .expect("fork control");
    app.press("id:receipts").expect("read fake server");
    let screen = app.screen();
    let calls: Vec<serde_json::Value> = screen
        .texts()
        .iter()
        .find_map(|text| serde_json::from_str(text).ok())
        .expect("server receipts");
    assert_eq!(calls.len(), 1, "one dispatch reaches the server");
    let command = &calls[0];
    assert_eq!(command["type"], "thread.fork");
    assert_eq!(command["sourceThreadId"], "parent");
    assert_eq!(command["sourcePoint"], json!({"type":"run","runId":"run"}));
}

#[test]
fn edit_from_here_requires_a_checkpoint_at_the_real_start_of_history() {
    let mut app = start(Root::of::<ConnectedTimelineFixture>(), DESKTOP);
    let edit = "role:button[Edit from here]";
    app.wait_for(edit)
        .expect("first turn has an available root checkpoint");
    app.press("id:page-history")
        .expect("first loaded turn is no longer the thread start");
    assert!(!app.screen().matches(edit).expect("selector"));
}

#[test]
fn cancelling_edit_from_here_allows_a_fresh_confirmation_after_the_thread_changes() {
    let mut app = start(Root::of::<ConnectedTimelineFixture>(), DESKTOP);
    let edit = "role:button[Edit from here]";
    let title = "Edit from here?";
    app.wait_for(edit).expect("editable user message");
    app.press(edit).expect("preflight the current history");
    app.wait_for(title).expect("first confirmation");
    app.press_key("ctrl+u").expect("the thread changes");
    app.press("role:button[Cancel]").expect("cancel");
    app.wait_for_gone(title, std::time::Duration::from_secs(5))
        .expect("first confirmation closes");

    app.press(edit).expect("preflight the new history");
    app.wait_for(title).expect("a fresh confirmation opens");
}

#[test]
fn recorded_item_session_hides_fork_when_it_cannot_fork_or_hand_off() {
    let mut app = start(Root::of::<ConnectedTimelineFixture>(), DESKTOP);
    let fork = "role:button[Fork from this response]";
    app.wait_for(fork)
        .expect("completed response with unknown capabilities");
    app.press("id:record-session")
        .expect("provider capabilities arrive");
    assert!(!app.screen().matches(fork).expect("selector"));
}
