//! The composition root against an in-process server: pairing, thread demand, removal, restore.
#![allow(clippy::unwrap_used)]

mod support {
    pub mod fake_server;
}

use support::fake_server::*;
use t3_client::{
    ClientStore,
    composition::{ClientRuntime, SendError},
    connection::{PairingInput, Phase},
    platform::{Storage, StorageArea},
    types::{SyncStatus, ThreadKey},
};
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;

const ENV: &str = "env";

fn runtime(harness: &Harness) -> ClientRuntime<TestPlatform> {
    ClientRuntime::new(
        harness.platform.clone(),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        harness.spawner(),
    )
}

fn pairing() -> PairingInput {
    PairingInput {
        pairing_url: Some(format!("{ORIGIN}/pair#token=pairing-token")),
        ..PairingInput::default()
    }
}

fn key(id: &str) -> ThreadKey {
    ThreadKey {
        environment: environment(ENV),
        thread: thread(id),
    }
}

fn paired(harness: &mut Harness) -> ClientRuntime<TestPlatform> {
    let client = runtime(harness);
    let pairing = client.pair(pairing());
    let id = harness.run(pairing).unwrap();
    assert_eq!(id, environment(ENV));
    harness.settle();
    client
}

#[test]
fn pairing_connects_lists_and_loads_the_shell_into_the_shared_store() {
    let mut harness = Harness::new(ENV);
    let client = runtime(&harness);
    let error = harness
        .run(client.pair(PairingInput::default()))
        .unwrap_err();
    assert_eq!(error.detail, "Enter a backend URL.");
    assert!(client.environments().is_empty());

    let id = harness.run(client.pair(pairing())).unwrap();
    harness.settle();
    let entries = client.environments();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, id);
    assert_eq!(entries[0].label, "Fake host");
    assert_eq!(entries[0].state.phase, Phase::Connected);
    let snapshot = client.store().snapshot();
    let shell = t3_client::state::selectors::shell(&snapshot, &id).unwrap();
    assert_eq!(shell.status, SyncStatus::Live);
    assert_eq!(client.store().shared_shell(&id).unwrap().threads.len(), 2);
}

#[test]
fn opens_are_counted_and_threads_opened_before_connecting_follow_once_connected() {
    let mut harness = Harness::new(ENV);
    let client = runtime(&harness);
    client.open_thread(&key("t1"));
    let id = harness.run(client.pair(pairing())).unwrap();
    assert_eq!(id, environment(ENV));
    harness.settle();
    assert_eq!(harness.server().thread_subscribes("t1"), 1);
    assert!(client.store().shared_thread(&key("t1")).is_some());

    client.open_thread(&key("t1"));
    client.close_thread(&key("t1"));
    harness.settle();
    assert_eq!(
        harness.server().open_threads_of("t1"),
        1,
        "one open remains"
    );
    client.close_thread(&key("t1"));
    harness.settle();
    assert_eq!(harness.server().open_threads_of("t1"), 0);
    // An unmatched close is harmless.
    client.close_thread(&key("t1"));
}

#[test]
fn removing_an_environment_ends_its_follows_and_forgets_its_data_and_credentials() {
    let mut harness = Harness::new(ENV);
    let client = paired(&mut harness);
    client.open_thread(&key("t1"));
    harness.settle();
    assert_eq!(harness.server().open_thread_streams(), 1);
    let platform = harness.platform.clone();
    let credentials = harness.run(async move {
        platform
            .list(StorageArea::Secure, "connection-credential".into())
            .await
            .unwrap()
    });
    assert_eq!(credentials.len(), 1);

    harness.run(client.remove(&environment(ENV))).unwrap();
    harness.settle();
    assert!(client.environments().is_empty());
    assert_eq!(harness.server().open_thread_streams(), 0);
    assert_eq!(harness.server().state.borrow().sockets_open, 0);
    assert!(client.store().snapshot().environments.is_empty());
    let platform = harness.platform.clone();
    let credentials = harness.run(async move {
        platform
            .list(StorageArea::Secure, "connection-credential".into())
            .await
            .unwrap()
    });
    assert!(credentials.is_empty(), "the saved bearer is gone");
    // Opening a thread of the removed environment follows nothing.
    client.open_thread(&key("t1"));
    harness.settle();
    assert_eq!(harness.server().thread_subscribes("t1"), 1);
}

