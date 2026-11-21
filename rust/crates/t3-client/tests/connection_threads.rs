//! Thread leases across the connection lifecycle, against an in-process server.
#![allow(clippy::unwrap_used)]

mod support {
    pub mod fake_server;
}

use std::{cell::RefCell, rc::Rc};
use support::fake_server::*;
use t3_client::{
    ClientStore,
    authorization::Sensitive,
    connection::{ConnectedClient, ServerCredential, connect_into},
    types::{SyncStatus, ThreadKey},
};

const ENV: &str = "env";

/// Set when the connection driver finished, with its result.
type Ended = Rc<RefCell<Option<Result<(), String>>>>;

/// Connects with a saved bearer and keeps the connection driver running on the harness.
fn connected(harness: &mut Harness) -> (ConnectedClient, Ended) {
    let platform = harness.platform.clone();
    let (client, driver) = harness
        .run(async move {
            connect_into(
                platform,
                ClientStore::default(),
                ORIGIN,
                ServerCredential::Bearer(Sensitive("bearer".into())),
            )
            .await
        })
        .unwrap();
    let ended = Rc::new(RefCell::new(None));
    let slot = ended.clone();
    harness.spawn(async move {
        *slot.borrow_mut() = Some(driver.await.map_err(|error| error.to_string()));
    });
    harness.settle();
    (client, ended)
}

fn key(id: &str) -> ThreadKey {
    ThreadKey {
        environment: environment(ENV),
        thread: thread(id),
    }
}

fn status(client: &ConnectedClient, id: &str) -> Option<SyncStatus> {
    let snapshot = client.store().snapshot();
    t3_client::state::selectors::thread(&snapshot, &key(id)).map(|thread| thread.status)
}

fn live_session(client: &ConnectedClient) -> u64 {
    client.store().snapshot().environments[&environment(ENV)]
        .session
        .0
}

#[test]
fn a_lease_follows_its_thread_and_dropping_it_ends_the_subscription() {
    let mut harness = Harness::new(ENV);
    let (client, _ended) = connected(&mut harness);
    let snapshot = client.store().snapshot();
    let shell = t3_client::state::selectors::shell(&snapshot, &environment(ENV)).unwrap();
    assert_eq!(shell.status, SyncStatus::Live);
    drop(snapshot);
    assert_eq!(harness.server().thread_subscribes("t1"), 0);

    let first = client.open_thread(thread("t1"));
    let second = client.open_thread(thread("t1"));
    harness.settle();
    assert_eq!(
        harness.server().thread_subscribes("t1"),
        1,
        "two leases, one follow"
    );
    assert_eq!(status(&client, "t1"), Some(SyncStatus::Live));
    assert_eq!(
        client
            .store()
            .shared_thread(&key("t1"))
            .unwrap()
            .visible_turn_items
            .len(),
        1
    );

    drop(first);
    harness.settle();
    assert_eq!(harness.server().open_thread_streams(), 1);
    drop(second);
    harness.settle();
    assert_eq!(harness.server().open_thread_streams(), 0);
    assert!(
        client.store().shared_thread(&key("t1")).is_some(),
        "the projection stays readable"
    );
}

#[test]
fn a_reconnect_follows_every_open_thread_again_on_the_new_session() {
    let mut harness = Harness::new(ENV);
    let (client, _ended) = connected(&mut harness);
    let _lease = client.open_thread(thread("t1"));
    harness.settle();
    let before = live_session(&client);
    assert_eq!(harness.server().thread_subscribes("t1"), 1);

    client.reconnect().unwrap();
    harness.settle();
    harness.advance(2_000);
    assert!(live_session(&client) > before, "a new session");
    assert_eq!(harness.server().thread_subscribes("t1"), 2);
    assert_eq!(
        harness.server().open_thread_streams(),
        1,
        "the old session's subscription closed"
    );
    assert_eq!(status(&client, "t1"), Some(SyncStatus::Live));

    // The server dropping the connection is the same: the next session follows again.
    harness.server().hang_up();
    harness.settle();
    harness.advance(10_000);
    assert_eq!(harness.server().thread_subscribes("t1"), 3);
    assert_eq!(harness.server().open_thread_streams(), 1);
}

