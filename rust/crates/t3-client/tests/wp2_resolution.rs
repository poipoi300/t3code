#![allow(clippy::unwrap_used)]

use futures::{FutureExt, future::LocalBoxFuture};
use serde_json::json;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use t3_client::{
    authorization::{HttpAuthorization, RemoteAuthorization, Sensitive},
    connection::*,
    environment::SessionIdentity,
    platform::{HostReply, HttpResponse, Platform, Storage, StorageArea},
    relay::{DpopSigner, ManagedRelayClient, RelayError},
    testing::{FakePlatform, ScriptedRpc},
    types::{EnvironmentId, Generation},
};

struct Signer;
impl DpopSigner for Signer {
    fn thumbprint(&self) -> LocalBoxFuture<'_, Result<String, RelayError>> {
        async { Ok("thumbprint".into()) }.boxed_local()
    }
    fn create_proof<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: Option<&'a str>,
    ) -> LocalBoxFuture<'a, Result<Sensitive<String>, RelayError>> {
        async { Ok(Sensitive("proof".into())) }.boxed_local()
    }
}
fn service(platform: Rc<dyn Platform>) -> RemoteAuthorization {
    let signer = Rc::new(Signer);
    let relay = ManagedRelayClient::new(
        platform.clone(),
        signer.clone(),
        "https://relay.test",
        t3_contracts::relay::RelayPublicClientId::T3Web,
    );
    RemoteAuthorization::new(platform, signer, relay)
}
fn identity() -> SessionIdentity {
    SessionIdentity {
        environment: EnvironmentId("environment-1".into()),
        generation: Generation(1),
        account: None,
        dpop_thumbprint: None,
    }
}
fn route(id: &str, mode: ConnectionMode) -> Route {
    Route {
        id: id.into(),
        environment: identity().environment,
        mode,
        http_base: "http://127.0.0.1:3777/".into(),
        ws_base: "ws://127.0.0.1:3777/".into(),
    }
}
fn response(body: serde_json::Value) -> Result<HttpResponse, t3_client::platform::PlatformError> {
    Ok(HttpResponse {
        status: 200,
        headers: BTreeMap::new(),
        body: serde_json::to_vec(&body).unwrap(),
    })
}
fn descriptor(protocol: Option<i64>) -> serde_json::Value {
    let mut value = json!({"environmentId":"environment-1", "label":"Compatible environment", "platform":{"os":"linux", "arch":"x64"}, "serverVersion":"0.0.0-test", "capabilities":{"repositoryIdentity":true}});
    if let Some(protocol) = protocol {
        value["orchestrationProtocolVersion"] = protocol.into();
    }
    value
}
fn primary_host(platform: &FakePlatform) {
    platform.push_host(Ok(HostReply::Text(None)));
    platform.push_host(Ok(HostReply::Document(
        json!({"label":"Test Client", "deviceType":"desktop", "surface":"web"}),
    )));
    platform.push_host(Ok(HostReply::Text(None)));
    platform.push_host(Ok(HostReply::Text(None)));
}

#[test]
fn primary_cookie_connection_needs_no_remote_ticket_and_announces_protocol() {
    futures::executor::block_on(async {
        let platform = Rc::new(FakePlatform::default());
        primary_host(&platform);
        platform.push_http(response(descriptor(Some(2))));
        let remote = service(platform.clone());
        let resolver = ConnectionResolver::new(platform.clone(), remote);
        let prepared = resolver
            .prepare(&route("primary", ConnectionMode::Local), &identity())
            .await
            .unwrap();
        assert_eq!(prepared.authorization, HttpAuthorization::Cookie);
        assert_eq!(
            prepared.socket_url.expose(),
            "ws://127.0.0.1:3777/ws?clientSurface=web&clientDeviceType=desktop&connectionMethod=direct&orchestrationProtocol=2&clientCapabilities=settingsSecretReferences"
        );
        assert_eq!(platform.http_requests().len(), 1);
        assert!(
            platform.http_requests()[0]
                .url
                .ends_with("/.well-known/t3/environment")
        );
    });
}

