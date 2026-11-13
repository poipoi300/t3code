#![allow(clippy::unwrap_used)]

use std::{cell::Cell, rc::Rc};
use t3_client::{
    connection::{ConnectionEvent, ConnectionMode, Phase, Route, Time},
    environment::*,
    platform::{NetworkStatus, Storage, StorageArea},
    testing::FakePlatform,
    types::{EnvironmentId, Generation, ProjectId, ThreadId},
};

fn registration(id: &str, enabled: bool) -> EnvironmentRegistration {
    let id = EnvironmentId(id.into());
    EnvironmentRegistration {
        id: id.clone(),
        label: "Host".into(),
        enabled,
        routes: vec![Route {
            id: "direct".into(),
            environment: id,
            mode: ConnectionMode::Remote,
            http_base: "https://host.test/".into(),
            ws_base: "wss://host.test/".into(),
        }],
    }
}

#[test]
fn endpoints_normalize_origin_and_clear_path_query_and_fragment() {
    for (input, http, ws) in [
        (
            "https://example.com/path?x=1#hash",
            "https://example.com/",
            "wss://example.com/",
        ),
        (
            "wss://example.com/socket",
            "https://example.com/",
            "wss://example.com/",
        ),
        (
            "http://127.0.0.1:3773/api",
            "http://127.0.0.1:3773/",
            "ws://127.0.0.1:3773/",
        ),
        (
            "https://EXAMPLE.com:443/",
            "https://example.com/",
            "wss://example.com/",
        ),
    ] {
        assert_eq!(normalize_http_base_url(input).unwrap(), http);
        assert_eq!(derive_ws_base_url(input).unwrap(), ws);
        assert_eq!(
            environment_endpoint_url(input, "/mcp").unwrap(),
            format!(
                "{}mcp",
                if input.starts_with("wss:") {
                    "wss://example.com/"
                } else {
                    http
                }
            )
        );
    }
    assert!(normalize_http_base_url("file:///secret").is_err());
    assert!(normalize_http_base_url("not a URL").is_err());
}

#[test]
fn scoped_keys_parse_only_first_separator_and_keep_environment_identity() {
    let a = scope_project_ref(
        EnvironmentId("a".into()),
        ProjectId("project:with:colons".into()),
    );
    let b = scope_thread_ref(EnvironmentId("b".into()), ThreadId("thread".into()));
    assert_eq!(scoped_project_key(&a), "a:project:with:colons");
    assert_eq!(parse_scoped_project_key(&scoped_project_key(&a)), Some(a));
    assert_eq!(parse_scoped_thread_key(&scoped_thread_key(&b)), Some(b));
    for invalid in ["", ":thread", "environment:", "thread"] {
        assert!(parse_scoped_thread_key(invalid).is_none());
        assert!(parse_scoped_project_key(invalid).is_none());
    }
}

#[test]
fn registry_preserves_catalog_position_and_disabled_intent() {
    let mut registry = EnvironmentRegistry::<()>::default();
    for id in ["z", "a", "m"] {
        registry
            .register(
                registration(id, id != "a"),
                false,
                NetworkStatus::Online,
                Time::default(),
            )
            .unwrap();
    }
    registry
        .register(
            registration("a", false),
            false,
            NetworkStatus::Online,
            Time::default(),
        )
        .unwrap();
    assert_eq!(
        registry
            .entries()
            .map(|e| e.id.0.as_str())
            .collect::<Vec<_>>(),
        ["z", "a", "m"]
    );
    let a = registry.get(&EnvironmentId("a".into())).unwrap();
    assert_eq!(a.supervisor().state().phase, Phase::Available);
    assert!(!a.supervisor().state().desired);
    let z = registry.get(&EnvironmentId("z".into())).unwrap();
    assert_eq!(z.supervisor().state().phase, Phase::Connecting);
}

#[test]
fn route_validation_and_reorder_are_atomic() {
    let mut registry = EnvironmentRegistry::<()>::default();
    let mut entry = registration("a", true);
    let mut relay = entry.routes[0].clone();
    relay.id = "relay".into();
    relay.mode = ConnectionMode::Relay;
    entry.routes.push(relay);
    registry
        .register(entry, false, NetworkStatus::Online, Time::default())
        .unwrap();
    let id = EnvironmentId("a".into());
    for invalid in [
        vec!["direct".into()],
        vec!["direct".into(), "direct".into()],
        vec!["other".into(), "relay".into()],
    ] {
        assert!(
            registry
                .reorder_routes(&id, &invalid, Time::default())
                .is_err()
        );
        assert_eq!(
            registry.get(&id).unwrap().registration().routes[0].id,
            "direct"
        );
    }
    registry
        .reorder_routes(&id, &["relay".into(), "direct".into()], Time::default())
        .unwrap();
    assert_eq!(
        registry.get(&id).unwrap().registration().routes[0].id,
        "relay"
    );
    let mut wrong = registration("b", true);
    wrong.routes[0].environment = id;
    assert!(
        registry
            .register(wrong, false, NetworkStatus::Online, Time::default())
            .is_err()
    );
}