#[test]
fn a_server_restart_refreshes_the_open_timeline_while_the_old_socket_closes() {
    let mut harness = Harness::new(ENV);
    harness.server().state.borrow_mut().hold_socket_close = true;
    let (client, _ended) = connected(&mut harness);
    let _lease = client.open_thread(thread("t1"));
    harness.settle();
    let before = live_session(&client);
    assert_eq!(status(&client, "t1"), Some(SyncStatus::Live));
    assert_eq!(harness.server().thread_subscribes("t1"), 1);

    let mut replacement = thread_snapshot("t1");
    replacement["snapshotSequence"] = serde_json::json!(2);
    let mut visible = replacement["projection"]["visibleTurnItems"][0].clone();
    visible["position"] = serde_json::json!(1);
    visible["sourceItemId"] = serde_json::json!("after-restart");
    visible["item"]["id"] = serde_json::json!("after-restart");
    visible["item"]["ordinal"] = serde_json::json!(2);
    replacement["projection"]["turnItems"]
        .as_array_mut()
        .unwrap()
        .push(visible["item"].clone());
    replacement["projection"]["visibleTurnItems"]
        .as_array_mut()
        .unwrap()
        .push(visible);
    {
        let mut server = harness.server().state.borrow_mut();
        server.hold_socket_close = false;
        server.thread_snapshots.insert("t1".into(), replacement);
    }
    harness.server().hang_up();
    harness.settle();
    harness.advance(10_000);

    assert!(
        live_session(&client) > before,
        "new socket despite the old close handshake"
    );
    assert_eq!(harness.server().thread_subscribes("t1"), 2);
    assert_eq!(harness.server().open_threads_of("t1"), 1);
    assert_eq!(status(&client, "t1"), Some(SyncStatus::Live));
    let timeline = client.store().shared_thread(&key("t1")).unwrap();
    assert_eq!(timeline.visible_turn_items.len(), 2);
}

#[test]
fn a_lease_dropped_during_a_reconnect_is_not_followed_on_the_new_session() {
    let mut harness = Harness::new(ENV);
    let (client, _ended) = connected(&mut harness);
    let kept = client.open_thread(thread("t1"));
    let dropped = client.open_thread(thread("t2"));
    harness.settle();
    assert_eq!(harness.server().thread_subscribes("t2"), 1);

    client.reconnect().unwrap();
    drop(dropped);
    harness.settle();
    harness.advance(2_000);
    assert_eq!(harness.server().thread_subscribes("t1"), 2);
    assert_eq!(
        harness.server().thread_subscribes("t2"),
        1,
        "not resurrected"
    );
    assert_eq!(harness.server().open_threads_of("t2"), 0);
    drop(kept);
}

#[test]
fn closing_the_client_ends_every_follow_and_leaves_nothing_open() {
    let mut harness = Harness::new(ENV);
    let (client, ended) = connected(&mut harness);
    let _one = client.open_thread(thread("t1"));
    let _two = client.open_thread(thread("t2"));
    harness.settle();
    assert_eq!(harness.server().open_thread_streams(), 2);

    client.close().unwrap();
    harness.settle();
    assert_eq!(*ended.borrow(), Some(Ok(())), "the driver finished");
    assert_eq!(harness.server().open_thread_streams(), 0);
    assert_eq!(harness.server().state.borrow().sockets_open, 0);
    // Leases outliving the connection follow nothing.
    let late = client.open_thread(thread("t3"));
    harness.settle();
    assert_eq!(harness.server().thread_subscribes("t3"), 0);
    drop(late);
}

#[test]
fn dropping_the_client_ends_every_follow() {
    let mut harness = Harness::new(ENV);
    let (client, ended) = connected(&mut harness);
    let lease = client.open_thread(thread("t1"));
    harness.settle();
    drop(client);
    harness.settle();
    assert!(ended.borrow().is_some(), "the driver finished");
    assert_eq!(harness.server().open_thread_streams(), 0);
    assert_eq!(harness.server().state.borrow().sockets_open, 0);
    drop(lease);
}

#[test]
fn coming_back_to_the_foreground_follows_open_threads_again() {
    // Review L5. TypeScript does not restart a cleanly ended thread stream on the same session
    // either, but resubscribes every open thread when the app becomes active again
    // (`shouldResubscribeAfterWakeup`). Here the supervisor's foreground wake emits `Resubscribe`,
    // which restarts every follow.
    let mut harness = Harness::new(ENV);
    harness.server().state.borrow_mut().thread_streams_end = true;
    let (client, _ended) = connected(&mut harness);
    let _lease = client.open_thread(t3_client::types::ThreadId("t1".into()));
    harness.settle();
    assert_eq!(harness.server().thread_subscribes("t1"), 1);
    assert_eq!(
        harness.server().open_threads_of("t1"),
        0,
        "the server ended the stream"
    );
    harness.advance(10_000);
    assert_eq!(
        harness.server().thread_subscribes("t1"),
        1,
        "an ended follow is not restarted in a loop"
    );
    harness.server().state.borrow_mut().thread_streams_end = false;
    harness
        .platform
        .fake
        .emit(t3_client::platform::PlatformEvent::Foreground);
    harness.settle();
    assert_eq!(harness.server().thread_subscribes("t1"), 2);
    assert_eq!(harness.server().open_threads_of("t1"), 1);
}