#[test]
fn old_host_is_blocked_but_remains_reachable_for_update() {
    futures::executor::block_on(async {
        let platform = Rc::new(FakePlatform::default());
        let remote = service(platform.clone());
        remote.set_session(identity());
        let resolver = ConnectionResolver::new(platform.clone(), remote);
        primary_host(&platform);
        platform.push_http(response(descriptor(None)));
        let error = resolver
            .prepare(&route("primary", ConnectionMode::Local), &identity())
            .await
            .unwrap_err();
        assert_eq!(error.kind, FailureKind::Unsupported);
        primary_host(&platform);
        platform.push_http(response(descriptor(None)));
        let prepared = resolver
            .prepare_for_update(&route("primary", ConnectionMode::Local), &identity())
            .await
            .unwrap();
        assert!(prepared.descriptor.is_some());
        assert!(
            !prepared
                .socket_url
                .expose()
                .contains("orchestrationProtocol")
        );
    });
}

#[test]
fn profile_and_credential_records_match_shared_persistence_and_are_secure() {
    futures::executor::block_on(async {
        let platform = Rc::new(FakePlatform::default());
        let stores = ConnectionStores::new(platform.clone());
        let profile = ConnectionProfile::Bearer {
            connection_id: "saved".into(),
            environment_id: identity().environment,
            label: "Remote".into(),
            http_base_url: "https://host.test/".into(),
            ws_base_url: "wss://host.test/".into(),
            learned: None,
            authorization: None,
            network: None,
        };
        let credential = ConnectionCredential::Bearer {
            token: Sensitive("secret".into()),
        };
        stores
            .register(profile.clone(), Some(credential.clone()))
            .await
            .unwrap();
        assert_eq!(
            stores
                .profile(&identity().environment, "saved")
                .await
                .unwrap(),
            Some(profile)
        );
        assert_eq!(
            stores
                .credential(&identity().environment, "saved")
                .await
                .unwrap(),
            Some(credential)
        );
        assert!(
            platform
                .list(StorageArea::Ordinary, "connection-credential".into())
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            platform
                .list(StorageArea::Ordinary, "connection-profile".into())
                .await
                .unwrap()
                .is_empty()
        );
        let key = platform
            .list(StorageArea::Secure, "connection-credential".into())
            .await
            .unwrap()
            .pop()
            .unwrap();
        let record: t3_client::persistence::Record<serde_json::Value> = serde_json::from_slice(
            &platform
                .get(StorageArea::Secure, key)
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(record.payload["connectionId"], "saved");
        assert_eq!(
            record.payload["credential"]["_tag"],
            "BearerConnectionCredential"
        );
        assert!(
            stores
                .profile(&EnvironmentId("other".into()), "saved")
                .await
                .is_err()
        );
        stores
            .remove(&identity().environment, "saved")
            .await
            .unwrap();
        assert!(
            stores
                .profile(&identity().environment, "saved")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            stores
                .credential(&identity().environment, "saved")
                .await
                .unwrap()
                .is_none()
        );
    });
}

struct ClosedFactory;
impl SessionFactory for ClosedFactory {
    type Session = ScriptedRpc;
    fn open<'a>(
        &'a self,
        _: &'a t3_client::authorization::AuthorizedEnvironment,
    ) -> LocalBoxFuture<'a, Result<Self::Session, ConnectionFailure>> {
        async {
            Err(ConnectionFailure::new(
                FailureKind::Transport,
                "Socket closed.",
            ))
        }
        .boxed_local()
    }
    fn ready<'a>(
        &'a self,
        _: &'a Self::Session,
    ) -> LocalBoxFuture<'a, Result<(), ConnectionFailure>> {
        async { Ok(()) }.boxed_local()
    }
}

