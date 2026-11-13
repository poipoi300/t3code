#![allow(clippy::unwrap_used)]
mod support;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::Ordering},
};
use support::*;
use t3_cloud::{
    Error, Result, Secret,
    http::{Request, Response, Sensitive, Transport},
    oauth::{PersistedToken, TokenManager},
    peer::*,
};
use t3_contracts::{
    peer_link::PeerLinkCreateInput,
    provider_policy::{ProviderInteractionMode, RuntimeMode},
};

fn input(url: &str) -> Sensitive<PeerLinkCreateInput> {
    Sensitive::new(
        serde_json::from_value(
            json!({"url":url,"pairingCode":"marker-pairing-code","access":"read-only"}),
        )
        .unwrap(),
    )
}
async fn sign_in(relay: &Relay) {
    relay
        .add(|r| {
            assert_eq!(r.url.path(), "/oauth/mcp/register");
            assert!(!r.headers.contains_key("authorization"));
            let body: Value = serde_json::from_slice(&r.body).unwrap();
            assert_eq!(body["software_id"], "t3code-peer-link");
            assert_eq!(
                body["redirect_uris"],
                json!(["http://127.0.0.1/t3code-peer-link"])
            );
            Ok(response(200, json!({"client_id":"client-one","client_name":"T3 peer","redirect_uris":["http://127.0.0.1/t3code-peer-link"],"grant_types":["authorization_code"],"response_types":["code"],"token_endpoint_auth_method":"none"})))
        })
        .await;
    relay
        .add(|r| {
            assert_eq!(r.url.path(), "/oauth/mcp/decision");
            let body: Value = serde_json::from_slice(&r.body).unwrap();
            assert_eq!(body["decision"]["code"], "marker-pairing-code");
            assert_eq!(body["authorization"]["code_challenge_method"], "S256");
            let mut url = url::Url::parse("http://127.0.0.1/t3code-peer-link").unwrap();
            url.query_pairs_mut()
                .append_pair("code", "marker-oauth-code")
                .append_pair("state", body["authorization"]["state"].as_str().unwrap());
            Ok(response(200, json!({"redirectTo":url.as_str()})))
        })
        .await;
    relay
        .add(|r| {
            assert_eq!(r.url.path(), "/oauth/mcp/token");
            let body: Value = serde_json::from_slice(&r.body).unwrap();
            assert_eq!(body["code"], "marker-oauth-code");
            assert!(body["code_verifier"].as_str().unwrap().len() >= 43);
            Ok(response(
                200,
                json!({"access_token":"marker-peer-token","expires_in":3600,"token_type":"Bearer","scope":"orchestration:read"}),
            ))
        })
        .await;
}
async fn peers() -> (
    Arc<PeerLinks>,
    Arc<Relay>,
    t3_persistence::Database,
    tempfile::TempDir,
) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.sqlite");
    let db = tokio::task::spawn_blocking(move || t3_persistence::Database::open(path))
        .await
        .unwrap()
        .unwrap();
    let relay = Arc::new(Relay::default());
    let peers = Arc::new(PeerLinks::new(
        db.clone(),
        Arc::new(MemorySecrets::default()),
        relay.clone(),
        Arc::new(Time::default()),
        descriptor("local"),
    ));
    peers.enabled.store(true, Ordering::Relaxed);
    (peers, relay, db, temp)
}

