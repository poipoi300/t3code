//! The composition root against an in-process server: pairing, thread demand, removal, restore.
#![allow(clippy::unwrap_used)]

mod support {
    pub mod fake_server;
}

use support::fake_server::*;
use t3_client::{
    ClientStore,
    composition::{ClientRuntime, RelayConfig, SendError, SshPairingInput},
    connection::{ConnectionMode, PairingInput, Phase, Time},
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

fn relay_runtime(harness: &Harness) -> ClientRuntime<TestPlatform> {
    ClientRuntime::with_relay(
        harness.platform.clone(),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        harness.spawner(),
        RelayConfig {
            relay_url: RELAY.into(),
            client_id: t3_contracts::relay::RelayPublicClientId::T3Web,
        },
    )
}

fn route_modes(client: &ClientRuntime<TestPlatform>) -> Vec<ConnectionMode> {
    client.environments()[0]
        .routes
        .iter()
        .map(|route| route.mode.clone())
        .collect()
}

#[test]
fn a_linked_environment_is_discovered_through_the_relay_and_connects_with_a_minted_credential() {
    // C1: relay discovery, relay registration and the DPoP connect path, against a fake relay.
    let mut harness = Harness::new(ENV);
    harness.server().state.borrow_mut().signed_in = true;
    let client = relay_runtime(&harness);
    harness.run(client.restore()).unwrap();
    harness.settle();
    let discovered = client.relay_environments().unwrap();
    assert!(discovered.listed, "{discovered:?}");
    let linked = &discovered.environments[&environment(ENV)];
    assert_eq!(linked.environment.label.0, "Fake host");
    assert_eq!(
        linked.availability,
        t3_client::relay::RelayEnvironmentAvailability::Online,
        "{:?} {:?}",
        linked.error,
        harness.server().state.borrow().relay
    );

    harness
        .run(client.add_relay_environment(&environment(ENV), "Fake host"))
        .unwrap();
    harness.advance(5_000);
    assert_eq!(route_modes(&client), [ConnectionMode::Relay]);
    assert_eq!(
        client.environments()[0].state.phase,
        Phase::Connected,
        "{:?}",
        client.environments()[0].state
    );
    let relay = harness.server().state.borrow().relay.clone();
    assert!(
        relay.contains(&"POST /v1/client/dpop-token".into()),
        "{relay:?}"
    );
    assert!(
        relay.contains(&"POST /v1/environments/env/connect".into()),
        "{relay:?}"
    );
    assert_eq!(
        saved(&mut harness, StorageArea::Secure, "connection-credential"),
        0,
        "a relay route stores no credential; each connection is minted"
    );
    assert!(
        client.diagnosis(&environment(ENV), None).is_none(),
        "connected"
    );

    // The route is saved: a new run connects through the relay again.
    drop(client);
    harness.settle();
    let restored = relay_runtime(&harness);
    harness.run(restored.restore()).unwrap();
    harness.advance(5_000);
    assert_eq!(route_modes(&restored), [ConnectionMode::Relay]);
    assert_eq!(restored.environments()[0].state.phase, Phase::Connected);
}

#[test]
fn a_relay_that_will_not_connect_is_diagnosed_on_the_relay_path() {
    let mut harness = Harness::new(ENV);
    {
        let mut state = harness.server().state.borrow_mut();
        state.signed_in = true;
        state.relay_refuses = true;
    }
    let client = relay_runtime(&harness);
    harness.run(client.restore()).unwrap();
    harness
        .run(client.add_relay_environment(&environment(ENV), "Fake host"))
        .unwrap();
    harness.advance(5_000);
    let entry = &client.environments()[0];
    assert_ne!(entry.state.phase, Phase::Connected, "{:?}", entry.state);
    let diagnosis = client.diagnosis(&environment(ENV), Some("1.0.0")).unwrap();
    let failure = client.environments()[0].state.last_failure.clone().unwrap();
    assert_eq!(
        failure.kind,
        t3_client::connection::FailureKind::EndpointUnavailable,
        "{failure:?}"
    );
    assert_eq!(failure.trace_id.as_deref(), Some("relay-trace"));
    assert!(
        diagnosis
            .hops
            .iter()
            .any(|hop| hop.hop == t3_client::connection::ConnectionHop::Relay),
        "{diagnosis:?}"
    );
    assert!(
        diagnosis.headline.contains("Fake host"),
        "{}",
        diagnosis.headline
    );

    harness.server().state.borrow_mut().relay_refuses = false;
    harness.advance(120_000);
    assert_eq!(client.environments()[0].state.phase, Phase::Connected);
}

#[test]
fn signing_out_of_t3_connect_drops_relay_routes_and_keeps_direct_ones() {
    // TypeScript's removeRelayEnvironments: a relay-only environment goes; one with a direct
    // route keeps it and reconnects over it.
    let mut harness = Harness::new(ENV);
    harness.server().state.borrow_mut().signed_in = true;
    let client = relay_runtime(&harness);
    harness.run(client.pair(pairing())).unwrap();
    harness
        .run(client.add_relay_environment(&environment(ENV), "Fake host"))
        .unwrap();
    harness.advance(5_000);
    assert_eq!(
        route_modes(&client),
        [ConnectionMode::Remote, ConnectionMode::Relay]
    );
    harness.run(client.remove_relay_environments()).unwrap();
    harness.advance(5_000);
    assert_eq!(route_modes(&client), [ConnectionMode::Remote]);
    assert_eq!(client.environments()[0].state.phase, Phase::Connected);

    let mut harness = Harness::new(ENV);
    harness.server().state.borrow_mut().signed_in = true;
    let client = relay_runtime(&harness);
    harness
        .run(client.add_relay_environment(&environment(ENV), "Fake host"))
        .unwrap();
    harness.advance(5_000);
    harness.run(client.remove_relay_environments()).unwrap();
    harness.settle();
    assert!(client.environments().is_empty());
    assert_eq!(harness.server().state.borrow().sockets_open, 0);
}

#[test]
fn t3_connect_needs_a_relay() {
    let mut harness = Harness::new(ENV);
    let client = runtime(&harness);
    assert!(matches!(
        harness.run(client.add_relay_environment(&environment(ENV), "Fake host")),
        Err(t3_client::ClientError::Unsupported(_))
    ));
    assert!(client.relay_environments().is_none());
}

#[test]
fn an_ssh_host_is_launched_by_the_desktop_reconnected_through_its_tunnel_and_torn_down_on_removal()
{
    let mut harness = Harness::new(ENV);
    let client = runtime(&harness);
    let id = harness
        .run(client.pair_ssh(SshPairingInput {
            target: serde_json::from_value(ssh_target("box")).unwrap(),
            label: None,
            expected_environment: None,
        }))
        .unwrap();
    assert_eq!(id, environment(ENV));
    harness.advance(5_000);
    assert_eq!(route_modes(&client), [ConnectionMode::Ssh]);
    assert_eq!(
        client.environments()[0].label,
        "box",
        "the alias when the host names nothing"
    );
    assert_eq!(client.environments()[0].state.phase, Phase::Connected);
    assert_eq!(
        harness.server().state.borrow().ssh,
        ["provision box", "prepare box"]
    );

    harness.run(client.remove(&environment(ENV))).unwrap();
    harness.settle();
    assert_eq!(
        harness
            .server()
            .state
            .borrow()
            .ssh
            .last()
            .map(String::as_str),
        Some("disconnect box"),
        "the managed backend and tunnel outlive the session, so removal stops them"
    );
    assert_eq!(
        saved(&mut harness, StorageArea::Secure, "connection-credential"),
        0
    );

    let elsewhere = harness.run(client.pair_ssh(SshPairingInput {
        target: serde_json::from_value(ssh_target("box")).unwrap(),
        label: None,
        expected_environment: Some(environment("other")),
    }));
    assert!(
        elsewhere.is_err(),
        "an SSH host that is a different machine is refused"
    );
}

#[test]
fn an_open_command_row_streams_its_output_and_a_settled_one_is_kept_for_five_minutes() {
    // C6: TypeScript's commandOutput subscription rules (idle TTL 0, completed TTL 5 min).
    let mut harness = Harness::new(ENV);
    let client = paired(&mut harness);
    assert!(client.command_output_streaming(&environment(ENV)));
    let item = t3_contracts::base_schemas::TurnItemId("cmd-1".into());
    harness.server().state.borrow_mut().command_running = true;
    let running = client.command_output(&key("t1"), &item);
    let second = client.command_output(&key("t1"), &item);
    harness.settle();
    assert_eq!(running.text().expect("the first frame"), "install\n12%");
    assert!(running.read(None).unwrap().running);
    assert_eq!(
        harness.server().state.borrow().command_output_subscribes,
        1,
        "readers of one row share one subscription"
    );
    drop((running, second));
    harness.settle();
    assert_eq!(
        harness.server().state.borrow().command_outputs_open,
        0,
        "a closed row receives nothing"
    );

    harness.server().state.borrow_mut().command_running = false;
    let settled = client.command_output(&key("t1"), &item);
    harness.settle();
    assert_eq!(settled.text().unwrap(), "install\n100%\n");
    assert!(!settled.read(None).unwrap().running);
    assert_eq!(
        harness.server().state.borrow().command_outputs_open,
        0,
        "the subscription ends once the command settles"
    );
    drop(settled);
    let reopened = client.command_output(&key("t1"), &item);
    assert_eq!(reopened.text().unwrap(), "install\n100%\n");
    drop(reopened);
    assert_eq!(harness.server().state.borrow().command_output_subscribes, 2);

    harness.advance(5 * 60_000 + 1_000);
    let expired = client.command_output(&key("t1"), &item);
    assert!(
        expired.read(None).is_none(),
        "loading again after five minutes"
    );
    harness.settle();
    assert!(expired.read(None).is_some());
    assert_eq!(harness.server().state.borrow().command_output_subscribes, 3);
}

/// A runtime restored over storage damaged after `paired` saved one environment.
fn restored_after(
    harness: &mut Harness,
    damage: impl FnOnce(&mut Harness),
) -> ClientRuntime<TestPlatform> {
    drop(paired(harness));
    harness.settle();
    damage(harness);
    let restored = runtime(harness);
    assert!(
        harness.run(restored.restore()).is_ok(),
        "restore carries on past a damaged record"
    );
    harness.settle();
    restored
}

/// After damage, the environment can still be removed and a server paired again.
fn still_removable_and_pairable(harness: &mut Harness, client: &ClientRuntime<TestPlatform>) {
    for entry in client.environments() {
        harness.run(client.remove(&entry.id)).unwrap();
    }
    harness.settle();
    assert!(client.environments().is_empty());
    harness.server().state.borrow_mut().undecryptable = None;
    assert_eq!(harness.run(client.pair(pairing())), Ok(environment(ENV)));
    harness.advance(5_000);
    assert_eq!(client.environments()[0].state.phase, Phase::Connected);
}

#[test]
fn an_undecryptable_credential_does_not_stop_restore_removal_or_pairing() {
    // Platform review H1: TypeScript quarantines a corrupt catalog and carries on.
    let mut harness = Harness::new(ENV);
    let client = restored_after(&mut harness, |harness| {
        harness.server().state.borrow_mut().undecryptable = Some("connection-credential".into());
    });
    let entries = client.environments();
    assert_eq!(entries.len(), 1, "still listed, so it can be removed");
    harness.advance(5_000);
    assert_ne!(client.environments()[0].state.phase, Phase::Connected);
    still_removable_and_pairable(&mut harness, &client);
}

#[test]
fn a_credential_of_an_unknown_kind_does_not_stop_restore_removal_or_pairing() {
    // A downgrade: an older build reads a credential `_tag` a newer one wrote.
    let mut harness = Harness::new(ENV);
    let client = restored_after(&mut harness, |harness| {
        let platform = harness.platform.clone();
        harness.run(async move {
            let keys = platform
                .list(StorageArea::Secure, "connection-credential".into())
                .await
                .unwrap();
            let id = keys[0].record.clone();
            let record = t3_client::persistence::Record::new(
                "connection-credential",
                None,
                &id,
                1,
                serde_json::json!({"connectionId": id, "credential": {"_tag": "FutureConnectionCredential", "secret": "x"}}),
            );
            platform
                .write(
                    StorageArea::Secure,
                    vec![t3_client::platform::StorageWrite {
                        key: keys[0].clone(),
                        value: Some(record.encode().unwrap()),
                    }],
                )
                .await
                .unwrap();
        });
    });
    assert_eq!(client.environments().len(), 1);
    still_removable_and_pairable(&mut harness, &client);
}

#[test]
fn an_unreadable_catalog_is_quarantined_and_pairing_starts_afresh() {
    let mut harness = Harness::new(ENV);
    let client = restored_after(&mut harness, |harness| {
        let platform = harness.platform.clone();
        harness.run(async move {
            platform
                .write(
                    StorageArea::Ordinary,
                    vec![t3_client::platform::StorageWrite {
                        key: t3_client::platform::StorageKey::new(
                            "connection-catalog",
                            None,
                            "current",
                        ),
                        value: Some(b"{not a catalog".to_vec()),
                    }],
                )
                .await
                .unwrap();
        });
    });
    assert!(client.environments().is_empty());
    assert_eq!(
        saved(
            &mut harness,
            StorageArea::Ordinary,
            "connection-catalog-quarantine"
        ),
        1,
        "the unreadable catalog is kept aside, not discarded"
    );
    // Second re-review B1: a crash before the UI showed the notice must not lose it, though the
    // next launch finds nothing wrong itself.
    let relaunched = runtime(&harness);
    harness.run(relaunched.restore()).unwrap();
    harness.settle();
    let notice = relaunched.take_storage_notice().expect("a notice");
    assert!(notice.reset_catalog);
    assert_eq!(relaunched.take_storage_notice(), None);
    harness.settle();
    // Re-review N8: the user is told once.
    let after = runtime(&harness);
    harness.run(after.restore()).unwrap();
    harness.settle();
    assert_eq!(after.take_storage_notice(), None);
    // Re-review N5: nothing can reach these once the catalog is empty, so they go with it.
    for namespace in [
        "connection-credential",
        "connection-profile",
        "authorization.remote-dpop",
    ] {
        assert_eq!(
            saved(&mut harness, StorageArea::Secure, namespace),
            0,
            "{namespace}"
        );
    }
    still_removable_and_pairable(&mut harness, &client);
}

#[test]
fn a_catalog_target_this_build_does_not_know_is_kept_once_with_its_secrets() {
    // Re-review N4: each registry load wrote another copy, and the next save deleted the
    // unknown target's rows, which the copy could then no longer bring back.
    let mut harness = Harness::new(ENV);
    let future_rows = [
        t3_client::platform::StorageKey::new("connection-profile", None, "future-connection"),
        t3_client::platform::StorageKey::new("connection-credential", None, "future-connection"),
        t3_client::persistence::remote_dpop_token_key(&environment("future")),
    ];
    let rows = future_rows.clone();
    let client = restored_after(&mut harness, move |harness| {
        let platform = harness.platform.clone();
        harness.run(async move {
            let key = t3_client::platform::StorageKey::new("connection-catalog", None, "current");
            let bytes = platform
                .get(StorageArea::Ordinary, key.clone())
                .await
                .unwrap()
                .unwrap();
            let mut catalog =
                t3_client::persistence::decode_record::<serde_json::Value>(&bytes, &key, 1)
                    .unwrap()
                    .payload;
            catalog["targets"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({
                    "_tag": "FutureConnectionTarget",
                    "environmentId": "future",
                    "label": "Future",
                    "connectionId": "future-connection",
                }));
            let record = t3_client::persistence::Record::new(
                "connection-catalog",
                None,
                "current",
                1,
                catalog,
            );
            platform
                .write(
                    StorageArea::Ordinary,
                    vec![t3_client::platform::StorageWrite {
                        key,
                        value: Some(record.encode().unwrap()),
                    }],
                )
                .await
                .unwrap();
            platform
                .write(
                    StorageArea::Secure,
                    rows.into_iter()
                        .map(|key| t3_client::platform::StorageWrite {
                            key,
                            value: Some(b"written by a newer build".to_vec()),
                        })
                        .collect(),
                )
                .await
                .unwrap();
        });
    });
    assert_eq!(client.environments().len(), 1);
    // Later launches and loads see the same catalog.
    harness.advance(1_000);
    let again = runtime(&harness);
    harness.run(again.restore()).unwrap();
    harness.settle();
    assert_eq!(
        saved(
            &mut harness,
            StorageArea::Ordinary,
            "connection-catalog-quarantine"
        ),
        1,
        "one copy of one catalog"
    );
    // A save (removing the known environment) leaves the unknown target's rows alone.
    harness.run(again.remove(&environment(ENV))).unwrap();
    harness.settle();
    let platform = harness.platform.clone();
    let kept = harness.run(async move {
        let mut kept = 0;
        for key in future_rows {
            kept += usize::from(
                platform
                    .get(StorageArea::Secure, key)
                    .await
                    .unwrap()
                    .is_some(),
            );
        }
        kept
    });
    assert_eq!(kept, 3);
}

#[test]
fn removing_an_environment_leaves_no_copy_of_its_token_in_storage() {
    // Platform review M2: a quarantined import copy held the token after removal.
    use base64::Engine;
    let mut harness = Harness::new(ENV);
    let client = paired(&mut harness);
    let platform = harness.platform.clone();
    harness.run(async move {
        let raw = serde_json::json!({"credentials":[{"credential":{"token":"paired-bearer"}}]});
        let record = t3_client::persistence::Record::new(
            "quarantine",
            None,
            "legacy-catalog",
            1,
            serde_json::json!({"bytesBase64": base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&raw).unwrap())}),
        );
        platform
            .write(
                StorageArea::Secure,
                vec![t3_client::platform::StorageWrite {
                    key: t3_client::platform::StorageKey::new("quarantine", None, "legacy-catalog"),
                    value: Some(record.encode().unwrap()),
                }],
            )
            .await
            .unwrap();
    });
    harness.run(client.remove(&environment(ENV))).unwrap();
    harness.settle();
    let platform = harness.platform.clone();
    let copies = harness.run(async move {
        let mut copies = Vec::new();
        for area in [StorageArea::Ordinary, StorageArea::Secure] {
            for namespace in [
                "connection-credential",
                "connection-profile",
                "connection-catalog",
                "authorization.remote-dpop",
                "import-backup",
                "quarantine",
                "import-conflict",
                "connection-catalog-quarantine",
            ] {
                for key in platform.list(area, namespace.into()).await.unwrap() {
                    let bytes = platform
                        .get(area, key.clone())
                        .await
                        .unwrap()
                        .unwrap_or_default();
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    let decoded = serde_json::from_slice::<serde_json::Value>(&bytes)
                        .ok()
                        .and_then(|value| {
                            value["payload"]["bytesBase64"].as_str().map(str::to_owned)
                        })
                        .and_then(|value| {
                            base64::engine::general_purpose::STANDARD.decode(value).ok()
                        })
                        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                        .unwrap_or_default();
                    if text.contains("paired-bearer") || decoded.contains("paired-bearer") {
                        copies.push(key);
                    }
                }
            }
        }
        copies
    });
    assert!(copies.is_empty(), "{copies:?}");
}