#[test]
fn registered_bearer_connects_without_manual_authorization_session_setup() {
    use t3_client::environment::EnvironmentRegistry;
    struct ReadyFactory;
    impl SessionFactory for ReadyFactory {
        type Session = ScriptedRpc;
        fn open<'a>(
            &'a self,
            _: &'a t3_client::authorization::AuthorizedEnvironment,
        ) -> LocalBoxFuture<'a, Result<Self::Session, ConnectionFailure>> {
            futures::future::ready(Ok(ScriptedRpc::default())).boxed_local()
        }
        fn ready<'a>(
            &'a self,
            _: &'a Self::Session,
        ) -> LocalBoxFuture<'a, Result<(), ConnectionFailure>> {
            futures::future::ready(Ok(())).boxed_local()
        }
    }
    futures::executor::block_on(async {
        let platform = Rc::new(FakePlatform::default());
        let remote = service(platform.clone());
        let mut registry = EnvironmentRegistry::with_authorization(remote.clone());
        let route = route("saved", ConnectionMode::Remote);
        registry
            .register_prepared(
                platform.as_ref(),
                PreparedRegistration {
                    registration: t3_client::environment::EnvironmentRegistration {
                        id: route.environment.clone(),
                        label: "Host".into(),
                        routes: vec![route.clone()],
                        enabled: true,
                    },
                    profile: ConnectionProfile::Bearer {
                        connection_id: route.id.clone(),
                        environment_id: route.environment.clone(),
                        label: "Host".into(),
                        http_base_url: route.http_base.clone(),
                        ws_base_url: route.ws_base.clone(),
                        learned: None,
                        authorization: None,
                        network: None,
                    },
                    credential: ConnectionCredential::Bearer {
                        token: Sensitive("saved-bearer".into()),
                    },
                },
                false,
                true,
                t3_client::platform::NetworkStatus::Online,
                Time::default(),
            )
            .await
            .unwrap();
        let handle = registry.get(&route.environment).unwrap();
        let identity = handle.pending_identity().unwrap();
        let attempt = handle.supervisor().active_attempt().unwrap();
        platform.push_http(response(descriptor(Some(2))));
        platform.push_http(response(
            json!({"ticket":"ticket", "expiresAt":"2026-11-14T00:00:00.000Z"}),
        ));
        platform.push_host(Ok(HostReply::Text(None)));
        platform.push_host(Ok(HostReply::Text(None)));
        let driver = ResolvedConnectionDriver::new(
            ConnectionResolver::new(platform.clone(), remote),
            ReadyFactory,
            identity,
            Rc::new(|_| {}),
        );
        let lease = driver.connect(vec![route.clone()]).await.unwrap();
        registry
            .get_mut(&route.environment)
            .unwrap()
            .install_authorized(
                attempt,
                &route.id,
                lease.prepared,
                lease.session,
                Time::default(),
            )
            .unwrap();
        assert!(
            registry
                .get(&route.environment)
                .unwrap()
                .session()
                .is_some()
        );
        assert_eq!(platform.http_requests().len(), 2);
        assert_eq!(
            platform.http_requests()[1]
                .headers
                .get("authorization")
                .map(String::as_str),
            Some("Bearer saved-bearer")
        );
    });
}

#[test]
fn driver_reports_preparing_before_each_ssh_route_and_saves_rotated_token() {
    futures::executor::block_on(async {
        let platform = Rc::new(FakePlatform::default());
        let remote = service(platform.clone());
        remote.set_session(identity());
        let stores = ConnectionStores::new(platform.clone());
        let target = json!({"alias":"build", "hostname":"host.test", "username":null, "port":null});
        for id in ["ssh-first", "ssh-second"] {
            stores
                .register(
                    ConnectionProfile::Ssh {
                        connection_id: id.into(),
                        environment_id: identity().environment,
                        label: "SSH".into(),
                        target: serde_json::from_value(target.clone()).unwrap(),
                    },
                    Some(ConnectionCredential::Bearer {
                        token: Sensitive("revoked-token".into()),
                    }),
                )
                .await
                .unwrap();
            platform.push_host(Ok(HostReply::Document(json!({"bootstrap":{"target":target, "httpBaseUrl":"http://127.0.0.1:3777/", "wsBaseUrl":"ws://127.0.0.1:3777/", "pairingToken":null}, "bearerToken":"paired-token"}))));
            platform.push_host(Ok(HostReply::Text(None)));
            platform.push_host(Ok(HostReply::Text(None)));
            platform.push_http(response(descriptor(Some(2))));
            platform.push_http(response(
                json!({"ticket":"ticket", "expiresAt":"2026-09-04T01:00:00.000Z"}),
            ));
        }
        let stages = Rc::new(RefCell::new(Vec::new()));
        let observed = stages.clone();
        let driver = ResolvedConnectionDriver::new(
            ConnectionResolver::new(platform.clone(), remote),
            ClosedFactory,
            identity(),
            Rc::new(move |stage| observed.borrow_mut().push(stage)),
        );
        let error = driver
            .connect(vec![
                route("ssh-first", ConnectionMode::Ssh),
                route("ssh-second", ConnectionMode::Ssh),
            ])
            .await
            .err()
            .unwrap();
        assert_eq!(error.kind, FailureKind::Transport);
        assert_eq!(
            *stages.borrow(),
            [
                AttemptStage::Preparing,
                AttemptStage::Preparing,
                AttemptStage::Opening,
                AttemptStage::Preparing,
                AttemptStage::Opening
            ]
        );
        assert_eq!(
            stores
                .credential(&identity().environment, "ssh-first")
                .await
                .unwrap()
                .unwrap()
                .token()
                .expose(),
            "paired-token"
        );
    });
}

