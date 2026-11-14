//! The composition root against an in-process server: pairing, thread demand, removal, restore.
#![allow(clippy::unwrap_used)]

mod support {
    pub mod fake_server;
}

use support::fake_server::*;
use t3_client::{
    ClientStore,
    composition::{ClientRuntime, SendError},
    connection::{PairingInput, Phase, Time},
    environment::EnvironmentRegistry,
    platform::{NetworkStatus, Storage, StorageArea},
    rpc::SessionEndpoint,
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

fn saved(harness: &mut Harness, area: StorageArea, namespace: &str) -> usize {
    let platform = harness.platform.clone();
    let namespace = namespace.to_owned();
    harness.run(async move { platform.list(area, namespace).await.unwrap().len() })
}

fn token_exchanges(harness: &Harness) -> usize {
    let state = harness.server().state.borrow();
    state
        .http
        .iter()
        .filter(|path| *path == "/oauth/token")
        .count()
}

fn registry(harness: &mut Harness) -> EnvironmentRegistry<SessionEndpoint> {
    let platform = harness.platform.clone();
    harness.run(async move {
        EnvironmentRegistry::load(
            &platform,
            NetworkStatus::Unknown,
            Time::from_platform(&platform),
        )
        .await
        .unwrap()
    })
}

#[test]
fn pairing_returns_once_saved_while_the_socket_keeps_failing() {
    // Review H2: TypeScript saves the registration and connects in the background.
    let mut harness = Harness::new(ENV);
    harness.server().state.borrow_mut().refuse_sockets = true;
    let client = runtime(&harness);
    let id = harness.run(client.pair(pairing())).unwrap();
    assert_eq!(id, environment(ENV));
    harness.settle();
    let entries = client.environments();
    assert_eq!(entries.len(), 1, "listed while it keeps trying");
    assert_eq!(entries[0].label, "Fake host");
    assert!(
        matches!(entries[0].state.phase, Phase::Backoff | Phase::Connecting),
        "{:?}",
        entries[0].state
    );
    harness.server().state.borrow_mut().refuse_sockets = false;
    harness.advance(60_000);
    assert_eq!(client.environments()[0].state.phase, Phase::Connected);
}

#[test]
fn an_environment_unreachable_at_restore_keeps_retrying_and_shows_its_queued_messages() {
    // Review H2 and H3: restore waits for no server, and an offline start loses nothing.
    let mut harness = Harness::new(ENV);
    let client = paired(&mut harness);
    harness.server().state.borrow_mut().hold_dispatches = true;
    assert_eq!(
        harness.run(client.send_message(&key("t1"), "queued".into())),
        Ok(())
    );
    harness.settle();
    drop(client);
    harness.settle();
    harness.server().state.borrow_mut().unreachable = true;
    let restored = runtime(&harness);
    harness.run(restored.restore()).unwrap();
    harness.settle();
    let entries = restored.environments();
    assert_eq!(entries.len(), 1);
    assert_ne!(
        entries[0].state.phase,
        Phase::Blocked,
        "{:?}",
        entries[0].state
    );
    assert!(entries[0].state.desired);
    assert_eq!(
        restored.pending_messages(&key("t1")).len(),
        1,
        "its saved message is visible while it is offline"
    );
    harness.server().state.borrow_mut().unreachable = false;
    harness.advance(60_000);
    assert_eq!(restored.environments()[0].state.phase, Phase::Connected);
}

#[test]
fn pairing_a_connected_environment_again_keeps_the_new_connection_live() {
    // Review M1: the replaced runtime must not publish over the new one.
    let mut harness = Harness::new(ENV);
    let client = paired(&mut harness);
    // A screen still holds the first connection while the user pairs again.
    let first = client.client(&environment(ENV)).unwrap();
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let watcher = {
        use futures::StreamExt;
        let (client, seen) = (client.clone(), seen.clone());
        let mut changes = client.changes();
        async move {
            while changes.next().await.is_some() {
                if let Some(entry) = client.environments().first() {
                    seen.borrow_mut().push(entry.state.desired);
                }
            }
        }
    };
    harness.spawn(watcher);
    harness.run(client.pair(pairing())).unwrap();
    harness.settle();
    assert!(
        seen.borrow().iter().all(|desired| *desired),
        "the replaced runtime published over the new one: {:?}",
        seen.borrow()
    );
    let entries = client.environments();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].state.desired, "{:?}", entries[0].state);
    assert_eq!(entries[0].state.phase, Phase::Connected);
    assert_eq!(harness.server().state.borrow().sockets_open, 1);
    let connected = client.client(&environment(ENV)).unwrap();
    assert!(harness.run(async move { connected.ready().await }).is_ok());
    drop(first);
}

#[test]
fn removing_while_pairing_leaves_nothing_behind() {
    // Review M2: registry changes apply in the order they were asked for.
    let mut harness = Harness::new(ENV);
    let client = runtime(&harness);
    harness.server().state.borrow_mut().refuse_sockets = true;
    let pairing = client.pair(pairing());
    harness.spawn(async move {
        let _ = pairing.await;
    });
    harness.settle();
    harness.run(client.remove(&environment(ENV))).unwrap();
    harness.server().state.borrow_mut().refuse_sockets = false;
    harness.advance(60_000);
    assert!(client.environments().is_empty());
    assert_eq!(harness.server().state.borrow().sockets_open, 0);
    assert_eq!(
        saved(&mut harness, StorageArea::Secure, "connection-credential"),
        0
    );
}

