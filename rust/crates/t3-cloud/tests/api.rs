#![cfg(feature = "auth")]
#![allow(clippy::unwrap_used)]
mod support;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use support::*;
use t3_cloud::{
    Error, Result, Secret, api::*, http::Sensitive, managed::*, reachability::*, store::Secrets,
    tunnel::ConnectorFacts,
};
use t3_contracts::{auth::AuthEnvironmentScope as Scope, environment_http::*, relay::*};
use tokio::sync::{Mutex, broadcast, watch};

struct Runtime {
    changes: Mutex<Vec<bool>>,
    notices: broadcast::Sender<RuntimeNotice>,
}
#[async_trait]
impl ManagedEndpoint for Runtime {
    async fn apply(
        &self,
        config: Option<Sensitive<RelayManagedEndpointRuntimeConfig>>,
    ) -> Result<EndpointStatus> {
        self.changes.lock().await.push(config.is_some());
        Ok(if config.is_some() {
            EndpointStatus::Running {
                provider_kind: RelayManagedEndpointProviderKind::CloudflareTunnel,
                pid: 42,
                tunnel_id: Some("tunnel-1".into()),
                tunnel_name: None,
            }
        } else {
            EndpointStatus::Disabled
        })
    }
    async fn inspect(&self) -> Option<ConnectorFacts> {
        Some(ConnectorFacts {
            pid: Some(42),
            ready_connections: Some(2),
            hostname: Some("env.test".into()),
            http2: true,
            ..Default::default()
        })
    }
    fn subscribe(&self) -> broadcast::Receiver<RuntimeNotice> {
        self.notices.subscribe()
    }
}
struct Probe;
#[async_trait]
impl ReachabilityProbe for Probe {
    async fn endpoint(&self, host: &str) -> EndpointProbe {
        assert_eq!(host, "env.test");
        EndpointProbe::Status {
            status: 200,
            cloudflare_error: None,
        }
    }
    async fn edge_reachable(&self) -> bool {
        panic!("healthy endpoint needs no edge probe")
    }
    async fn exit_node(&self) -> Option<t3_cloud::direct::ExitNode> {
        panic!("healthy endpoint needs no exit-node probe")
    }
}
fn context(scopes: Vec<Scope>) -> RequestContext {
    RequestContext {
        request_url: Secret::new("http://127.0.0.1:3773/api/connect/link-state"),
        subject: "desktop-bootstrap".into(),
        scopes,
        forwarded_authority: false,
    }
}
async fn service() -> (
    Arc<CloudService>,
    Arc<Runtime>,
    Arc<MemorySecrets>,
    Arc<Relay>,
    tempfile::TempDir,
) {
    service_with_policy(watch::channel(true).1).await
}
async fn service_with_policy(
    connect_allowed: watch::Receiver<bool>,
) -> (
    Arc<CloudService>,
    Arc<Runtime>,
    Arc<MemorySecrets>,
    Arc<Relay>,
    tempfile::TempDir,
) {
    let (link, secrets, relay, time, _) = cloud().await;
    let runtime = Arc::new(Runtime {
        changes: Mutex::new(Vec::new()),
        notices: broadcast::channel(10).0,
    });
    let (issuer, directory) = issuer().await;
    let service = CloudService::new(CloudServices {
        link,
        secrets: secrets.clone(),
        runtime: runtime.clone(),
        reachability: Arc::new(ConnectReachability::new(Arc::new(Probe), time)),
        issuer: Arc::new(issuer),
        origin: serde_json::from_value(json!({"localHttpHost":"127.0.0.1","localHttpPort":3773}))
            .unwrap(),
        connect_allowed,
    })
    .unwrap();
    (service, runtime, secrets, relay, directory)
}