#[test]
fn replaced_identity_and_wrong_route_environment_fail_before_http() {
    futures::executor::block_on(async {
        let platform = Rc::new(FakePlatform::default());
        let remote = service(platform.clone());
        let resolver = ConnectionResolver::new(platform.clone(), remote.clone());
        remote.set_session(SessionIdentity {
            generation: Generation(2),
            ..identity()
        });
        assert!(
            resolver
                .prepare(&route("primary", ConnectionMode::Local), &identity())
                .await
                .is_err()
        );
        remote.set_session(identity());
        let mut wrong = route("primary", ConnectionMode::Local);
        wrong.environment = EnvironmentId("other".into());
        assert!(resolver.prepare(&wrong, &identity()).await.is_err());
        assert!(platform.http_requests().is_empty());
    });
}

#[test]
fn pairing_resolves_hash_tokens_hosted_links_and_host_plus_code() {
    for (input, base, token) in [
        (
            PairingInput {
                pairing_url: Some("https://host.test/path?token=query#token=hash".into()),
                ..PairingInput::default()
            },
            "https://host.test/",
            "hash",
        ),
        (
            PairingInput {
                pairing_url: Some("https://app.t3.codes/?host=build.test%3A3773#token=code".into()),
                ..PairingInput::default()
            },
            "https://build.test:3773/",
            "code",
        ),
        (
            PairingInput {
                host: Some(" //build.test:3773/path ".into()),
                pairing_code: Some(" code ".into()),
                ..PairingInput::default()
            },
            "https://build.test:3773/",
            "code",
        ),
    ] {
        let resolved = resolve_pairing_target(&input).unwrap();
        assert_eq!(resolved.http_base, base);
        assert_eq!(resolved.credential.expose(), token);
    }
    assert!(resolve_pairing_target(&PairingInput::default()).is_err());
    assert!(
        resolve_pairing_target(&PairingInput {
            pairing_url: Some("https://host.test".into()),
            ..PairingInput::default()
        })
        .is_err()
    );
}

#[test]
fn pairing_checks_descriptor_identity_known_addresses_and_compatibility_before_exchange() {
    futures::executor::block_on(async {
        for scenario in [
            "mismatch",
            "incompatible",
            "known-address",
            "discovery-failed",
        ] {
            let platform = FakePlatform::default();
            let mut doc = descriptor(if scenario == "incompatible" {
                Some(3)
            } else {
                Some(2)
            });
            if scenario == "mismatch" {
                doc["environmentId"] = "other".into();
            }
            if scenario == "discovery-failed" {
                platform.push_http(Err(t3_client::platform::PlatformError::Network(
                    "unavailable".into(),
                )));
            } else {
                platform.push_http(response(doc));
            }
            let input = PairingInput {
                host: Some("https://host.test".into()),
                pairing_code: Some("one-time-code".into()),
                expected_environment: if scenario == "mismatch" {
                    Some(identity().environment)
                } else {
                    None
                },
                ..PairingInput::default()
            };
            let prepared = prepare_pairing_registration(&platform, &input, |_| {
                if scenario == "known-address" {
                    Err(ConnectionFailure::new(
                        FailureKind::Configuration,
                        "new address",
                    ))
                } else {
                    Ok(())
                }
            })
            .await;
            assert!(prepared.is_err());
            assert_eq!(platform.http_requests().len(), 1);
            assert!(platform.http_requests()[0].body.is_empty());
        }
    });
}