#[test]
fn one_unreadable_profile_does_not_stop_saving_the_other_environments() {
    // Re-review N2: every save read every profile with `?`, so one undecryptable profile made
    // pairing, toggling and removing any other environment fail.
    let mut harness = Harness::new(ENV);
    drop(paired(&mut harness));
    harness.settle();
    // A second saved environment, whose profile alone becomes unreadable.
    let platform = harness.platform.clone();
    harness.run(async move {
        let mut registry: EnvironmentRegistry<SessionEndpoint> = EnvironmentRegistry::load(
            &platform,
            NetworkStatus::Unknown,
            Time::from_platform(&platform),
        )
        .await
        .unwrap();
        let other = environment("other");
        let id = "bearer:other:https://other.test".to_owned();
        let prepared = t3_client::connection::PreparedRegistration {
            registration: t3_client::environment::EnvironmentRegistration {
                id: other.clone(),
                label: "Other host".into(),
                routes: vec![t3_client::connection::Route {
                    id: id.clone(),
                    environment: other.clone(),
                    mode: ConnectionMode::Remote,
                    http_base: "https://other.test".into(),
                    ws_base: "wss://other.test".into(),
                }],
                enabled: true,
            },
            profile: t3_client::connection::ConnectionProfile::Bearer {
                connection_id: id,
                environment_id: other,
                label: "Other host".into(),
                http_base_url: "https://other.test".into(),
                ws_base_url: "wss://other.test".into(),
                learned: None,
                authorization: None,
                network: None,
            },
            credential: t3_client::connection::ConnectionCredential::Bearer {
                token: t3_client::authorization::Sensitive("other-bearer".into()),
            },
        };
        registry
            .register_prepared(
                &platform,
                prepared,
                false,
                true,
                NetworkStatus::Unknown,
                Time::from_platform(&platform),
            )
            .await
            .unwrap();
    });
    {
        let mut state = harness.server().state.borrow_mut();
        state.undecryptable = Some("connection-profile".into());
        state.undecryptable_record = Some("other".into());
    }
    let client = runtime(&harness);
    harness.run(client.restore()).unwrap();
    harness.settle();
    let entries = client.environments();
    assert_eq!(entries.len(), 2);
    let damaged: Vec<_> = entries.iter().filter(|entry| entry.damaged).collect();
    assert_eq!(damaged.len(), 1, "the user can tell which one");
    assert_eq!(damaged[0].id, environment("other"));
    assert_eq!(
        client.take_storage_notice().map(|notice| notice.damaged),
        Some(vec![environment("other")])
    );

    // Pairing the healthy server again saves every entry.
    assert_eq!(harness.run(client.pair(pairing())), Ok(environment(ENV)));
    // Toggling saves every entry too.
    let platform = harness.platform.clone();
    harness.run(async move {
        let mut registry: EnvironmentRegistry<SessionEndpoint> = EnvironmentRegistry::load(
            &platform,
            NetworkStatus::Unknown,
            Time::from_platform(&platform),
        )
        .await
        .unwrap();
        for enabled in [false, true] {
            registry
                .set_enabled_persisted(
                    &platform,
                    &environment(ENV),
                    enabled,
                    Time::from_platform(&platform),
                )
                .await
                .unwrap();
        }
    });
    // Removing the healthy one keeps the damaged one listed, and removing that works too.
    harness.run(client.remove(&environment(ENV))).unwrap();
    harness.settle();
    assert_eq!(client.environments().len(), 1);
    harness.run(client.remove(&environment("other"))).unwrap();
    harness.settle();
    assert!(client.environments().is_empty());
}