#[tokio::test(start_paused = true)]
async fn mint_policy_refuses_before_spending_proofs_and_observes_live_changes() {
    let (allowed, receiver) = watch::channel(false);
    let (service, _, secrets, relay, _directory) = service_with_policy(receiver).await;
    let signer = t3_cloud::crypto::EnvironmentKey::generate().unwrap();
    secrets
        .set(t3_cloud::cloud::MINT_KEY, signer.public_key.as_bytes())
        .await
        .unwrap();
    let proof = signer.sign(t3_cloud::crypto::MINT_REQUEST,&json!({
        "iss":"https://issuer.test","aud":"t3-env:env-one","sub":"user-one",
        "iat":NOW/1000,"exp":NOW/1000+120,"jti":"policy-jti","nonce":"policy-nonce",
        "environmentId":"env-one","scope":["environment:connect"],"clientProofKeyThumbprint":"policy-key","cnf":{"jkt":"policy-key"}
    })).unwrap();
    let input = || Sensitive::new(serde_json::from_value(json!({"proof":proof.expose()})).unwrap());
    assert!(matches!(
        service.mint_credential(input()).await,
        Err(Error::Disabled)
    ));
    assert!(
        secrets
            .get("cloud-mint-jti-policy-jti")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        secrets
            .get("cloud-mint-nonce-policy-nonce")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        service
            .link_state(&context(vec![Scope::RelayRead]))
            .await
            .unwrap()
            .linked
    );
    allowed.send_replace(true);
    assert!(service.mint_credential(input()).await.is_ok());
    assert!(
        secrets
            .get("cloud-mint-jti-policy-jti")
            .await
            .unwrap()
            .is_some()
    );
    allowed.send_replace(false);
    assert!(matches!(
        service.mint_credential(input()).await,
        Err(Error::Disabled)
    ));
    relay.done().await;
}
#[tokio::test]
async fn cloud_routes_authorize_activate_report_and_unlink_without_disclosing_secrets() {
    let (service, runtime, secrets, relay, _directory) = service().await;
    assert!(service.link_state(&context(vec![])).await.is_err());
    let state = service
        .link_state(&context(vec![Scope::RelayRead]))
        .await
        .unwrap();
    assert!(state.linked);
    assert_eq!(state.linkable_here, Some(Some(true)));
    assert_eq!(state.tunnel_hostname, Some(Some("env.test".into())));
    let input=serde_json::from_value(json!({"relayUrl":"https://relay.test","relayIssuer":"https://issuer.test","cloudUserId":"user-one","environmentCredential":"marker-rotated","cloudMintPublicKey":t3_cloud::crypto::EnvironmentKey::generate().unwrap().public_key,"endpointRuntime":support::runtime()})).unwrap();
    relay
        .add(|request| {
            assert_eq!(
                request.url.path(),
                "/v1/environments/env-one/tunnel/recovery"
            );
            assert_eq!(
                request.headers["authorization"].expose(),
                "Bearer marker-rotated"
            );
            Ok(response(200, json!({"status":"ready"})))
        })
        .await;
    let result = service
        .relay_config(&context(vec![Scope::RelayWrite]), Sensitive::new(input))
        .await
        .unwrap();
    assert_eq!(result.endpoint_runtime_status["pid"], 42);
    assert_eq!(*runtime.changes.lock().await, vec![false, true]);
    assert!(
        secrets
            .get("cloud-endpoint-confirmed-origin")
            .await
            .unwrap()
            .is_some()
    );
    assert!(matches!(
        service
            .check_reachability(&context(vec![Scope::RelayRead]))
            .await
            .unwrap(),
        EnvironmentConnectReachability::Reachable { .. }
    ));
    secrets.set(t3_cloud::cloud::HOLD, b"true").await.unwrap();
    service
        .unlink(&context(vec![Scope::RelayWrite]))
        .await
        .unwrap();
    assert!(secrets.get(t3_cloud::cloud::HOLD).await.unwrap().is_none());
    assert!(!service.state().await.unwrap().linked);
    let failure = HttpFailure::from(Error::Remote {
        rejection: t3_cloud::Rejection::Forbidden,
        retry_after_ms: None,
    });
    assert_eq!(failure.status, 403);
    let reply = reply(Ok(Secret::new("marker-private"))).unwrap();
    assert!(!format!("{reply:?}").contains("marker-private"));
    assert_eq!(reply.headers["cache-control"], "no-store");
    relay.done().await;
}
#[tokio::test]
async fn hosted_approval_requires_matching_active_host_and_keeps_peer_links_local() {
    let (service, _, secrets, _, _directory) = service().await;
    let redirect = service
        .approval_redirect(
            "https://env.test",
            "?state=marker-state&environment=attacker&client_id=marker-client",
            false,
            "https://app.test",
        )
        .await
        .unwrap();
    let url = url::Url::parse(redirect.expose()).unwrap();
    assert_eq!(url.path(), "/approve-agent");
    let query = url.query_pairs().collect::<Vec<_>>();
    assert_eq!(query.iter().filter(|(k, _)| k == "environment").count(), 1);
    assert!(
        query
            .iter()
            .any(|(k, v)| k == "environment" && v == "env-one")
    );
    assert!(!format!("{redirect:?}").contains("marker-state"));
    for (issuer, peer) in [("https://other.test", false), ("https://env.test", true)] {
        assert_eq!(
            service
                .approval_redirect(issuer, "?state=x", peer, "https://app.test")
                .await
                .unwrap()
                .expose(),
            "/connect-agent?state=x"
        );
    }
    secrets
        .set(t3_cloud::cloud::REVOKED, b"true")
        .await
        .unwrap();
    assert_eq!(
        service
            .approval_redirect("https://env.test", "", false, "https://app.test")
            .await
            .unwrap()
            .expose(),
        "/connect-agent"
    );
}
#[tokio::test]
async fn publish_only_preferences_do_not_require_or_contact_a_relay() {
    let (service, _, secrets, relay, _directory) = service().await;
    service.link.unlink().await.unwrap();
    let state = service
        .preferences(
            &context(vec![Scope::RelayWrite]),
            EnvironmentCloudPreferencesRequest {
                publish_agent_activity: true,
                hold_webhooks_while_offline: None,
            },
        )
        .await
        .unwrap();
    assert!(state.publish_agent_activity);
    assert!(secrets.get(t3_cloud::cloud::HOLD).await.unwrap().is_none());
    relay.done().await;
}