#[test]
fn pairing_saves_updatable_older_host_and_preserves_credential_on_metadata_edit() {
    futures::executor::block_on(async {
        let platform = FakePlatform::default();
        let mut old = descriptor(Some(1));
        old["capabilities"]["serverSelfUpdate"] = "boot-service".into();
        platform.push_http(response(old));
        platform.push_host(Ok(HostReply::Document(
            json!({"label":"Test Client", "deviceType":"desktop"}),
        )));
        platform.push_http(response(json!({"access_token":"paired-token", "issued_token_type":"urn:ietf:params:oauth:token-type:access_token", "token_type":"Bearer", "expires_in":3600,"scope":"environment:read"})));
        let prepared = prepare_pairing_registration(
            &platform,
            &PairingInput {
                host: Some("https://host.test".into()),
                pairing_code: Some("code".into()),
                ..PairingInput::default()
            },
            |_| Ok(()),
        )
        .await
        .unwrap();
        assert_eq!(
            prepared.registration.routes[0].id,
            "bearer:environment-1:https://host.test"
        );
        let updated = prepare_bearer_update(
            &identity().environment,
            " Renamed ",
            "wss://new.test/path#hash",
            Some(&prepared.profile),
            Some(&prepared.credential),
        )
        .unwrap();
        assert_eq!(updated.registration.label, "Renamed");
        assert_eq!(
            updated.registration.routes[0].id,
            prepared.registration.routes[0].id
        );
        assert_eq!(
            updated.registration.routes[0].http_base,
            "https://new.test/"
        );
        assert_eq!(updated.credential, prepared.credential);
    });
}

struct OpenScriptFactory {
    rpc: RefCell<Option<ScriptedRpc>>,
    opens: Rc<std::cell::Cell<usize>>,
}
impl SessionFactory for OpenScriptFactory {
    type Session = ScriptedRpc;
    fn open<'a>(
        &'a self,
        _: &'a t3_client::authorization::AuthorizedEnvironment,
    ) -> LocalBoxFuture<'a, Result<Self::Session, ConnectionFailure>> {
        async move {
            self.opens.set(self.opens.get() + 1);
            self.rpc
                .borrow_mut()
                .take()
                .ok_or_else(|| ConnectionFailure::new(FailureKind::Transport, "No session script"))
        }
        .boxed_local()
    }
    fn ready<'a>(
        &'a self,
        _: &'a Self::Session,
    ) -> LocalBoxFuture<'a, Result<(), ConnectionFailure>> {
        async { panic!("outdated host must not synchronize a normal session") }.boxed_local()
    }
}

fn update_input() -> t3_contracts::server::ServerSelfUpdateInput {
    serde_json::from_value(json!({"targetVersion":"2.0.0"})).unwrap()
}