#[test]
fn signing_out_with_a_relay_only_and_a_mixed_environment_saved_together() {
    // Re-review N3: removing the relay-only one saved its own registry, so the stale outer
    // save failed and the mixed environment kept its relay route.
    let mut harness = Harness::new(ENV);
    harness.server().state.borrow_mut().signed_in = true;
    let client = relay_runtime(&harness);
    harness.run(client.pair(pairing())).unwrap();
    harness
        .run(client.add_relay_environment(&environment(ENV), "Fake host"))
        .unwrap();
    harness
        .run(client.add_relay_environment(&environment("other"), "Other host"))
        .unwrap();
    harness.advance(5_000);
    assert_eq!(client.environments().len(), 2);
    let signed_out = harness.run(client.remove_relay_environments());
    assert!(signed_out.is_ok(), "{signed_out:?}");
    harness.advance(5_000);
    let entries = client.environments();
    assert_eq!(entries.len(), 1, "the relay-only environment is gone");
    assert_eq!(entries[0].id, environment(ENV));
    assert_eq!(
        entries[0]
            .routes
            .iter()
            .map(|route| route.mode.clone())
            .collect::<Vec<_>>(),
        [ConnectionMode::Remote]
    );
    assert_eq!(entries[0].state.phase, Phase::Connected);
    // Round 6: nothing of the signed-out one is left in the store, or comes back.
    harness.advance(60_000);
    assert!(
        !client
            .store()
            .snapshot()
            .environments
            .contains_key(&environment("other"))
    );
}