#[tokio::test]
async fn peer_link_persists_only_a_secret_reference_and_resolves_only_matching_fenced_identity() {
    let (peers, relay, db, _temp) = peers().await;
    relay
        .add(|r| {
            assert_eq!(r.url.path(), "/.well-known/t3/environment");
            Ok(response(
                200,
                serde_json::to_value(descriptor("remote")).unwrap(),
            ))
        })
        .await;
    sign_in(&relay).await;
    let link = peers
        .link(input("https://peer.test/path?ignored"))
        .await
        .unwrap();
    assert_eq!(link.environment_id.0, "remote");
    assert_eq!(link.urls[0].0, "https://peer.test");
    let stored = db
        .reader(|c| {
            Ok(c.query_row(
                "SELECT secret_name FROM peer_environment_links WHERE environment_id='remote'",
                [],
                |r| r.get::<_, String>(0),
            )?)
        })
        .await
        .unwrap();
    assert!(stored.starts_with("peer-link-"));
    assert!(!stored.contains("marker-peer-token"));
    relay
        .add(|r| {
            assert!(!r.headers.contains_key("authorization"));
            Ok(response(
                200,
                serde_json::to_value(descriptor("other")).unwrap(),
            ))
        })
        .await;
    assert!(peers.resolve("remote").await.is_err());
    relay
        .add(|_| {
            Ok(response(
                200,
                serde_json::to_value(descriptor("remote")).unwrap(),
            ))
        })
        .await;
    let resolved = peers.resolve("remote").await.unwrap();
    assert_eq!(resolved.token.expose(), "marker-peer-token");
    assert!(!format!("{resolved:?}").contains("marker-peer-token"));
    relay
        .add(|_| {
            let mut d = descriptor("remote");
            d.capabilities.link_fence = Some(false);
            Ok(response(200, serde_json::to_value(d).unwrap()))
        })
        .await;
    assert!(matches!(
        peers.resolve("remote").await,
        Err(Error::Identity)
    ));
    db.writer(|c| {
        c.execute("UPDATE peer_environment_links SET fenced=0", [])?;
        Ok(())
    })
    .await
    .unwrap();
    assert!(matches!(peers.resolve("remote").await, Err(Error::Expired)));
    assert!(peers.unlink("remote").await.unwrap());
    assert!(!peers.unlink("remote").await.unwrap());
    relay.done().await;
}

#[tokio::test]
async fn selected_environment_checks_addresses_before_any_pairing_code_leaves() {
    let (peers, relay, _, _temp) = peers().await;
    relay
        .add(|r| {
            assert_eq!(r.url.host_str(), Some("wrong.test"));
            assert!(r.body.is_empty());
            Ok(response(
                200,
                serde_json::to_value(descriptor("wrong")).unwrap(),
            ))
        })
        .await;
    relay
        .add(|r| {
            assert_eq!(r.url.host_str(), Some("right.test"));
            assert!(r.body.is_empty());
            Ok(response(
                200,
                serde_json::to_value(descriptor("right")).unwrap(),
            ))
        })
        .await;
    sign_in(&relay).await;
    let input=Sensitive::new(serde_json::from_value(json!({"url":"https://wrong.test","alternateUrls":["http://public.test","https://right.test","http://127.0.0.1:3773"],"pairingCode":"marker-pairing-code","access":"approval-required","expectedEnvironmentId":"right"})).unwrap());
    let link = peers.link(input).await.unwrap();
    assert_eq!(link.urls.len(), 1);
    assert_eq!(link.urls[0].0, "https://right.test");
    relay.done().await;
}

#[tokio::test]
async fn peer_sign_in_refuses_self_old_capabilities_and_redirects_with_wrong_state() {
    let (peers, relay, _, _temp) = peers().await;
    relay
        .add(|_| {
            Ok(response(
                200,
                serde_json::to_value(descriptor("local")).unwrap(),
            ))
        })
        .await;
    assert!(matches!(
        peers.link(input("https://self.test")).await,
        Err(Error::Identity)
    ));
    relay
        .add(|_| {
            let mut d = descriptor("remote");
            d.capabilities.mcp_mode_limit_header = None;
            Ok(response(200, serde_json::to_value(d).unwrap()))
        })
        .await;
    assert!(matches!(
        peers.link(input("https://old.test")).await,
        Err(Error::Identity)
    ));
    relay
        .add(|_| {
            Ok(response(
                200,
                serde_json::to_value(descriptor("remote")).unwrap(),
            ))
        })
        .await;
    relay
        .add(|_| Ok(response(200, json!({"client_id":"client-one","client_name":"T3 peer","redirect_uris":["http://127.0.0.1/t3code-peer-link"],"grant_types":["authorization_code"],"response_types":["code"],"token_endpoint_auth_method":"none"}))))
        .await;
    relay.add(|_|Ok(response(200,json!({"redirectTo":"https://evil.test/t3code-peer-link?code=marker-code&state=wrong"})))).await;
    let error = peers.link(input("https://peer.test")).await.unwrap_err();
    assert!(matches!(error, Error::Proof));
    assert!(!format!("{error:?} {error}").contains("marker-code"));
    relay.done().await;
}