#[test]
fn outdated_host_updates_over_bare_rpc_and_reenables_after_compatible_descriptor() {
    futures::executor::block_on(async {
        for progress in [false, true] {
            let platform = Rc::new(FakePlatform::default());
            primary_host(&platform);
            let mut old = descriptor(Some(1));
            old["capabilities"]["serverSelfUpdate"] = "boot-service".into();
            old["capabilities"]["serverSelfUpdateProgress"] = progress.into();
            platform.push_http(response(old));
            let mut resumed = descriptor(Some(2));
            resumed["serverVersion"] = "2.0.0".into();
            platform.push_http(response(resumed));
            let remote = service(platform.clone());
            remote.set_session(identity());
            let rpc = ScriptedRpc::default();
            rpc.scripts
                .borrow_mut()
                .push_back(t3_client::testing::RpcScript {
                    method: if progress {
                        "server.updateServerWithProgress"
                    } else {
                        "server.updateServer"
                    }
                    .into(),
                    input: json!({"targetVersion":"2.0.0"}),
                    replies: if progress {
                        vec![
                            Ok(json!({"type":"progress", "stage":"downloading"})),
                            Err(t3_client::ClientError::Disconnected),
                        ]
                    } else {
                        vec![Ok(
                            json!({"targetVersion":"2.0.0", "method":"boot-service"}),
                        )]
                    },
                });
            let opens = Rc::new(std::cell::Cell::new(0));
            let factory = OpenScriptFactory {
                rpc: RefCell::new(Some(rpc)),
                opens: opens.clone(),
            };
            let stages = RefCell::new(Vec::new());
            let enabled = Rc::new(std::cell::Cell::new(false));
            let finished = enabled.clone();
            let result = ConnectionResolver::new(platform.clone(), remote)
                .update_outdated_host(
                    &factory,
                    &[route("primary", ConnectionMode::Local)],
                    &identity(),
                    update_input(),
                    |stage| stages.borrow_mut().push(stage),
                    move || {
                        let finished = finished.clone();
                        async move {
                            finished.set(true);
                            Ok(())
                        }
                        .boxed_local()
                    },
                )
                .await
                .unwrap();
            assert!(
                matches!(result, OutdatedHostUpdateResult::Updated(ref result) if result.target_version.0 == "2.0.0")
            );
            assert_eq!(opens.get(), 1);
            assert!(enabled.get());
            assert_eq!(
                stages.borrow().last(),
                Some(&OutdatedHostUpdateStage::Resuming)
            );
            assert_eq!(stages.borrow().len(), if progress { 2 } else { 1 });
        }
    });
}

#[test]
fn outdated_host_requires_update_capability_before_opening_socket() {
    futures::executor::block_on(async {
        let platform = Rc::new(FakePlatform::default());
        primary_host(&platform);
        platform.push_http(response(descriptor(Some(1))));
        let remote = service(platform.clone());
        remote.set_session(identity());
        let opens = Rc::new(std::cell::Cell::new(0));
        let factory = OpenScriptFactory {
            rpc: RefCell::new(Some(ScriptedRpc::default())),
            opens: opens.clone(),
        };
        let result = ConnectionResolver::new(platform, remote)
            .update_outdated_host(
                &factory,
                &[route("primary", ConnectionMode::Local)],
                &identity(),
                update_input(),
                |_| {},
                || async { Ok(()) }.boxed_local(),
            )
            .await;
        assert!(result.is_err());
        assert_eq!(opens.get(), 0);
    });
}

#[test]
fn declared_http_errors_keep_safe_classification_status_and_trace() {
    use t3_client::authorization::AuthorizationError;
    for (raw, status, kind) in [
        (
            json!({"_tag":"EnvironmentAuthInvalidError","code":"auth_invalid","reason":"invalid_credential","traceId":"trace-1","dpopFailureReason":"time_window"}),
            401,
            FailureKind::Authentication,
        ),
        (
            json!({"_tag":"EnvironmentRequestInvalidError","code":"invalid_request","reason":"invalid_scope","traceId":"trace-1"}),
            400,
            FailureKind::Configuration,
        ),
        (
            json!({"_tag":"EnvironmentOperationForbiddenError","code":"operation_forbidden","reason":"current_session_revoke_not_allowed","traceId":"trace-1"}),
            403,
            FailureKind::Permission,
        ),
        (
            json!({"_tag":"EnvironmentInternalError","code":"internal_error","reason":"internal_error","traceId":"trace-1"}),
            500,
            FailureKind::RemoteUnavailable,
        ),
    ] {
        let error = AuthorizationError::Remote {
            status,
            details: Sensitive(Box::new(serde_json::from_value(raw).unwrap())),
        };
        let failure = map_remote_dpop_environment_error(&error);
        assert_eq!(failure.kind, kind);
        assert_eq!(failure.http_status, Some(status));
        assert_eq!(failure.trace_id.as_deref(), Some("trace-1"));
        if status == 401 {
            assert_eq!(failure.cause, Some(FailureCause::ClockSkew));
        }
    }
}