#[test]
fn config_subscription_delivers_published_palette_updates_to_the_store() {
    use futures::StreamExt;
    use serde_json::json;
    use t3_contracts::rpc::{RpcMethod, WsSubscribeServerConfigRpc};

    let mut harness = Harness::new(ENV);
    let input = std::rc::Rc::new(std::cell::RefCell::new(None));
    let recorded = input.clone();
    harness.server().state.borrow_mut().extra_rpc = Some(std::rc::Rc::new(move |request| {
        if request.tag != WsSubscribeServerConfigRpc::TAG {
            return None;
        }
        recorded.replace(Some(request.payload.clone()));
        let mut events = vec![json!({"version":1, "type":"snapshot", "config":config(ENV)})];
        if request.payload["environmentThemes"] == true {
            events.push(json!({"version":1, "type":"environmentThemesUpdated", "payload":{
                "themes":[{"id":"nightfall","name":"Nightfall","appearance":"dark","canvas":"#1a1b26","accent":"#7aa2f7"}]
            }}));
        }
        Some(t3_rpc::HandlerOutput::Stream(
            futures::stream::once(async move { Ok(events) })
                .chain(futures::stream::pending())
                .boxed_local(),
        ))
    }));
    let client = paired(&mut harness);
    let store = client.store().snapshot();
    let config = store.environments[&environment(ENV)]
        .config
        .as_ref()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&config.environment_themes).unwrap()[0]["id"],
        "nightfall",
        "the server publishes palette events only to clients that requested them"
    );
    assert_eq!(
        *input.borrow(),
        Some(json!({
            "environmentThemes":true, "usageLimitSources":true, "usageLimitsCommand":true,
            "workspaceSnapshotsOnDemand":true
        }))
    );
}

