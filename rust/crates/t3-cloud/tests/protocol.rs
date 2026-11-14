#![allow(clippy::unwrap_used)]
mod support;
use async_trait::async_trait;
use serde_json::{Value, json};
use support::*;
use t3_cloud::{
    Clock, Error, Rejection, Result, Secret,
    cloud::*,
    crypto::{self, EnvironmentKey},
    http::{Client, Sensitive},
    store::{Secrets, consume_replay},
};
use t3_contracts::relay::*;

fn health_claims() -> Value {
    json!({"iss":"https://issuer.test","aud":"t3-env:env-one","sub":"user-one","jti":"health-jti","iat":NOW/1000,"exp":NOW/1000+120,"environmentId":"env-one","nonce":"health-nonce","scope":["environment:status"]})
}
#[cfg(feature = "auth")]
fn mint_claims() -> Value {
    let mut v = health_claims();
    v["scope"] = json!(["environment:connect"]);
    v["clientProofKeyThumbprint"] = json!("client-key");
    v["cnf"] = json!({"jkt":"client-key"});
    v
}
#[cfg(feature = "auth")]
#[tokio::test(start_paused = true)]
async fn signed_health_and_mint_check_identity_binding_and_consume_both_replay_guards() {
    let (cloud, secrets, _, _, mint) = cloud().await;
    let (issuer, _auth_directory) = issuer().await;
    let proof = mint.sign(crypto::HEALTH_REQUEST, &health_claims()).unwrap();
    let answer = cloud.health(proof.clone()).await.unwrap();
    let verified: RelayEnvironmentHealthResponseProofPayload = crypto::verify(
        &cloud.signing_key.public_key,
        &Secret::new(&answer.expose().proof.0),
        crypto::HEALTH_RESPONSE,
        "t3-env:env-one",
        "https://issuer.test",
        NOW / 1000,
    )
    .unwrap();
    assert_eq!(verified.request_nonce.0, "health-nonce");
    assert_eq!(verified.descriptor, descriptor("env-one"));
    assert!(matches!(cloud.health(proof).await, Err(Error::Replay)));
    let mut alternate = health_claims();
    alternate["jti"] = json!("new-jti");
    assert!(matches!(
        cloud
            .health(mint.sign(crypto::HEALTH_REQUEST, &alternate).unwrap())
            .await,
        Err(Error::Replay)
    ));
    assert!(
        secrets
            .get("cloud-health-jti-new-jti")
            .await
            .unwrap()
            .is_some()
    );
    let proof = mint.sign(crypto::MINT_REQUEST, &mint_claims()).unwrap();
    let answer = cloud.mint(proof.clone(), &issuer).await.unwrap();
    let verified: RelayEnvironmentMintResponseProofPayload = crypto::verify(
        &cloud.signing_key.public_key,
        &Secret::new(&answer.expose().proof.0),
        crypto::MINT_RESPONSE,
        "t3-env:env-one",
        "https://issuer.test",
        NOW / 1000,
    )
    .unwrap();
    assert_eq!(verified.credential, answer.expose().credential);
    assert_eq!(verified.client_proof_key_thumbprint.0, "client-key");
    assert_eq!(verified.exp, NOW / 1000 + 120);
    assert!(!format!("{answer:?}").contains(&answer.expose().credential.0));
    assert!(matches!(
        cloud.mint(proof, &issuer).await,
        Err(Error::Replay)
    ));
}