struct Session(Rc<Cell<usize>>);
impl Drop for Session {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn session_install_has_one_winner_and_releases_losers_and_disabled_sessions() {
    let dropped = Rc::new(Cell::new(0));
    let mut registry = EnvironmentRegistry::default();
    registry
        .register(
            registration("a", true),
            false,
            NetworkStatus::Online,
            Time::default(),
        )
        .unwrap();
    let handle = registry.get_mut(&EnvironmentId("a".into())).unwrap();
    let attempt = handle.supervisor().active_attempt().unwrap();
    let identity = handle.pending_identity().unwrap();
    assert!(
        handle
            .install(
                attempt,
                "direct",
                identity.clone(),
                Session(dropped.clone()),
                Time::default()
            )
            .is_ok()
    );
    assert!(
        handle
            .install(
                attempt,
                "direct",
                identity,
                Session(dropped.clone()),
                Time::default()
            )
            .is_err()
    );
    assert_eq!(dropped.get(), 1);
    assert!(handle.session().is_some());
    handle.handle(ConnectionEvent::SetEnabled(false), Time::default(), 0.5);
    assert_eq!(dropped.get(), 2);
    assert!(handle.session().is_none());
}

#[test]
fn wrong_environment_or_generation_cannot_install_a_session() {
    let dropped = Rc::new(Cell::new(0));
    let mut registry = EnvironmentRegistry::default();
    registry
        .register(
            registration("a", true),
            false,
            NetworkStatus::Online,
            Time::default(),
        )
        .unwrap();
    let handle = registry.get_mut(&EnvironmentId("a".into())).unwrap();
    let attempt = handle.supervisor().active_attempt().unwrap();
    let mut identity = handle.pending_identity().unwrap();
    identity.environment = EnvironmentId("wrong".into());
    assert!(
        handle
            .install(
                attempt,
                "direct",
                identity,
                Session(dropped.clone()),
                Time::default()
            )
            .is_err()
    );
    let mut identity = handle.pending_identity().unwrap();
    identity.generation = Generation(0);
    assert!(
        handle
            .install(
                attempt,
                "direct",
                identity,
                Session(dropped.clone()),
                Time::default()
            )
            .is_err()
    );
    assert_eq!(dropped.get(), 2);
    assert_eq!(handle.supervisor().state().phase, Phase::Connecting);
}

#[test]
fn removed_and_reregistered_environment_rejects_old_attempts() {
    let dropped = Rc::new(Cell::new(0));
    let id = EnvironmentId("a".into());
    let mut registry = EnvironmentRegistry::default();
    registry
        .register(
            registration("a", true),
            false,
            NetworkStatus::Online,
            Time::default(),
        )
        .unwrap();
    let old = registry.get(&id).unwrap();
    let attempt = old.supervisor().active_attempt().unwrap();
    let identity = old.pending_identity().unwrap();
    registry.remove(&id, Time::default()).unwrap();
    registry
        .register(
            registration("a", true),
            false,
            NetworkStatus::Online,
            Time::default(),
        )
        .unwrap();
    assert!(
        registry
            .get_mut(&id)
            .unwrap()
            .install(
                attempt,
                "direct",
                identity,
                Session(dropped.clone()),
                Time::default()
            )
            .is_err()
    );
    assert_eq!(dropped.get(), 1);
}

#[test]
fn disabling_is_persisted_before_runtime_and_survives_reload() {
    futures::executor::block_on(async {
        let platform = FakePlatform::default();
        let id = EnvironmentId("a".into());
        let mut registry = EnvironmentRegistry::<()>::default();
        registry
            .register(
                registration("a", true),
                false,
                NetworkStatus::Online,
                Time::default(),
            )
            .unwrap();
        registry
            .set_enabled_persisted(&platform, &id, false, Time::default())
            .await
            .unwrap();
        assert!(!registry.get(&id).unwrap().supervisor().enabled());
        let restored =
            EnvironmentRegistry::<()>::load(&platform, NetworkStatus::Online, Time::default())
                .await
                .unwrap();
        assert!(!restored.get(&id).unwrap().registration().enabled);
        assert_eq!(
            restored.get(&id).unwrap().supervisor().state().phase,
            Phase::Available
        );
        assert!(
            !platform
                .list(StorageArea::Ordinary, "connection-catalog".into())
                .await
                .unwrap()
                .is_empty()
        );
        registry
            .set_enabled_persisted(&platform, &id, true, Time::default())
            .await
            .unwrap();
        let mut restored =
            EnvironmentRegistry::<()>::load(&platform, NetworkStatus::Online, Time::default())
                .await
                .unwrap();
        let startup = restored.broadcast(ConnectionEvent::Connect, Time::default(), 0.5);
        assert!(matches!(
            startup.actions.as_slice(),
            [(environment, t3_client::connection::ConnectionAction::Establish { .. })] if environment == &id
        ));
    });
}

#[test]
fn platform_owned_environment_cannot_be_removed_by_user() {
    let mut registry = EnvironmentRegistry::<()>::default();
    registry
        .register(
            registration("primary", true),
            true,
            NetworkStatus::Online,
            Time::default(),
        )
        .unwrap();
    assert!(
        registry
            .remove(&EnvironmentId("primary".into()), Time::default())
            .is_err()
    );
}

#[test]
fn persisted_environment_removal_erases_credentials_tokens_and_routing_grants() {
    use t3_client::{
        authorization::{RemoteDpopAccessToken, RemoteTokenStore, Sensitive},
        connection::{
            ConnectionCredential, ConnectionProfile, ConnectionStores, PreparedRegistration,
        },
        persistence::{RoutingAccess, compare_exchange_routing_grant, load_routing_grant},
    };
    futures::executor::block_on(async {
        let platform = Rc::new(FakePlatform::default());
        let mut registry = EnvironmentRegistry::<()>::default();
        let entry = registration("a", false);
        let id = entry.id.clone();
        registry
            .register_prepared(
                platform.as_ref(),
                PreparedRegistration {
                    profile: ConnectionProfile::Bearer {
                        connection_id: "direct".into(),
                        environment_id: id.clone(),
                        label: entry.label.clone(),
                        http_base_url: entry.routes[0].http_base.clone(),
                        ws_base_url: entry.routes[0].ws_base.clone(),
                        learned: None,
                        authorization: None,
                        network: None,
                    },
                    credential: ConnectionCredential::Bearer {
                        token: Sensitive("removed-bearer".into()),
                    },
                    registration: entry,
                },
                false,
                false,
                NetworkStatus::Online,
                Time::default(),
            )
            .await
            .unwrap();
        let tokens = RemoteTokenStore::new(platform.clone());
        tokens.put(&RemoteDpopAccessToken {
            environment_id: id.clone(), account_id: Some("account".into()), label: "Host".into(),
            endpoint: serde_json::from_value(serde_json::json!({"httpBaseUrl":"https://host.test", "wsBaseUrl":"wss://host.test", "providerKind":"cloudflare_tunnel"})).unwrap(),
            access_token: Sensitive("removed-dpop".into()), expires_at_epoch_ms: 1_000_000, dpop_thumbprint: "key".into(),
        }).await.unwrap();
        compare_exchange_routing_grant(
            platform.as_ref(),
            &id,
            None,
            "saved-endpoint".into(),
            RoutingAccess::ReadWrite,
        )
        .await
        .unwrap();
        registry.remove(&id, Time::default()).unwrap();
        registry.save(platform.as_ref()).await.unwrap();
        let restored = EnvironmentRegistry::<()>::load(
            platform.as_ref(),
            NetworkStatus::Online,
            Time::default(),
        )
        .await
        .unwrap();
        assert!(restored.get(&id).is_none());
        let stores = ConnectionStores::new(platform.clone());
        assert!(stores.profile(&id, "direct").await.unwrap().is_none());
        assert!(stores.credential(&id, "direct").await.unwrap().is_none());
        assert!(tokens.get(&id).await.unwrap().is_none());
        assert!(
            load_routing_grant(platform.as_ref(), &id)
                .await
                .unwrap()
                .is_none()
        );
    });
}

#[test]
fn stale_catalog_write_cannot_reenable_a_disabled_environment() {
    futures::executor::block_on(async {
        let platform = FakePlatform::default();
        let id = EnvironmentId("a".into());
        let mut first = EnvironmentRegistry::<()>::default();
        first
            .register(
                registration("a", true),
                false,
                NetworkStatus::Online,
                Time::default(),
            )
            .unwrap();
        first.save(&platform).await.unwrap();
        let mut second =
            EnvironmentRegistry::<()>::load(&platform, NetworkStatus::Online, Time::default())
                .await
                .unwrap();
        first
            .set_enabled_persisted(&platform, &id, false, Time::default())
            .await
            .unwrap();
        assert!(second.save(&platform).await.is_err());
        assert!(second.get(&id).unwrap().registration().enabled);
        let loaded =
            EnvironmentRegistry::<()>::load(&platform, NetworkStatus::Online, Time::default())
                .await
                .unwrap();
        assert!(!loaded.get(&id).unwrap().registration().enabled);
    });
}

#[test]
fn removing_one_route_retains_environment_and_emits_scoped_cleanup() {
    let mut registry = EnvironmentRegistry::<()>::default();
    let id = EnvironmentId("a".into());
    let mut entry = registration("a", true);
    let mut relay = entry.routes[0].clone();
    relay.id = "relay".into();
    relay.mode = ConnectionMode::Relay;
    entry.routes.push(relay);
    registry
        .register(entry, false, NetworkStatus::Online, Time::default())
        .unwrap();
    let change = registry
        .remove_route(&id, "direct", Time::default())
        .unwrap();
    assert_eq!(registry.get(&id).unwrap().registration().routes.len(), 1);
    assert!(change.cleanup.contains(&EnvironmentCleanup::Route {
        environment: id.clone(),
        route_id: "direct".into()
    }));
    let change = registry
        .remove_route(&id, "relay", Time::default())
        .unwrap();
    assert!(registry.get(&id).is_none());
    assert!(
        change
            .cleanup
            .contains(&EnvironmentCleanup::Environment(id))
    );
}