#[test]
fn a_malformed_config_chunk_resubscribes_on_the_same_socket() {
    // WP5: the runtime's config job ended on a chunk failure, which replaced a healthy session.
    use futures::StreamExt;
    use t3_contracts::rpc::RpcMethod;
    let mut harness = Harness::new(ENV);
    let failed = std::rc::Rc::new(std::cell::Cell::new(false));
    let seen = failed.clone();
    harness.server().state.borrow_mut().extra_rpc = Some(std::rc::Rc::new(move |request| {
        if request.tag != t3_contracts::rpc::WsSubscribeServerConfigRpc::TAG || seen.replace(true) {
            return None;
        }
        Some(t3_rpc::HandlerOutput::Stream(
            futures::stream::iter([Ok(vec![serde_json::json!({"version": 1, "type": "bogus"})])])
                .chain(futures::stream::pending())
                .boxed_local(),
        ))
    }));
    let client = paired(&mut harness);
    harness.advance(5_000);
    assert!(failed.get(), "the malformed chunk was served");
    assert_eq!(harness.server().state.borrow().sockets_opened, 1);
    assert_eq!(client.environments()[0].state.phase, Phase::Connected);
}

#[test]
fn announced_direct_addresses_are_learned_for_that_environment_except_unsafe_ones() {
    // A learned route comes from the server's own config, so each announced address is checked
    // as TypeScript's `mergeLearnedRoutes` does before it can carry this environment's token.
    let mut harness = Harness::new(ENV);
    harness.server().state.borrow_mut().direct_endpoints = Some(serde_json::json!([
        // Loopback names whichever device opens it, never the server.
        {"kind": "lan", "httpBaseUrl": "http://127.0.0.1:3773/"},
        // Plain HTTP would carry a token that was paired over HTTPS.
        {"kind": "lan", "httpBaseUrl": "http://192.168.1.20:3773/"},
        {"kind": "lan", "httpBaseUrl": "ftp://192.168.1.20/"},
        {"kind": "tailnet", "httpBaseUrl": "https://box.example.ts.net/"},
    ]));
    let client = paired(&mut harness);
    harness.advance(1_000);
    let platform = harness.platform.clone();
    let routes = harness.run(async move {
        let registry: EnvironmentRegistry<SessionEndpoint> = EnvironmentRegistry::load(
            &platform,
            NetworkStatus::Unknown,
            Time::from_platform(&platform),
        )
        .await
        .unwrap();
        registry
            .get(&environment(ENV))
            .unwrap()
            .registration()
            .routes
            .clone()
    });
    let paired_route = routes
        .iter()
        .find(|route| !route.id.starts_with("learned:"))
        .unwrap()
        .id
        .clone();
    let learned: Vec<_> = routes
        .iter()
        .filter(|route| route.id.starts_with("learned:"))
        .collect();
    assert_eq!(learned.len(), 1, "{routes:?}");
    assert_eq!(learned[0].http_base, "https://box.example.ts.net/");
    // It borrows the paired credential by id, for this environment only.
    assert_eq!(
        learned[0].id,
        format!("learned:{ENV}:https://box.example.ts.net@{paired_route}")
    );
    let learned_id = learned[0].id.clone();
    let platform = harness.platform.clone();
    let id = learned_id.clone();
    let profile = harness.run(async move {
        t3_client::persistence::load_record::<serde_json::Value>(
            &platform,
            StorageArea::Secure,
            &t3_client::platform::StorageKey::new("connection-profile", None, &id),
            1,
        )
        .await
        .unwrap()
        .unwrap()
        .payload
    });
    assert_eq!(profile["learned"], true);
    assert_eq!(profile["network"], "tailscale");
    assert_eq!(profile["environmentId"], ENV);
    // Removing the environment removes what it announced.
    harness.run(client.remove(&environment(ENV))).unwrap();
    harness.settle();
    let platform = harness.platform.clone();
    let left = harness.run(async move {
        platform
            .get(
                StorageArea::Secure,
                t3_client::platform::StorageKey::new("connection-profile", None, &learned_id),
            )
            .await
            .unwrap()
    });
    assert_eq!(left, None);
}

