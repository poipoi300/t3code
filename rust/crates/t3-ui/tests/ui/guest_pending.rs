//! GUEST-3: the owner's pending guest messages at the end of the real thread timeline: Send and
//! Dismiss per message, hidden without the operate scope, retryable after a failure.

use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::{cell::RefCell, rc::Rc};

use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::{Value, json};
use t3_client::ClientStore;
use t3_client::thread_views::thread_ops::guest_share::GuestMessageDecision;
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_contracts::{
    base_schemas::{RunId, TurnItemId},
    orchestration_v2::OrchestrationV2ThreadStreamItem,
};
use t3_ui::runtime::{Rt, backend::Unavailable, host::Choice};
use t3_ui::screens::thread::guest_pending::GuestMessageResolver;
use t3_ui::screens::thread::rewind::{
    RewindAvailability, RewindError, RewindService, RewindState, RewindTarget, TimelineRewindClient,
};
use t3_ui::state::nav::{Navigator, Route};

use crate::support::{DESKTOP, PHONE};

const AT: &str = "2026-11-14T12:00:00Z";

// The engine runs the UI on its own thread, so the scripted owner is process-wide and the tests
// that use it take turns.
static TURN: Mutex<()> = Mutex::new(());
static OPERATE: AtomicBool = AtomicBool::new(true);
static FAIL_ONCE: AtomicBool = AtomicBool::new(false);
static CALLS: Mutex<Vec<(String, GuestMessageDecision)>> = Mutex::new(Vec::new());

fn take_turn() -> std::sync::MutexGuard<'static, ()> {
    TURN.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn key() -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId("environment".into()),
        thread: ThreadId("shared".into()),
    }
}

struct Owner;

impl TimelineRewindClient for Owner {
    fn state(&self, _: &ThreadKey) -> RewindState {
        RewindState { reverting: false }
    }
    fn has_operate_scope(&self, _: &ThreadKey) -> bool {
        OPERATE.load(Ordering::SeqCst)
    }
    fn availability(&self, _: &ThreadKey, _: &RewindTarget) -> RewindAvailability {
        RewindAvailability::default()
    }
    fn changes(&self, _: &ThreadKey) -> LocalBoxStream<'static, ()> {
        futures::stream::pending().boxed_local()
    }
    fn rewind(
        &self,
        _: ThreadKey,
        _: RewindTarget,
        _: bool,
    ) -> LocalBoxFuture<'static, Result<String, RewindError>> {
        futures::future::pending().boxed_local()
    }
    fn fork(
        &self,
        _: ThreadKey,
        _: ThreadId,
        _: TurnItemId,
        _: RunId,
    ) -> LocalBoxFuture<'static, Result<ThreadKey, RewindError>> {
        futures::future::pending().boxed_local()
    }
}

fn snapshot(sequence: u64, messages: Value) -> OrchestrationV2ThreadStreamItem {
    let thread = json!({"id":"shared","projectId":"project","title":"Shared","providerInstanceId":"codex",
        "modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access",
        "interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":null,
        "lineage":{"rootThreadId":"shared","parentThreadId":null,"relationshipToParent":null},
        "forkedFrom":null,"createdBy":"user","creationSource":"web","createdAt":AT,"updatedAt":AT,
        "archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null,
        "guestShare":{"id":"share1","host":"box.example","createdAt":AT,
            "expiresAt":"2099-01-01T00:00:00.000Z","guestName":"Alex","messages":messages}});
    serde_json::from_value(json!({
        "kind":"snapshot","snapshotSequence":sequence,
        "projection":{"thread":thread,"runs":[],"attempts":[],"nodes":[],"subagents":[],
            "providerSessions":[],"providerThreads":[],"providerTurns":[],"runtimeRequests":[],
            "messages":[],"plans":[],"turnItems":[],"checkpointScopes":[],"checkpoints":[],
            "contextHandoffs":[],"contextTransfers":[],"visibleTurnItems":[],"updatedAt":AT}
    }))
    .expect("canonical thread snapshot")
}

fn message(id: &str, text: &str, status: &str) -> Value {
    json!({"id":id,"text":text,"status":status,"submittedAt":AT})
}