#[cfg(feature = "auth")]
#[tokio::test(start_paused = true)]
async fn rejects_signed_wrong_account_environment_scope_clock_key_and_cnf_without_spending_proof() {
    let (cloud, secrets, _, _, mint) = cloud().await;
    let (issuer, _auth_directory) = issuer().await;
    for (field, value) in [
        ("iss", json!("https://other.test")),
        ("aud", json!("t3-env:other")),
        ("sub", json!("other-user")),
        ("environmentId", json!("other-env")),
        ("scope", json!([])),
        (
            "scope",
            json!(["environment:connect", "environment:status"]),
        ),
        ("scope", json!(["environment:status"])),
        ("cnf", json!({"jkt":"different-key"})),
        ("exp", json!(NOW / 1000 + 301)),
        ("exp", json!(NOW / 1000)),
        ("exp", json!(NOW / 1000 - 60)),
        ("iat", json!(NOW / 1000 + 61)),
        ("iat", json!(NOW / 1000 - 361)),
        ("nonce", json!("../marker-secret")),
    ] {
        let mut payload = mint_claims();
        payload[field] = value;
        let error = cloud
            .mint(mint.sign(crypto::MINT_REQUEST, &payload).unwrap(), &issuer)
            .await
            .err()
            .unwrap();
        assert!(matches!(error, Error::Proof), "{field}: {error:?}");
        assert!(!format!("{error:?} {error}").contains("marker-secret"));
    }
    assert!(
        secrets
            .get("cloud-mint-jti-health-jti")
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        cloud
            .mint(
                EnvironmentKey::generate()
                    .unwrap()
                    .sign(crypto::MINT_REQUEST, &mint_claims())
                    .unwrap(),
                &issuer
            )
            .await,
        Err(Error::Proof)
    ));
    assert!(matches!(
        cloud
            .mint(
                mint.sign(crypto::HEALTH_REQUEST, &mint_claims()).unwrap(),
                &issuer
            )
            .await,
        Err(Error::Proof)
    ));
    // The same valid proof still works after rejected attempts.
    cloud
        .mint(
            mint.sign(crypto::MINT_REQUEST, &mint_claims()).unwrap(),
            &issuer,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn replay_is_atomic_across_racing_requests_and_survives_service_recreation() {
    let (cloud, secrets, relay, time, mint) = cloud().await;
    let proof = mint.sign(crypto::HEALTH_REQUEST, &health_claims()).unwrap();
    let (a, b) = tokio::join!(cloud.health(proof.clone()), cloud.health(proof.clone()));
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let recreated = CloudLink::new(descriptor("env-one"), secrets.clone(), relay, time)
        .await
        .unwrap();
    assert!(matches!(recreated.health(proof).await, Err(Error::Replay)));
    let (a, b) = tokio::join!(
        consume_replay(secrets.as_ref(), "mint", "same-jti", "same-nonce", "date"),
        consume_replay(secrets.as_ref(), "mint", "same-jti", "same-nonce", "date")
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
}

#[tokio::test]
async fn environment_key_creation_migration_races_and_lazy_corrupt_key_failure() {
    let store = MemorySecrets::default();
    let (a, b) = tokio::join!(EnvironmentKey::load(&store), EnvironmentKey::load(&store));
    assert_eq!(a.unwrap().public_key, b.unwrap().public_key);
    let store = MemorySecrets::default();
    store
        .set("cloud-link-ed25519-private-key", b"marker-invalid-private")
        .await
        .unwrap();
    store
        .set("cloud-link-ed25519-public-key", b"public")
        .await
        .unwrap();
    let key = EnvironmentKey::load(&store).await.unwrap();
    assert_eq!(key.public_key, "public");
    assert!(!format!("{key:?}").contains("marker-invalid-private"));
    let error = key.sign(crypto::LINK, &json!({})).unwrap_err();
    assert_eq!(format!("{error}"), "remote proof rejected");
    assert!(
        store
            .get("cloud-link-ed25519-key-pair")
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn fake_relay_checks_exact_registration_and_recovery_protocol() {
    let (cloud, secrets, relay, time, _) = cloud().await;
    let public = cloud.signing_key.public_key.clone();
    relay
        .add(move |request| {
            assert_eq!(request.method, reqwest::Method::POST);
            assert_eq!(
                request.url.as_str(),
                "https://relay.test/v1/environments/env-one/tunnel/recovery"
            );
            assert_eq!(
                request.headers["authorization"].expose(),
                "Bearer marker-environment-secret"
            );
            let body: RelayManagedEndpointRecoveryRegistrationRequest =
                serde_json::from_slice(&request.body).unwrap();
            let proof: RelayManagedEndpointRegisterProofPayload = crypto::verify(
                &public,
                &Secret::new(body.proof.0),
                crypto::RECOVERY,
                "t3-env:env-one",
                "https://issuer.test",
                NOW / 1000,
            )
            .unwrap();
            assert_eq!(proof.exp - proof.iat, 60);
            assert_eq!(proof.cloud_user_id, body.cloud_user_id);
            assert_eq!(proof.tunnel_id, body.tunnel_id);
            assert_eq!(proof.origin, body.origin);
            Ok(response(200, json!({"status":"recovery_required"})))
        })
        .await;
    assert_eq!(
        cloud
            .register(t3_cloud::direct::parse_local_origin("http://127.0.0.1:3773").unwrap())
            .await
            .unwrap(),
        Registration::RecoveryRequired
    );
    let public = cloud.signing_key.public_key.clone();
    relay.add(move |request|{
        assert_eq!(request.url.path(),"/v1/environments/env-one/tunnel");assert_eq!(request.timeout,std::time::Duration::from_secs(120));
        let body:RelayManagedEndpointRecoveryRequest=serde_json::from_slice(&request.body).unwrap();
        let proof:RelayManagedEndpointRecoverProofPayload=crypto::verify(&public,&Secret::new(body.proof.0),crypto::RECOVERY,"t3-env:env-one","https://issuer.test",NOW/1000).unwrap();
        assert_eq!(proof.exp-proof.iat,60);assert_eq!(proof.origin,body.origin);
        Ok(response(200,json!({"endpoint":{"httpBaseUrl":"https://env.test","wsBaseUrl":"wss://env.test","providerKind":"cloudflare_tunnel"},"endpointRuntime":{"providerKind":"cloudflare_tunnel","connectorToken":"marker-new-connector","tunnelId":"tunnel-2"}})))
    }).await;
    let recovered = cloud
        .recover(
            t3_cloud::direct::parse_local_origin("http://127.0.0.1:3773").unwrap(),
            Some(&Sensitive::new(runtime())),
        )
        .await
        .unwrap();
    assert!(!format!("{recovered:?}").contains("marker-new-connector"));
    assert!(
        String::from_utf8(secrets.get(RUNTIME).await.unwrap().unwrap())
            .unwrap()
            .contains("tunnel-2")
    );
    assert_eq!(time.now_ms(), NOW);
    relay.done().await;
}

#[tokio::test]
async fn relay_unreachable_keeps_credentials_and_401_marks_revoked_until_registration_succeeds() {
    let (cloud, secrets, relay, _, _) = cloud().await;
    for (status, result) in [
        (503, None),
        (401, Some(Registration::Revoked)),
        (200, Some(Registration::Ready)),
    ] {
        relay
            .add(move |_| {
                Ok(response(
                    status,
                    if status == 200 {
                        json!({"status":"ready"})
                    } else {
                        json!({"message":"marker-environment-secret"})
                    },
                ))
            })
            .await;
        let answer = cloud
            .register(t3_cloud::direct::parse_local_origin("http://127.0.0.1:3773").unwrap())
            .await;
        if let Some(expected) = result {
            assert_eq!(answer.unwrap(), expected);
        } else {
            assert!(matches!(
                answer,
                Err(Error::Remote {
                    rejection: Rejection::Unavailable,
                    ..
                })
            ));
        }
        assert_eq!(
            secrets.get(CREDENTIAL).await.unwrap().unwrap(),
            b"marker-environment-secret"
        );
        if status == 401 {
            assert_eq!(secrets.get(REVOKED).await.unwrap().unwrap(), b"true");
        }
    }
    assert!(secrets.get(REVOKED).await.unwrap().is_none());
    cloud.unlink().await.unwrap();
    assert!(secrets.get(CREDENTIAL).await.unwrap().is_none());
    assert_eq!(
        cloud
            .register(t3_cloud::direct::parse_local_origin("http://127.0.0.1:3773").unwrap())
            .await
            .unwrap(),
        Registration::NotLinked
    );
}

#[tokio::test]
async fn account_link_uses_fresh_challenge_borrows_relay_clock_and_installs_rotated_credential() {
    let (cloud, secrets, relay, time, mint) = cloud().await;
    let challenge = mint
        .sign("t3-link-challenge+jwt", &json!({"iat":NOW/1000+120}))
        .unwrap();
    let challenge_copy = challenge.clone();
    relay
        .add(move |r| {
            assert_eq!(r.url.path(), "/v1/client/environment-link-challenges");
            assert_eq!(
                r.headers["authorization"].expose(),
                "Bearer marker-account-secret"
            );
            Ok(response(
                200,
                json!({"challenge":challenge_copy.expose(),"expiresAt":"2027-01-15T08:05:00Z"}),
            ))
        })
        .await;
    let public = cloud.signing_key.public_key.clone();
    let mint_public = mint.public_key.clone();
    relay.add(move |r|{
        assert_eq!(r.url.path(),"/v1/client/environment-links");
        let body:RelayEnvironmentLinkRequest=serde_json::from_slice(&r.body).unwrap();
        let proof:RelayEnvironmentLinkProofPayload=crypto::verify(&public,&Secret::new(body.proof.0),crypto::LINK,"t3-env:env-one","https://issuer.test",NOW/1000+120).unwrap();
        assert_eq!(proof.challenge.0,challenge.expose());assert_eq!(proof.iat,NOW/1000+120);assert_eq!(proof.exp-proof.iat,300);assert_eq!(proof.scopes,vec![RelayEnvironmentLinkScope::AgentActivityNotifications,RelayEnvironmentLinkScope::ManagedTunnels]);
        Ok(response(200,json!({"ok":true,"cloudUserId":"user-one","environmentId":"env-one","endpoint":{"providerKind":"cloudflare_tunnel","httpBaseUrl":"https://env.test","wsBaseUrl":"wss://env.test"},"endpointRuntime":runtime(),"relayIssuer":"https://issuer.test","environmentCredential":"marker-rotated-secret","cloudMintPublicKey":mint_public})))
    }).await;
    let client = Client::relay(relay.clone(), "https://relay.test", time).unwrap();
    let request=Sensitive::new(serde_json::from_value(json!({"challenge":"old","relayIssuer":"https://issuer.test","endpoint":{"providerKind":"cloudflare_tunnel","httpBaseUrl":"http://127.0.0.1:3773","wsBaseUrl":"ws://127.0.0.1:3773"},"origin":{"localHttpHost":"127.0.0.1","localHttpPort":3773}})).unwrap());
    let linked = register_account(
        &client,
        &Secret::new("marker-account-secret"),
        &cloud,
        request,
        RelayEnvironmentLinkChallengeRequest {
            notifications_enabled: true,
            live_activities_enabled: true,
            managed_tunnels_enabled: true,
        },
        "http://127.0.0.1:3773",
    )
    .await
    .unwrap();
    assert!(!format!("{linked:?}").contains("marker-rotated-secret"));
    assert_eq!(
        secrets.get(CREDENTIAL).await.unwrap().unwrap(),
        b"marker-rotated-secret"
    );
    relay.done().await;
}

#[tokio::test]
async fn signed_activity_preferences_and_held_webhook_wake_match_relay_contract() {
    let (cloud, _, relay, _, _) = cloud().await;
    assert!(
        cloud
            .publish_activity("thread-one", None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(!cloud.wake_hooks().await.unwrap());
    relay
        .add(|r| {
            assert_eq!(r.url.path(), "/v1/environments/env-one/link-preferences");
            assert_eq!(
                serde_json::from_slice::<Value>(&r.body).unwrap(),
                json!({"holdWebhooksWhileOffline":true})
            );
            Ok(response(200, json!({"holdWebhooksWhileOffline":true})))
        })
        .await;
    cloud.preferences(true, true).await.unwrap();
    let public = cloud.signing_key.public_key.clone();
    relay
        .add(move |r| {
            assert_eq!(
                r.url.path(),
                "/v1/environments/env-one/threads/thread-one/agent-activity"
            );
            let body: RelayAgentActivityPublishRequest = serde_json::from_slice(&r.body).unwrap();
            let proof: RelayAgentActivityPublishProofPayload = crypto::verify(
                &public,
                &Secret::new(body.proof.0),
                crypto::ACTIVITY,
                "t3-env:env-one",
                "https://issuer.test",
                NOW / 1000,
            )
            .unwrap();
            assert_eq!(proof.state, body.state);
            assert_eq!(proof.thread_id.0, "thread-one");
            Ok(response(200, json!({"ok":true,"deliveries":[]})))
        })
        .await;
    assert!(
        cloud
            .publish_activity("thread-one", None)
            .await
            .unwrap()
            .unwrap()
            .ok
    );
    relay
        .add(|r| {
            assert_eq!(r.url.path(), "/v1/environments/env-one/hooks/wake");
            Ok(response(200, json!({"pending":true})))
        })
        .await;
    assert!(cloud.wake_hooks().await.unwrap());
    for (phase, detail, expected) in [
        (
            "failed",
            "marker-private-provider-error",
            Some("The agent run failed.".to_owned()),
        ),
        ("running", "   ", None),
        ("running", "x", Some("x".repeat(160))),
    ] {
        let expected_detail = expected;
        let public_key = cloud.signing_key.public_key.clone();
        relay
            .add(move |request| {
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                let proof: RelayAgentActivityPublishProofPayload = crypto::verify(
                    &public_key,
                    &Secret::new(body["proof"].as_str().unwrap()),
                    crypto::ACTIVITY,
                    "t3-env:env-one",
                    "https://issuer.test",
                    NOW / 1000,
                )
                .unwrap();
                assert_eq!(serde_json::to_value(proof.state).unwrap(), body["state"]);
                assert_eq!(
                    body["state"].get("detail").and_then(Value::as_str),
                    expected_detail.as_deref()
                );
                assert!(
                    !String::from_utf8(request.body)
                        .unwrap()
                        .contains("marker-private-provider-error")
                );
                Ok(response(200, json!({"ok":true,"deliveries":[]})))
            })
            .await;
        let detail = if detail == "x" {
            "x".repeat(200)
        } else {
            detail.to_owned()
        };
        let mut state: RelayAgentActivityState = serde_json::from_value(json!({"environmentId":"env-one","threadId":"thread-one","threadTitle":"Thread","projectTitle":"Project","phase":phase,"headline":"Working","modelTitle":"Test","deepLink":"t3://thread-one","updatedAt":"2027-01-15T08:00:00.000Z"})).unwrap();
        state.detail = Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
            detail,
        )));
        cloud
            .publish_activity("thread-one", Some(state))
            .await
            .unwrap();
    }
    relay.done().await;
}

#[tokio::test]
async fn install_cannot_switch_accounts_without_unlinking_and_clears_old_origin_confirmation() {
    let (cloud, secrets, _, _, mint) = cloud().await;
    secrets
        .set("cloud-endpoint-confirmed-origin", b"old-confirmation")
        .await
        .unwrap();
    let make = |user: &str| {
        Sensitive::new(serde_json::from_value(json!({"relayUrl":"https://relay.test","cloudUserId":user,"environmentCredential":"marker-new-secret","cloudMintPublicKey":mint.public_key,"endpointRuntime":runtime()})).unwrap())
    };
    assert!(matches!(
        cloud.install(make("other-account")).await,
        Err(Error::Identity)
    ));
    assert_eq!(
        secrets.get(CREDENTIAL).await.unwrap().unwrap(),
        b"marker-environment-secret"
    );
    cloud.install(make("user-one")).await.unwrap();
    assert!(
        secrets
            .get("cloud-endpoint-confirmed-origin")
            .await
            .unwrap()
            .is_none()
    );
    cloud.unlink().await.unwrap();
    cloud.install(make("other-account")).await.unwrap();
}

#[tokio::test]
async fn confirmed_origin_fallback_and_shutdown_release_are_bound_to_cli_desire_and_runtime() {
    let (cloud, _, relay, _, _) = cloud().await;
    let origin = t3_cloud::direct::parse_local_origin("http://127.0.0.1:3773").unwrap();
    assert!(cloud.confirmed_runtime(&origin).await.unwrap().is_none());
    cloud
        .confirm_origin(&Sensitive::new(runtime()), origin.clone())
        .await
        .unwrap();
    assert!(cloud.confirmed_runtime(&origin).await.unwrap().is_some());
    assert!(
        cloud
            .confirmed_runtime(
                &t3_cloud::direct::parse_local_origin("http://127.0.0.1:3774").unwrap()
            )
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !cloud
            .release_managed_tunnel(Some(&Secret::new("marker-account-secret")))
            .await
            .unwrap()
    );
    cloud
        .set_desired_link(Some(DesiredLinkMode::PublishOnly))
        .await
        .unwrap();
    assert!(
        !cloud
            .release_managed_tunnel(Some(&Secret::new("marker-account-secret")))
            .await
            .unwrap()
    );
    cloud
        .set_desired_link(Some(DesiredLinkMode::Managed))
        .await
        .unwrap();
    assert!(!cloud.release_managed_tunnel(None).await.unwrap());
    relay
        .add(|r| {
            assert_eq!(r.method, reqwest::Method::DELETE);
            assert_eq!(r.url.path(), "/v1/client/environment-links/env-one/tunnel");
            assert_eq!(
                r.headers["authorization"].expose(),
                "Bearer marker-account-secret"
            );
            Ok(response(200, json!({"ok":true})))
        })
        .await;
    assert!(
        cloud
            .release_managed_tunnel(Some(&Secret::new("marker-account-secret")))
            .await
            .unwrap()
    );
    relay.done().await;
}

#[tokio::test]
async fn held_hook_proofs_allow_offline_hold_but_bind_every_delivery_header() {
    let (cloud, _, _, _, mint) = cloud().await;
    let payload = json!({"iss":"https://issuer.test","aud":"t3-env:env-one","sub":"env-one","jti":"delivery-jti","iat":NOW/1000-24*60*60,"exp":NOW/1000+60,"environmentId":"env-one","deliveryId":"delivery-one","receivedAt":"2027-01-14T08:00:00.000Z","hookId":"hook-one"});
    let proof = mint.sign(crypto::HOOK, &payload).unwrap();
    cloud
        .verify_hook(
            proof.clone(),
            "hook-one",
            "delivery-one",
            "2027-01-14T08:00:00.000Z",
        )
        .await
        .unwrap();
    for (hook, delivery, received) in [
        ("wrong", "delivery-one", "2027-01-14T08:00:00.000Z"),
        ("hook-one", "wrong", "2027-01-14T08:00:00.000Z"),
        ("hook-one", "delivery-one", "wrong"),
    ] {
        assert!(matches!(
            cloud
                .verify_hook(proof.clone(), hook, delivery, received)
                .await,
            Err(Error::Proof)
        ));
    }
}

struct HeldRelay(
    tokio::sync::mpsc::UnboundedSender<tokio::sync::oneshot::Sender<t3_cloud::http::Response>>,
);
#[async_trait]
impl t3_cloud::http::Transport for HeldRelay {
    async fn send(&self, _: t3_cloud::http::Request) -> Result<t3_cloud::http::Response> {
        let (send, receive) = tokio::sync::oneshot::channel();
        self.0.send(send).unwrap();
        receive.await.map_err(|_| Error::Protocol)
    }
}
#[tokio::test]
async fn stale_registration_and_recovery_responses_cannot_revoke_or_overwrite_a_relink() {
    for recovering in [false, true] {
        let (_, secrets, _, clock, mint) = cloud().await;
        let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
        let cloud = std::sync::Arc::new(
            CloudLink::new(
                descriptor("env-one"),
                secrets.clone(),
                std::sync::Arc::new(HeldRelay(send)),
                clock,
            )
            .await
            .unwrap(),
        );
        let child = cloud.clone();
        let task = tokio::spawn(async move {
            let origin = t3_cloud::direct::parse_local_origin("http://127.0.0.1:3773").unwrap();
            if recovering {
                child
                    .recover(origin, None)
                    .await
                    .map(|_| Registration::Ready)
            } else {
                child.register(origin).await
            }
        });
        let answer = receive.recv().await.unwrap();
        let mut replacement = runtime();
        replacement.connector_token =
            t3_contracts::base_schemas::TrimmedNonEmptyString("marker-relinked-connector".into());
        cloud.install(Sensitive::new(serde_json::from_value(json!({"relayUrl":"https://relay.test","relayIssuer":"https://issuer.test","cloudUserId":"user-one","environmentCredential":"marker-relinked-credential","cloudMintPublicKey":mint.public_key,"endpointRuntime":replacement})).unwrap())).await.unwrap();
        answer.send(if recovering {response(200,json!({"endpoint":{"providerKind":"cloudflare_tunnel","httpBaseUrl":"https://env.test","wsBaseUrl":"wss://env.test"},"endpointRuntime":runtime()}))}else{response(401,json!({}))}).unwrap();
        let result = task.await.unwrap();
        if recovering {
            assert!(matches!(result, Err(Error::Superseded)));
        } else {
            assert_eq!(result.unwrap(), Registration::Superseded);
        }
        assert!(secrets.get(REVOKED).await.unwrap().is_none());
        assert_eq!(
            secrets.get(CREDENTIAL).await.unwrap().unwrap(),
            b"marker-relinked-credential"
        );
        assert!(
            String::from_utf8(secrets.get(RUNTIME).await.unwrap().unwrap())
                .unwrap()
                .contains("marker-relinked-connector")
        );
    }
}

#[tokio::test]
async fn link_proof_refuses_forwarded_authority_and_non_desktop_remote_hosts() {
    let (cloud, _, _, _, _) = cloud().await;
    let request=Sensitive::new(serde_json::from_value(json!({"challenge":"challenge","relayIssuer":"https://issuer.test","endpoint":{"providerKind":"manual","httpBaseUrl":"https://env.test","wsBaseUrl":"wss://env.test"},"origin":{"localHttpHost":"127.0.0.1","localHttpPort":3773}})).unwrap());
    assert!(
        cloud
            .link_proof_for_client(
                &request,
                LinkRequestContext {
                    request_url: "http://127.0.0.1:3773",
                    desktop_bootstrap: true,
                    forwarded_authority: true
                }
            )
            .is_err()
    );
    assert!(
        cloud
            .link_proof_for_client(
                &request,
                LinkRequestContext {
                    request_url: "http://wsl.test:3773",
                    desktop_bootstrap: false,
                    forwarded_authority: false
                }
            )
            .is_err()
    );
    let proof = cloud
        .link_proof_for_client(
            &request,
            LinkRequestContext {
                request_url: "http://wsl.test:3773",
                desktop_bootstrap: true,
                forwarded_authority: false,
            },
        )
        .unwrap();
    let claims = crypto::unverified(proof.expose()).unwrap();
    assert_eq!(claims["scopes"], json!(["agent_activity_notifications"]));
}