/// Removal ends everything that writes the environment into the store before clearing it: a
/// "remove from this device" must not leave, or bring back, its config or connection state.
fn removed_and_stays_removed(harness: &mut Harness, client: &ClientRuntime<TestPlatform>) {
    harness.run(client.remove(&environment(ENV))).unwrap();
    harness.settle();
    let removed = |client: &ClientRuntime<TestPlatform>| {
        !client
            .store()
            .snapshot()
            .environments
            .contains_key(&environment(ENV))
    };
    assert!(removed(client), "cleared by remove");
    // Long enough for any surviving supervisor to reconnect and write again.
    harness.advance(120_000);
    assert!(removed(client), "nothing brought it back");
    assert!(client.environments().is_empty());
}

#[test]
fn a_removed_environment_does_not_come_back_into_the_store() {
    // Connection lane, round 6: after `remove`, the store held the environment again, with its
    // config, a connection state and a second session.
    let mut harness = Harness::new(ENV);
    let client = paired(&mut harness);
    harness.advance(5_000);
    assert!(
        client
            .store()
            .snapshot()
            .environments
            .get(&environment(ENV))
            .is_some_and(|state| state.config.is_some())
    );
    removed_and_stays_removed(&mut harness, &client);
}

#[test]
fn an_environment_removed_while_it_reconnects_does_not_come_back_into_the_store() {
    let mut harness = Harness::new(ENV);
    let client = paired(&mut harness);
    harness.advance(5_000);
    harness.server().state.borrow_mut().refuse_sockets = true;
    harness.server().hang_up();
    harness.advance(2_000);
    assert_ne!(client.environments()[0].state.phase, Phase::Connected);
    harness.server().state.borrow_mut().refuse_sockets = false;
    removed_and_stays_removed(&mut harness, &client);
}