#[test]
fn a_supervisor_that_stops_on_an_error_says_so_and_can_be_reconnected() {
    // Review M3.
    let mut harness = Harness::new(ENV);
    let client = paired(&mut harness);
    harness.server().state.borrow_mut().fail_random = true;
    harness.server().hang_up();
    harness.settle();
    let entries = client.environments();
    assert_eq!(
        entries[0].state.phase,
        Phase::Blocked,
        "{:?}",
        entries[0].state
    );
    assert!(entries[0].state.last_failure.is_some());
    harness.server().state.borrow_mut().fail_random = false;
    harness.run(client.reconnect(&environment(ENV))).unwrap();
    harness.advance(10_000);
    assert_eq!(client.environments()[0].state.phase, Phase::Connected);
}

#[test]
fn restore_leaves_a_disabled_environment_disconnected() {
    // Review M4: TypeScript respects disabled environments.
    let mut harness = Harness::new(ENV);
    drop(paired(&mut harness));
    harness.settle();
    let mut saved_registry = registry(&mut harness);
    let platform = harness.platform.clone();
    harness.run(async move {
        saved_registry
            .set_enabled_persisted(
                &platform,
                &environment(ENV),
                false,
                Time::from_platform(&platform),
            )
            .await
            .unwrap();
    });
    let opened = harness.server().state.borrow().sockets_opened;
    let restored = runtime(&harness);
    harness.run(restored.restore()).unwrap();
    harness.advance(10_000);
    let entries = restored.environments();
    assert_eq!(entries.len(), 1, "still listed");
    assert!(!entries[0].state.desired, "{:?}", entries[0].state);
    assert_ne!(entries[0].state.phase, Phase::Connected);
    assert_eq!(harness.server().state.borrow().sockets_opened, opened);
    let enabled = registry(&mut harness)
        .get(&environment(ENV))
        .unwrap()
        .registration()
        .enabled;
    assert!(!enabled, "restore did not re-enable it");
}

#[test]
fn pairing_checks_the_address_before_spending_the_code_and_saves_an_outdated_server() {
    // Review M5: prepare_pairing_registration, as TypeScript's onboarding.
    let mut harness = Harness::new(ENV);
    let client = runtime(&harness);
    let elsewhere = PairingInput {
        expected_environment: Some(environment("other")),
        ..pairing()
    };
    let error = harness.run(client.pair(elsewhere)).unwrap_err();
    assert!(
        error.detail.contains("a different machine"),
        "{}",
        error.detail
    );
    assert_eq!(token_exchanges(&harness), 0, "the code is still unspent");
    assert!(client.environments().is_empty());

    harness.run(client.pair(pairing())).unwrap();
    harness.settle();
    let impostor = PairingInput {
        pairing_url: Some("https://elsewhere.test/pair#token=second".into()),
        ..PairingInput::default()
    };
    let error = harness.run(client.pair(impostor)).unwrap_err();
    assert!(error.detail.contains("already saved"), "{}", error.detail);
    assert_eq!(token_exchanges(&harness), 1);

    let mut harness = Harness::new(ENV);
    harness.server().state.borrow_mut().outdated = true;
    let client = runtime(&harness);
    assert_eq!(
        harness.run(client.pair(pairing())),
        Ok(environment(ENV)),
        "saved so it can be updated from here"
    );
    harness.settle();
    let entries = client.environments();
    assert_eq!(entries.len(), 1);
    let failure = entries[0].state.last_failure.clone().unwrap();
    assert!(failure.server_update_required, "{failure:?}");

    let mut harness = Harness::new(ENV);
    harness.server().state.borrow_mut().anonymous = true;
    let client = runtime(&harness);
    assert_eq!(
        harness.run(client.pair(pairing())),
        Ok(environment(ENV)),
        "a host without a presentation still pairs"
    );
}

#[test]
fn removing_an_environment_forgets_its_queued_messages() {
    // Review M6: a later pairing of the same server must not deliver them unasked.
    let mut harness = Harness::new(ENV);
    let client = paired(&mut harness);
    harness.server().state.borrow_mut().hold_dispatches = true;
    assert_eq!(
        harness.run(client.send_message(&key("t1"), "queued".into())),
        Ok(())
    );
    harness.settle();
    assert_eq!(saved(&mut harness, StorageArea::Ordinary, "outbox"), 1);
    assert_eq!(
        saved(&mut harness, StorageArea::Ordinary, "operation-outbox"),
        1
    );
    harness.run(client.remove(&environment(ENV))).unwrap();
    harness.settle();
    assert_eq!(saved(&mut harness, StorageArea::Ordinary, "outbox"), 0);
    assert_eq!(
        saved(&mut harness, StorageArea::Ordinary, "operation-outbox"),
        0
    );
    harness.server().state.borrow_mut().hold_dispatches = false;
    harness.run(client.pair(pairing())).unwrap();
    harness.settle();
    assert!(client.pending_messages(&key("t1")).is_empty());
}