#[test]
fn a_new_runtime_restores_saved_environments_with_their_credentials() {
    let mut harness = Harness::new(ENV);
    let client = paired(&mut harness);
    drop(client);
    harness.settle();
    assert_eq!(
        harness.server().state.borrow().sockets_open,
        0,
        "dropping closes"
    );

    let restored = runtime(&harness);
    harness.run(restored.restore()).unwrap();
    harness.settle();
    let entries = restored.environments();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].state.phase, Phase::Connected);
    assert_eq!(harness.server().state.borrow().sockets_open, 1);
    restored.open_thread(&key("t2"));
    harness.settle();
    assert_eq!(harness.server().open_threads_of("t2"), 1);
}

#[test]
fn messages_need_text_and_a_live_session() {
    let mut harness = Harness::new(ENV);
    let client = runtime(&harness);
    assert_eq!(
        harness.run(client.send_message(&key("t1"), "hello".into())),
        Err(SendError::Disconnected)
    );
    let client = paired(&mut harness);
    assert_eq!(
        harness.run(client.send_message(&key("t1"), "   ".into())),
        Err(SendError::Rejected("Enter a message.".into()))
    );
}

#[test]
fn a_query_shares_one_driver_across_leases_and_stops_with_the_last() {
    let mut harness = Harness::new(ENV);
    let client = paired(&mut harness);
    let first = client.turn_diff(&key("t1"), 0, 2, false);
    let second = client.turn_diff(&key("t1"), 0, 2, false);
    harness.settle();
    assert!(std::rc::Rc::ptr_eq(first.resource(), second.resource()));
    assert_eq!(harness.server().state.borrow().turn_diffs, 1, "one driver");
    let value = first.resource().snapshot().value.unwrap();
    assert_eq!(value.diff, "diff --git a/x b/x");
    assert_eq!(value.to_turn_count, 2);
    let other = client.turn_diff(&key("t1"), 1, 2, false);
    harness.settle();
    assert_eq!(
        harness.server().state.borrow().turn_diffs,
        2,
        "another input, another query"
    );
    drop((first, other));
    assert!(!second.resource().is_closed(), "one lease still holds it");
    let resource = second.resource().clone();
    drop(second);
    assert!(resource.is_closed(), "the last lease stops the driver");
}

#[test]
fn a_refused_message_stays_visible_with_the_servers_reason_until_dismissed() {
    use futures::{FutureExt, StreamExt};
    use t3_client::thread_views::composer::send::SendState;
    let mut harness = Harness::new(ENV);
    let client = paired(&mut harness);
    let mut changes = client.message_changes();
    assert_eq!(
        harness.run(client.send_message(&key("t1"), "hello".into())),
        Ok(())
    );
    harness.settle();
    assert!(
        changes.next().now_or_never().flatten().is_some(),
        "a change fired"
    );
    let pending = client.pending_messages(&key("t1"));
    assert_eq!(pending.len(), 1, "{pending:?}");
    let reason = "Failed to dispatch orchestration V2 command: service ProviderAdapterRegistry.get: No orchestration provider adapter is registered for codex.";
    assert!(
        matches!(&pending[0].state, SendState::Restored { reason: r, .. } if r == reason),
        "{:?}",
        pending[0].state
    );
    assert_eq!(client.draft_text(&key("t1")).as_deref(), Some("hello"));
    while changes.next().now_or_never().flatten().is_some() {}
    client.dismiss_message(&key("t1"), &pending[0].message.message_id);
    assert!(client.pending_messages(&key("t1")).is_empty());
    harness.settle();
    assert!(changes.next().now_or_never().flatten().is_some());
    assert_eq!(
        client.pending_messages(&key("t2")),
        vec![],
        "other threads see nothing"
    );
}