#[test]
fn first_run_policy_waits_for_the_servers_bootstrap_provenance() {
    use futures::{StreamExt, channel::mpsc};
    use serde_json::json;
    use t3_contracts::rpc::{
        RpcMethod, WsOrchestrationV2SubscribeShellRpc, WsSubscribeServerLifecycleRpc,
    };
    let mut harness = Harness::new(ENV);
    let (events, receiver) = mpsc::unbounded();
    let receiver = std::cell::RefCell::new(Some(receiver));
    let server = harness.server();
    server
        .state
        .borrow_mut()
        .extra_methods
        .push((WsSubscribeServerLifecycleRpc::TAG, t3_rpc::CallKind::Stream));
    server.state.borrow_mut().extra_rpc = Some(std::rc::Rc::new(move |request| {
        if request.tag == WsSubscribeServerLifecycleRpc::TAG {
            return Some(t3_rpc::HandlerOutput::Stream(
                receiver.borrow_mut().take()?.boxed_local(),
            ));
        }
        if request.tag == WsOrchestrationV2SubscribeShellRpc::TAG {
            let mut shell = shell_snapshot(&[]);
            shell["projects"] = json!([]);
            return Some(t3_rpc::HandlerOutput::Stream(
                futures::stream::iter([Ok(vec![
                    json!({"kind":"snapshot","snapshot":shell}),
                    json!({"kind":"synchronized"}),
                ])])
                .chain(futures::stream::pending())
                .boxed_local(),
            ));
        }
        None
    }));
    let client = runtime(&harness);
    harness.run(client.restore()).unwrap();
    harness.run(client.pair(pairing())).unwrap();
    harness.settle();
    assert!(!client.route_policy().unwrap().first_run);
    let observed = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let recorder = observed.clone();
    let mut changes = client.first_run().watch();
    harness.spawn(async move {
        while let Some(next) = changes.next().await {
            recorder.borrow_mut().push(next);
        }
    });
    harness.settle();
    assert!(!observed.borrow().last().unwrap().gate.stalled);
    harness.advance(4000);
    assert!(observed.borrow().last().unwrap().gate.stalled);
    for status in ["pending", "complete"] {
        events.unbounded_send(Ok(vec![json!({"type":"welcome","version":1,"sequence":1,"payload":{"environment":config(ENV)["environment"],"cwd":"/workspace/project","projectName":"Project","bootstrapStatus":status}})])).unwrap();
        harness.settle();
        assert_eq!(
            client.route_policy().unwrap().first_run,
            status == "complete"
        );
    }
    assert_eq!(
        observed.borrow().last().unwrap().gate.decision,
        t3_client::view_logic::onboarding::FirstRunDecision::Wizard
    );
    let id = environment(ENV);
    let generation = client.store().snapshot().environments[&id].session;
    client.store().set_welcome(&id, generation, None);
    harness.settle();
    assert!(
        client.route_policy().unwrap().first_run,
        "temporary loss of provenance must hold the wizard"
    );
    let first_run = client.first_run();
    harness
        .run(async move { first_run.complete().await })
        .unwrap();
    assert!(!client.route_policy().unwrap().first_run);
}