#[tokio::test]
async fn mcp_reopens_lost_sessions_and_carries_the_callers_limits_in_every_exchange() {
    let (peers, relay, _, _temp) = peers().await;
    relay
        .add(|_| {
            Ok(response(
                200,
                serde_json::to_value(descriptor("remote")).unwrap(),
            ))
        })
        .await;
    sign_in(&relay).await;
    peers.link(input("https://peer.test")).await.unwrap();
    relay
        .add(|_| {
            Ok(response(
                200,
                serde_json::to_value(descriptor("remote")).unwrap(),
            ))
        })
        .await;
    for n in 0..6 {
        relay.add(move|r|{
            assert_eq!(r.url.path(),"/mcp");assert_eq!(r.headers["authorization"].expose(),"Bearer marker-peer-token");assert_eq!(r.headers["t3-mode-limit"].expose(),"approval-required/plan");
            let body:Value=serde_json::from_slice(&r.body).unwrap();
            match n {
                0|3=>{assert_eq!(body["method"],"initialize");let mut r=response(200,json!({"result":{}}));r.headers.insert("mcp-session-id".into(),format!("session-{n}"));Ok(r)},
                1|4=>{assert_eq!(body["method"],"notifications/initialized");Ok(response(202,json!({})))},
                2=>Ok(response(404,json!({}))),
                _=>{assert_eq!(body["params"]["arguments"]["clientRequestId"],scoped_request_id("caller","retry-one"));assert!(body["params"]["arguments"].get("environmentId").is_none());Ok(Response {status:200,headers:BTreeMap::from([("content-type".into(),"text/event-stream".into())]),body:b"event: message\ndata: {\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"{\\\"ok\\\":true}\"}]}}\n\n".to_vec()})}
            }
        }).await;
    }
    let client = PeerMcpClient::new(peers);
    let result = client
        .call::<Value>(PeerCall {
            environment_id: "remote",
            tool: "t3_thread_read",
            arguments: json!({"environmentId":"remote","clientRequestId":"retry-one"}),
            limits: CallLimits {
                runtime_mode: RuntimeMode::ApprovalRequired,
                interaction_mode: ProviderInteractionMode::Plan,
            },
            namespace: "caller",
            sender: None,
            linked_origin: false,
        })
        .await
        .unwrap();
    assert_eq!(result.expose(), &json!({"ok":true}));
    assert_eq!(forwarded_wait_chunk(60_000), 50_000);
    assert_ne!(
        scoped_request_id("a", "same"),
        scoped_request_id("b", "same")
    );
    relay.done().await;
}

fn token() -> PersistedToken {
    PersistedToken {
        access_token: Secret::new("marker-old-access"),
        refresh_token: Secret::new("marker-refresh"),
        expires_at_epoch_ms: NOW + 299_999,
        identity: Some("account".into()),
    }
}
#[tokio::test]
async fn oauth_refresh_is_serialized_rotates_tokens_preserves_identity_and_classifies_rejections() {
    let secrets = Arc::new(MemorySecrets::default());
    let relay = Arc::new(Relay::default());
    let time = Arc::new(Time::default());
    let manager = TokenManager::new(
        secrets.clone(),
        relay.clone(),
        time,
        "https://oauth.test/token",
        "client".into(),
    )
    .unwrap();
    manager.store(&token()).await.unwrap();
    relay.add(|r|{assert_eq!(r.url.path(),"/token");let fields=url::form_urlencoded::parse(&r.body).collect::<BTreeMap<_,_>>();assert_eq!(fields["refresh_token"],"marker-refresh");assert_eq!(fields["grant_type"],"refresh_token");assert_eq!(r.timeout,std::time::Duration::from_secs(10));Ok(response(200,json!({"access_token":"marker-new-access","refresh_token":"marker-new-refresh","expires_in":3600,"token_type":"Bearer","scope":"orchestration:read"})))}).await;
    let (a, b) = tokio::join!(manager.get_existing(), manager.get_existing());
    for token in [a.unwrap().unwrap(), b.unwrap().unwrap()] {
        assert_eq!(token.access_token.expose(), "marker-new-access");
        assert_eq!(token.refresh_token.expose(), "marker-new-refresh");
        assert_eq!(token.identity.as_deref(), Some("account"));
        assert!(!format!("{token:?}").contains("marker-"));
    }
    for status in [400, 401, 408, 429, 503] {
        manager.store(&token()).await.unwrap();
        relay
            .add(move |_| {
                Ok(response(
                    status,
                    json!({"error_description":"marker-refresh"}),
                ))
            })
            .await;
        let error = manager.get_existing().await.unwrap_err();
        assert!(!format!("{error:?} {error}").contains("marker-refresh"));
        if matches!(status, 400 | 401) {
            assert!(matches!(error, Error::LoginRequired));
        } else {
            assert!(matches!(error, Error::Remote { .. }));
        }
        assert!(manager.has_credential().await.unwrap());
    }
    manager.clear().await.unwrap();
    assert!(manager.get_existing().await.unwrap().is_none());
    relay.done().await;
}
struct HeldTransport {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[async_trait]
impl Transport for HeldTransport {
    async fn send(&self, _: Request) -> Result<Response> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(response(
            200,
            json!({"access_token":"marker-new-access","refresh_token":"marker-new-refresh","expires_in":3600,"token_type":"Bearer","scope":"orchestration:read"}),
        ))
    }
}
#[tokio::test]
async fn oauth_refresh_finishes_persisting_rotated_credentials_after_requester_cancellation() {
    let secrets = Arc::new(MemorySecrets::default());
    let transport = Arc::new(HeldTransport {
        entered: Default::default(),
        release: Default::default(),
    });
    let manager = Arc::new(
        TokenManager::new(
            secrets,
            transport.clone(),
            Arc::new(Time::default()),
            "https://oauth.test/token",
            "client".into(),
        )
        .unwrap(),
    );
    manager.store(&token()).await.unwrap();
    let child = manager.clone();
    let task = tokio::spawn(async move { child.get_existing().await });
    transport.entered.notified().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    transport.release.notify_one();
    let token = manager.get_existing().await.unwrap().unwrap();
    assert_eq!(token.refresh_token.expose(), "marker-new-refresh");
}

#[tokio::test]
async fn forwarded_wait_uses_short_chunks_and_keeps_the_run_chosen_by_the_first_chunk() {
    let (peers, relay, _, _temp) = peers().await;
    relay
        .add(|_| {
            Ok(response(
                200,
                serde_json::to_value(descriptor("remote")).unwrap(),
            ))
        })
        .await;
    sign_in(&relay).await;
    peers.link(input("https://peer.test")).await.unwrap();
    relay
        .add(|_| {
            Ok(response(
                200,
                serde_json::to_value(descriptor("remote")).unwrap(),
            ))
        })
        .await;
    relay
        .add(|_| {
            let mut result = response(200, json!({"result":{}}));
            result
                .headers
                .insert("mcp-session-id".into(), "session-one".into());
            Ok(result)
        })
        .await;
    relay.add(|_| Ok(response(202, json!({})))).await;
    relay.add(|r|{let body:Value=serde_json::from_slice(&r.body).unwrap();let args=&body["params"]["arguments"];assert_eq!(args["timeoutMs"],50_000);assert!(args.get("runId").is_none());Ok(response(200,json!({"result":{"content":[{"type":"text","text":json!({"threadId":"thread","runId":"run-one","status":"running","timedOut":true}).to_string()}]}})))}).await;
    relay
        .add(|_| {
            Ok(response(
                200,
                serde_json::to_value(descriptor("remote")).unwrap(),
            ))
        })
        .await;
    relay.add(|r|{let body:Value=serde_json::from_slice(&r.body).unwrap();let args=&body["params"]["arguments"];assert_eq!(args["timeoutMs"],10_000);assert_eq!(args["runId"],"run-one");Ok(response(200,json!({"result":{"content":[{"type":"text","text":json!({"threadId":"thread","runId":"run-one","status":"completed","timedOut":false}).to_string()}]}})))}).await;
    peers.enabled.store(false, Ordering::Relaxed);
    let client = PeerMcpClient::new(peers);
    let result = client
        .call::<Value>(PeerCall {
            environment_id: "remote",
            tool: "t3_thread_wait",
            arguments: json!({"threadId":"thread","timeoutMs":60_000}),
            limits: CallLimits {
                runtime_mode: RuntimeMode::ApprovalRequired,
                interaction_mode: ProviderInteractionMode::Plan,
            },
            namespace: "caller",
            sender: None,
            linked_origin: false,
        })
        .await
        .unwrap();
    assert_eq!(result.expose()["runId"], "run-one");
    assert_eq!(result.expose()["timedOut"], false);
    relay.done().await;
    assert!(matches!(
        client
            .call::<Value>(PeerCall {
                environment_id: "remote",
                tool: "t3_thread_read",
                arguments: json!({}),
                limits: CallLimits {
                    runtime_mode: RuntimeMode::ApprovalRequired,
                    interaction_mode: ProviderInteractionMode::Plan
                },
                namespace: "caller",
                sender: None,
                linked_origin: true
            })
            .await,
        Err(Error::Identity)
    ));
}

#[test]
fn sender_header_encodes_components_and_truncates_at_whole_unicode_characters() {
    let sender = t3_contracts::orchestration_v2::OrchestrationV2LinkedSender {
        environment_id: "env/one".to_owned().into(),
        thread_id: "thread/one".to_owned().into(),
        title: "😀".repeat(201),
    };
    let encoded = sender_header(&sender);
    let fields = encoded.split('/').collect::<Vec<_>>();
    assert_eq!(fields.len(), 3);
    assert_eq!(fields[0], "env%2Fone");
    assert_eq!(fields[1], "thread%2Fone");
    assert_eq!(
        percent_encoding::percent_decode_str(fields[2])
            .decode_utf8()
            .unwrap()
            .chars()
            .count(),
        200
    );
}

#[test]
fn peer_refusals_preserve_codes_without_echoing_remote_messages() {
    for (code, expected) in [
        (
            "runtime_mode_escalation_denied",
            "RuntimeModeEscalationDenied",
        ),
        ("capability_denied", "CapabilityDenied"),
        ("run_not_found", "RunNotFound"),
        ("unknown_future_code", "OrchestrationError"),
    ] {
        let error = parse_mcp_response(&response(200, json!({"result":{"isError":true,"content":[{"type":"text","text":json!({"code":code,"message":"marker-remote-secret"}).to_string()}]}}))).unwrap_err();
        assert_eq!(
            format!("{error}"),
            format!("remote MCP call refused ({expected})")
        );
        assert!(!format!("{error:?} {error}").contains("marker-remote-secret"));
    }
}