#[component]
fn Fixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let store = ClientStore::default();
    let session = store.begin_session(key().environment);
    let sequence = Rc::new(RefCell::new(1u64));
    store
        .apply_thread(
            key(),
            session,
            vec![snapshot(
                1,
                json!([
                    message("gm1", "Can you also run the tests?", "pending"),
                    message("gm2", "An older note", "dismissed"),
                ]),
            )],
        )
        .expect("seeded thread");
    // The server's answer: the resolved message leaves the pending list.
    let projection = store.clone();
    provide_context(GuestMessageResolver(Rc::new(move |_, id, decision| {
        CALLS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((id.0.clone(), decision));
        if FAIL_ONCE.swap(false, Ordering::SeqCst) {
            return futures::future::ready(Err("The server refused.".to_owned())).boxed_local();
        }
        *sequence.borrow_mut() += 1;
        let status = match decision {
            GuestMessageDecision::Send => "sent",
            GuestMessageDecision::Dismiss => "dismissed",
        };
        projection
            .apply_thread(
                key(),
                session,
                vec![snapshot(
                    *sequence.borrow(),
                    json!([message(&id.0, "Can you also run the tests?", status)]),
                )],
            )
            .expect("answered");
        futures::future::ready(Ok(())).boxed_local()
    })));
    provide_context(RewindService(Rc::new(Owner)));
    provide_context(Rt::new(
        store,
        Rc::new(Unavailable("fixture".into())),
        Choice::Demo,
    ));
    provide_context(Navigator::new(Route::Thread(key())));
    view()
        .w_full()
        .h_full()
        .child(t3_ui::ui::toast::toast_provider(
            view()
                .w_full()
                .h_full()
                .child(t3_ui::screens::thread::timeline::timeline(key())),
            Default::default(),
        ))
}

fn start(window: (f32, f32), operate: bool, fail_once: bool) -> Headless {
    OPERATE.store(operate, Ordering::SeqCst);
    FAIL_ONCE.store(fail_once, Ordering::SeqCst);
    CALLS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    let mut sets = t3_ui::registries();
    let mut fixtures = Registry::new();
    fixtures.register::<Fixture>();
    sets.push(fixtures);
    let mut config = EngineConfig {
        ui_sets: sets,
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<Fixture>())
    };
    if window == DESKTOP {
        config.environment.os = nsh_core::platform::Os::Linux;
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    }
    Headless::start(config).expect("fixture")
}

fn calls() -> Vec<(String, GuestMessageDecision)> {
    CALLS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

fn send_and_dismiss(window: (f32, f32)) {
    let _turn = take_turn();
    let mut app = start(window, true, false);
    app.wait_for("Can you also run the tests?")
        .expect("the pending message");
    assert!(app.screen().shows("Alex"), "the guest's name is on it");
    assert!(
        !app.screen().shows("An older note"),
        "a resolved message is not pending"
    );
    app.press("role:button[Send message from Alex]")
        .expect("send");
    app.wait_for_gone(
        "Can you also run the tests?",
        std::time::Duration::from_secs(5),
    )
    .expect("answered messages leave the list");
    assert_eq!(
        calls(),
        vec![("gm1".to_owned(), GuestMessageDecision::Send)]
    );
}

#[test]
fn the_owner_sends_a_pending_guest_message_at_both_widths() {
    send_and_dismiss(DESKTOP);
    send_and_dismiss(PHONE);
}

#[test]
fn the_owner_dismisses_a_pending_guest_message() {
    let _turn = take_turn();
    let mut app = start(DESKTOP, true, false);
    app.wait_for("Can you also run the tests?")
        .expect("pending");
    app.press("role:button[Dismiss message from Alex]")
        .expect("dismiss");
    app.wait_for_gone(
        "Can you also run the tests?",
        std::time::Duration::from_secs(5),
    )
    .expect("answered");
    assert_eq!(
        calls(),
        vec![("gm1".to_owned(), GuestMessageDecision::Dismiss)]
    );
}

#[test]
fn without_the_operate_scope_messages_show_but_cannot_be_resolved() {
    let _turn = take_turn();
    let mut app = start(DESKTOP, false, false);
    app.wait_for("Can you also run the tests?")
        .expect("pending");
    assert!(!app.screen().shows("Send"));
    assert!(!app.screen().shows("Dismiss"));
}

#[test]
fn a_refused_resolution_leaves_the_message_to_try_again() {
    let _turn = take_turn();
    let mut app = start(DESKTOP, true, true);
    app.wait_for("Can you also run the tests?")
        .expect("pending");
    app.press("role:button[Send message from Alex]")
        .expect("send");
    app.wait_for("The server refused.").expect("the refusal");
    assert!(app.screen().shows("Can you also run the tests?"));
    app.press("role:button[Send message from Alex]")
        .expect("retry");
    app.wait_for_gone(
        "Can you also run the tests?",
        std::time::Duration::from_secs(5),
    )
    .expect("the retry is answered");
    assert_eq!(calls().len(), 2);
}