#[test]
fn local_codex_setup_installs_before_sign_in_and_ignores_other_flows() {
    use futures::{FutureExt, StreamExt};
    use serde_json::{Value, json};
    use std::{cell::RefCell, rc::Rc};
    use t3_contracts::{
        provider_instance::ProviderInstanceId,
        rpc::{
            RpcMethod, WsProviderAuthStartRpc, WsProviderAuthSubscribeRpc,
            WsProviderInstallStartRpc, WsProviderInstallSubscribeRpc,
        },
    };
    use t3_rpc::{CallKind, Exit, HandlerOutput};
    let mut harness = Harness::new(ENV);
    let calls = Rc::new(RefCell::new(Vec::new()));
    let seen = calls.clone();
    let install = |phase: &str, operation: Option<&str>, version: Option<&str>| {
        json!({
            "driver":"codex", "operationId":operation, "phase":phase, "downloadedBytes":0,
            "totalBytes":null, "version":"1", "installedVersion":version, "canRemove":false, "message":null
        })
    };
    let auth = |phase: &str, flow: &str, page: Option<&str>| {
        json!({
            "instanceId":"codex","phase":phase,"flowId":flow,"authorizationUrl":page,"expiresAt":null,"message":null
        })
    };
    let target = "https://example.test/sign-in";
    {
        let mut server = harness.server().state.borrow_mut();
        server.extra_methods.extend([
            (WsProviderInstallStartRpc::TAG, CallKind::Unary),
            (WsProviderInstallSubscribeRpc::TAG, CallKind::Stream),
            (WsProviderAuthStartRpc::TAG, CallKind::Unary),
            (WsProviderAuthSubscribeRpc::TAG, CallKind::Stream),
        ]);
        server.extra_rpc = Some(Rc::new(move |request| {
            let unary = |answer: Value| {
                HandlerOutput::Unary(async move { Exit::success(answer) }.boxed_local())
            };
            let stream = |answers: Vec<Value>| {
                HandlerOutput::Stream(
                    futures::stream::iter(answers.into_iter().map(|v| Ok(vec![v])))
                        .chain(futures::stream::pending())
                        .boxed_local(),
                )
            };
            let result = match request.tag.as_str() {
                WsProviderInstallSubscribeRpc::TAG => stream(vec![
                    install("idle", None, None),
                    install("failed", Some("old"), None),
                    install("succeeded", Some("setup"), Some("1")),
                ]),
                WsProviderInstallStartRpc::TAG => {
                    unary(install("downloading", Some("setup"), None))
                }
                WsProviderAuthStartRpc::TAG => unary(auth("starting", "new", None)),
                WsProviderAuthSubscribeRpc::TAG => stream(vec![
                    auth("waiting", "old", Some("https://example.test/old")),
                    auth("waiting", "new", Some(target)),
                ]),
                _ => return None,
            };
            seen.borrow_mut().push(request.tag.clone());
            Some(result)
        }));
    }
    let client = runtime(&harness);
    let id = harness
        .run(client.pair(PairingInput {
            pairing_url: Some("http://127.0.0.1/pair#token=pairing-token".into()),
            ..PairingInput::default()
        }))
        .unwrap();
    harness.settle();
    client
        .client(&id)
        .unwrap()
        .session()
        .unwrap()
        .update_grants(t3_client::permissions::SessionGrants {
            valid: true,
            scopes: ["providers:manage".to_owned()].into_iter().collect(),
        });
    let mut cfg = config(ENV);
    cfg["settings"] =
        serde_json::to_value(t3_contracts::settings::ServerSettings::default()).unwrap();
    cfg["settings"]["providerInstances"]["codex"] =
        json!({"driver":"codex","enabled":true,"config":{"setupMode":"managed"}});
    cfg["providers"] = json!([{"instanceId":"codex","driver":"codex","enabled":true,"installed":false,"version":null,
        "status":"ready","auth":{"status":"unauthenticated"},"checkedAt":"2026-08-23T00:00:00.000Z",
        "setup":{"canAuthenticate":true,"canInstall":true},"models":[],"slashCommands":[],"skills":[]}]);
    let generation = client.store().snapshot().environments[&id].session;
    assert!(
        client
            .store()
            .set_config(&id, generation, serde_json::from_value(cfg).unwrap())
    );
    let first_run = client.first_run();
    let page = harness
        .run(async move {
            first_run
                .sign_in_page(&id, ProviderInstanceId("codex".into()))
                .await
        })
        .unwrap();
    assert_eq!(page, target);
    assert_eq!(
        *calls.borrow(),
        [
            WsProviderInstallSubscribeRpc::TAG,
            WsProviderInstallStartRpc::TAG,
            WsProviderAuthStartRpc::TAG,
            WsProviderAuthSubscribeRpc::TAG
        ]
    );
}
