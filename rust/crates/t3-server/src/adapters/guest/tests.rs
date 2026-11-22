//! Real HTTP over a test tunnel identity and the actual database, dispatcher and event stream.
#![allow(clippy::unwrap_used)]

use super::*;
use crate::adapters::config::{ConfigInputs, LocalServices, ServerFlags};
use crate::services::{Domains, Phase, ServiceRegistry};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use t3_cloud::{
    managed::{EndpointStatus, RuntimeNotice},
    tunnel::ConnectorFacts,
};
use t3_orchestration::{
    composition::{RuntimePorts, StaticSettings},
    ports::sessions_recovery::provider_adapter_registry::AdapterRegistry,
    runtime::checkpoints::GitCheckpointBackend,
    stores::StoreDatabase,
};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_util::sync::CancellationToken;

struct Connector(tokio::sync::broadcast::Sender<RuntimeNotice>);
#[async_trait]
impl ManagedEndpoint for Connector {
    async fn apply(
        &self,
        _: Option<
            t3_cloud::http::Sensitive<t3_contracts::relay::RelayManagedEndpointRuntimeConfig>,
        >,
    ) -> Result<EndpointStatus> {
        Err(Error::Disabled)
    }
    async fn inspect(&self) -> Option<ConnectorFacts> {
        Some(ConnectorFacts {
            pid: None,
            metrics_port: None,
            ready_connections: Some(1),
            hostname: Some("fixture.example".into()),
            http2: false,
        })
    }
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<RuntimeNotice> {
        self.0.subscribe()
    }
}

async fn local(path: &Path, port: u16) -> LocalServices {
    let input = ConfigInputs {
        flags: ServerFlags {
            base_dir: Some(path.to_string_lossy().into_owned()),
            host: Some("127.0.0.1".into()),
            port: Some(port),
            ..Default::default()
        },
        env: Default::default(),
        home: path.into(),
        process_cwd: path.into(),
        bootstrap: None,
        headless: true,
        force_auto_bootstrap: Some(false),
        service_managed: false,
        cli_log_level: None,
        automatic_port: None,
        persisted_observability: Default::default(),
        static_dir: None,
    };
    let local = LocalServices::open(
        crate::adapters::config::resolve(&input).unwrap(),
        port,
        None,
    )
    .await
    .unwrap();
    local
        .settings
        .update(&serde_json::from_value(json!({"guestSharingEnabled": true})).unwrap())
        .await
        .unwrap();
    local
}

async fn command(runtime: &OrchestrationRuntime, value: Value) {
    runtime
        .dispatch_command(serde_json::from_value(value).unwrap())
        .await
        .unwrap();
}

async fn next_event(response: &mut reqwest::Response, pending: &mut Vec<u8>) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(end) = pending.windows(2).position(|bytes| bytes == b"\n\n") {
                let frame = pending.drain(..end + 2).collect::<Vec<_>>();
                if let Some(body) = frame.strip_prefix(b"data: ") {
                    return serde_json::from_slice(body).unwrap();
                }
            } else {
                pending.extend(response.chunk().await.unwrap().unwrap());
            }
        }
    })
    .await
    .unwrap()
}

struct Harness {
    http: reqwest::Client,
    base: String,
    id: String,
    credential: String,
    runtime: Arc<OrchestrationRuntime>,
    server: tokio::task::JoinHandle<()>,
    _root: tempfile::TempDir,
}

impl Harness {
    async fn finish(self) {
        self.server.abort();
        let _ = self.server.await;
        self.runtime.shutdown().await.unwrap();
    }
}

struct ManualClock(std::sync::atomic::AtomicI64);
impl t3_cloud::Clock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

async fn harness(clock: t3_cloud::SharedClock) -> Harness {
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let local = local(root.path(), addr.port()).await;
    let mut ports = RuntimePorts::new(
        StoreDatabase(local.database().clone()),
        Arc::new(AdapterRegistry::new(Vec::new())),
        Arc::new(GitCheckpointBackend::default()),
        Arc::new(crate::adapters::orchestration::SystemClock),
    );
    ports.settings = Arc::new(StaticSettings::new(
        local.settings.snapshot().await.unwrap(),
    ));
    let runtime = Arc::new(OrchestrationRuntime::start(ports).await.unwrap());
    runtime
        .project_command(
            serde_json::from_value(json!({
                "type": "project.create", "commandId": "test:project", "projectId": "test:project",
                "title": "Project", "workspaceRoot": root.path(),
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    for (id, title) in [("thread:shared", "Shared"), ("thread:private", "Private")] {
        command(
            &runtime,
            json!({"type":"thread.create","commandId":format!("create:{id}"),
            "threadId":id,"projectId":"test:project","title":title,
            "modelSelection":{"instanceId":"devReplay","model":"minimal"},
            "runtimeMode":"full-access","interactionMode":"default","branch":null,
            "worktreePath":null,"createdBy":"user","creationSource":"web"}),
        )
        .await;
    }
    let threads = Arc::new(Threads {
        runtime: runtime.clone(),
        settings: local.settings.clone(),
        connector: Arc::new(Connector(tokio::sync::broadcast::channel(2).0)),
        allowed: true,
        #[cfg(feature = "providers")]
        providers: None,
    });
    let secrets = t3_auth::ServerSecretStore::open(local.config.paths.secrets())
        .await
        .unwrap();
    let guest = Arc::new(GuestAdapter {
        sharing: GuestSharing::new(threads, Arc::new(secrets), clock),
        runtime: runtime.clone(),
    });
    let share = guest
        .sharing
        .create(t3_contracts::guest_share::GuestShareCreateInput {
            thread_id: ThreadId("thread:shared".into()),
        })
        .await
        .unwrap()
        .into_inner();
    let (url, id) = (share.url.0, share.share.id.0);
    let credential = url.split_once('#').unwrap().1.to_owned();
    let mut domains = Domains::default();
    domains.insert(guest).unwrap();
    let shutdown = CancellationToken::new();
    let services = Arc::new(ServiceRegistry {
        handlers: crate::handlers::registry_with(&domains).unwrap(),
        local,
        shutdown: shutdown.clone(),
        connections: Default::default(),
        lifecycle: Default::default(),
        domains,
    });
    services.lifecycle.advance(Phase::Ready);
    let app = crate::http::router(services);
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
        .unwrap();
    });
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    Harness {
        http,
        base,
        id,
        credential,
        runtime,
        server,
        _root: root,
    }
}

#[tokio::test]
async fn guest_cookie_binds_to_one_share_and_revocation_ends_the_live_thread() {
    let Harness {
        http,
        base,
        id,
        credential,
        runtime,
        server,
        _root,
    } = harness(Arc::new(t3_cloud::SystemClock)).await;
    let credential = credential.as_str();
    let response = http
        .post(format!("{base}/api/guest/redeem"))
        .json(&json!({"credential": credential, "name": "Guest"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    let header = response.headers()["set-cookie"].to_str().unwrap();
    for attribute in [
        "HttpOnly",
        "Secure",
        "SameSite=Strict",
        "Path=/api/guest",
        "Max-Age=86400",
    ] {
        assert!(
            header.split(';').any(|part| part.trim() == attribute),
            "missing {attribute}"
        );
    }
    let cookie = header.split(';').next().unwrap();
    let (_, token) = cookie.split_once('=').unwrap();
    assert_eq!(
        http.get(format!("{base}/api/guest/session?share=other"))
            .header("cookie", format!("t3_guest_other={token}"))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        http.get(format!("{base}/api/orchestration/shell"))
            .header("x-t3-orchestration-protocol", "2")
            .header("cookie", cookie)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let mut stream = http
        .get(format!(
            "{base}/api/guest/stream?share={id}&threadId=thread:private"
        ))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), 200);
    assert_eq!(stream.headers()["cache-control"], "no-store, no-transform");
    let mut pending = Vec::new();
    let view = next_event(&mut stream, &mut pending).await;
    assert_eq!(view["type"], "view");
    assert_eq!(view["view"]["title"], "Shared");
    let snapshot = next_event(&mut stream, &mut pending).await;
    assert_eq!(snapshot["item"]["kind"], "snapshot");
    assert_eq!(
        snapshot["item"]["projection"]["thread"]["id"],
        "thread:shared"
    );
    let sent = http
        .post(format!("{base}/api/guest/messages?share={id}"))
        .header("cookie", cookie)
        .json(&json!({"shareId": id, "text": "A pending guest message"}))
        .send()
        .await
        .unwrap();
    assert_eq!(sent.status(), 204);
    let delta = next_event(&mut stream, &mut pending).await;
    assert_eq!(delta["item"]["kind"], "event");
    let view = next_event(&mut stream, &mut pending).await;
    assert_eq!(view["view"]["messages"][0]["status"], "pending");
    command(&runtime, json!({"type":"thread.guest-share.close","commandId":"test:close","threadId":"thread:shared"})).await;
    assert_eq!(
        next_event(&mut stream, &mut pending).await,
        json!({"type":"ended"})
    );
    assert_eq!(
        http.get(format!("{base}/api/guest/session?share={id}"))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    server.abort();
    let _ = server.await;
    runtime.shutdown().await.unwrap();
}

async fn redeem(h: &Harness) -> String {
    let response = h
        .http
        .post(format!("{}/api/guest/redeem", h.base))
        .json(&json!({"credential": h.credential, "name": "Guest"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn an_expired_share_refuses_redeemed_cookies_and_unspent_links() {
    let now = chrono::Utc::now().timestamp_millis();
    let clock = Arc::new(ManualClock(std::sync::atomic::AtomicI64::new(now)));
    let h = harness(clock.clone()).await;
    let cookie = redeem(&h).await;
    let session = format!("{}/api/guest/session?share={}", h.base, h.id);
    let check = |cookie: String| {
        let (http, session) = (h.http.clone(), session.clone());
        async move {
            http.get(session)
                .header("cookie", cookie)
                .send()
                .await
                .unwrap()
        }
    };
    assert_eq!(check(cookie.clone()).await.status(), 204);
    clock.0.fetch_add(
        t3_contracts::guest_share::GUEST_SHARE_TTL_MS as i64 + 1,
        std::sync::atomic::Ordering::SeqCst,
    );
    let expired = check(cookie.clone()).await;
    assert_eq!(expired.status(), 403);
    let expired = expired.json::<Value>().await.unwrap();
    assert_eq!(expired["reason"], "inactive");
    let stream = h
        .http
        .get(format!("{}/api/guest/stream?share={}", h.base, h.id))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), 403);
    let submit = h
        .http
        .post(format!("{}/api/guest/messages?share={}", h.base, h.id))
        .header("cookie", &cookie)
        .json(&json!({"shareId": h.id, "text": "late"}))
        .send()
        .await
        .unwrap();
    assert_eq!(submit.status(), 403);
    h.finish().await;
}

#[tokio::test]
async fn a_guest_credential_reaches_only_its_share_and_never_owner_routes() {
    let h = harness(Arc::new(t3_cloud::SystemClock)).await;
    let cookie = redeem(&h).await;
    // Redemption is single-use: replaying the invite is refused.
    let replay = h
        .http
        .post(format!("{}/api/guest/redeem", h.base))
        .json(&json!({"credential": h.credential, "name": "Other"}))
        .send()
        .await
        .unwrap();
    assert!(replay.status().is_client_error(), "{}", replay.status());
    // The token under a forged share name or no cookie at all grants nothing.
    let token = cookie.split_once('=').unwrap().1;
    for forged in [
        format!("t3_guest_{}x={token}", h.id),
        format!("session={token}"),
        String::new(),
    ] {
        let response = h
            .http
            .get(format!("{}/api/guest/session?share={}", h.base, h.id))
            .header("cookie", forged.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403, "{forged}");
    }
    // The private thread is not selectable through the item and history reads.
    for path in [
        "item?item=item-that-is-not-in-the-shared-thread&threadId=thread:private",
        "history?cursor=0&threadId=thread:private",
    ] {
        let response = h
            .http
            .get(format!("{}/api/guest/{path}&share={}", h.base, h.id))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        assert!(!body.contains("thread:private"), "{path}: {body}");
        assert_ne!(status, 200, "{path}: {body}");
    }
    // Owner surfaces never accept the guest cookie.
    for (method, path) in [
        ("GET", "/api/orchestration/shell"),
        ("POST", "/api/auth/pairing-token"),
    ] {
        let response = h
            .http
            .request(method.parse().unwrap(), format!("{}{path}", h.base))
            .header("x-t3-orchestration-protocol", "2")
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        assert!(!body.contains(token), "{path}");
        assert!(
            matches!(status, 400 | 401 | 403 | 404 | 405),
            "{method} {path}: {status}"
        );
    }
    let session = h
        .http
        .get(format!("{}/api/auth/session", h.base))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(session["authenticated"], false, "{session}");
    let mut upgrade = format!("{}/ws", h.base.replace("http", "ws"))
        .into_client_request()
        .unwrap();
    upgrade
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    assert!(tokio_tungstenite::connect_async(upgrade).await.is_err());
    h.finish().await;
}

/// The production client path end to end: the shared guest HTTP transport over a native host
/// against the real routes, with the cookie held by the host as a browser would hold it.
#[tokio::test]
async fn the_guest_client_joins_reads_and_submits_over_the_real_routes() {
    use std::rc::Rc;
    use t3_client::guest::{
        http::HttpGuestTransport,
        session::{GuestClient, GuestCredential, GuestEnd, GuestPhase, GuestSession},
    };
    use t3_platform::guest::NativeGuestHttp;

    let Harness {
        http: _,
        base,
        id,
        credential,
        runtime,
        server,
        _root,
    } = harness(Arc::new(t3_cloud::SystemClock)).await;
    let origin: url::Url = base.parse().unwrap();
    let browser = |origin: &url::Url| {
        Rc::new(HttpGuestTransport::new(
            NativeGuestHttp::new(reqwest::Client::builder(), origin.clone()).unwrap(),
        ))
    };
    let wake = Rc::new(tokio::sync::Notify::new());
    let until = |client: GuestClient, ready: fn(&GuestSession) -> bool| {
        let wake = wake.clone();
        async move {
            tokio::time::timeout(Duration::from_secs(10), async {
                while !client.snapshot(ready) {
                    wake.notified().await;
                }
            })
            .await
            .unwrap();
        }
    };
    let notify = {
        let wake = wake.clone();
        move |_: &GuestSession| wake.notify_one()
    };
    tokio::task::LocalSet::new()
        .run_until(async {
            let first = browser(&origin);
            let client = GuestClient::new(
                first.clone(),
                &id,
                Some(GuestCredential::new(credential.clone())),
                notify.clone(),
            );
            client.start().await;
            assert!(client.snapshot(|s| matches!(s.phase, GuestPhase::NeedsName { .. })));
            client.join("Guest").await;
            assert!(client.snapshot(|s| s.phase == GuestPhase::Connecting));

            // The invite works once: another browser is told so, and nothing is leaked into it.
            let other = GuestClient::new(
                browser(&origin),
                &id,
                Some(GuestCredential::new(credential.clone())),
                |_| {},
            );
            other.start().await;
            other.join("Intruder").await;
            assert!(other.snapshot(|s| s.phase == GuestPhase::Ended(GuestEnd::Used)));

            let running = tokio::task::spawn_local({
                let client = client.clone();
                async move { client.run().await }
            });
            until(client.clone(), |s| s.phase == GuestPhase::Live).await;
            assert_eq!(client.snapshot(|s| s.view.as_ref().unwrap().title.clone()), "Shared");

            client.submit("A pending guest message").await.unwrap();
            until(client.clone(), |s| s.pending_count() == 1).await;

            // Returning to the page with the cookie but no invite resumes the same session.
            let returning = GuestClient::new(first, &id, None, |_| {});
            returning.start().await;
            assert!(returning.snapshot(|s| s.phase == GuestPhase::Connecting));
            let stranger = GuestClient::new(browser(&origin), &id, None, |_| {});
            stranger.start().await;
            assert!(stranger.snapshot(|s| s.phase == GuestPhase::Ended(GuestEnd::Invalid)));

            command(
                &runtime,
                json!({"type":"thread.guest-share.close","commandId":"test:e2e-close","threadId":"thread:shared"}),
            )
            .await;
            until(client.clone(), |s| {
                s.phase == GuestPhase::Ended(GuestEnd::Inactive)
            })
            .await;
            tokio::time::timeout(Duration::from_secs(10), running)
                .await
                .unwrap()
                .unwrap();
        })
        .await;
    server.abort();
    let _ = server.await;
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_guest_client_pages_history_and_reads_command_output_over_the_real_routes() {
    use std::rc::Rc;
    use t3_client::{
        composition::Spawner,
        guest::{
            http::HttpGuestTransport,
            session::{GuestClient, GuestCredential, GuestPhase, GuestSession, TranscriptRole},
        },
    };
    use t3_orchestration::api::{CommitGuard, CommitRequest, Decision, EventCommitter};
    use t3_platform::guest::NativeGuestHttp;

    let Harness {
        http: _,
        base,
        id,
        credential,
        runtime,
        server,
        _root,
    } = harness(Arc::new(t3_cloud::SystemClock)).await;

    let now = "2026-11-21T00:00:00.000Z";
    let mut events = Vec::new();
    for ordinal in 1..=12u64 {
        let item_id = format!("msg-item-{ordinal}");
        let event_id = format!("ev-msg-{ordinal}");
        events.push(
            serde_json::from_value(json!({
                "id": event_id,
                "threadId": "thread:shared",
                "occurredAt": now,
                "type": "turn-item.updated",
                "payload": {
                    "id": item_id,
                    "type": "user_message",
                    "threadId": "thread:shared",
                    "runId": null,
                    "nodeId": null,
                    "providerThreadId": null,
                    "providerTurnId": null,
                    "nativeItemRef": null,
                    "parentItemId": null,
                    "ordinal": ordinal,
                    "status": "completed",
                    "title": null,
                    "startedAt": null,
                    "completedAt": null,
                    "updatedAt": now,
                    "createdBy": "user",
                    "creationSource": "web",
                    "messageId": format!("msg-{ordinal}"),
                    "inputIntent": "turn_start",
                    "text": format!("Turn {ordinal}"),
                    "attachments": []
                }
            }))
            .unwrap(),
        );
    }
    events.push(
        serde_json::from_value(json!({
            "id": "ev-cmd-13",
            "threadId": "thread:shared",
            "occurredAt": now,
            "type": "turn-item.updated",
            "payload": {
                "id": "cmd-e2e",
                "type": "command_execution",
                "threadId": "thread:shared",
                "runId": null,
                "nodeId": null,
                "providerThreadId": null,
                "providerTurnId": null,
                "nativeItemRef": null,
                "parentItemId": null,
                "ordinal": 13,
                "status": "completed",
                "title": null,
                "startedAt": null,
                "completedAt": null,
                "updatedAt": now,
                "input": "vp check",
                "output": "all checks green\n",
                "outputOmitted": true,
                "exitCode": 0
            }
        }))
        .unwrap(),
    );
    runtime
        .events
        .commit(CommitRequest {
            receipt: None,
            decision: Decision {
                events,
                ..Default::default()
            },
            guard: CommitGuard::None,
        })
        .await
        .unwrap();

    let origin: url::Url = base.parse().unwrap();
    let transport = Rc::new(HttpGuestTransport::new(Rc::new(
        NativeGuestHttp::new(reqwest::Client::builder(), origin).unwrap(),
    )));
    let wake = Arc::new(tokio::sync::Notify::new());
    let notify = {
        let wake = wake.clone();
        move |_: &GuestSession| wake.notify_one()
    };
    let close_runtime = runtime.clone();
    tokio::task::LocalSet::new()
        .run_until(async move {
            let client = GuestClient::new(
                transport,
                &id,
                Some(GuestCredential::new(credential)),
                notify,
            );
            client.start().await;
            client.join("Guest").await;
            let running = tokio::task::spawn_local({
                let client = client.clone();
                async move { client.run().await }
            });
            tokio::time::timeout(Duration::from_secs(10), async {
                while !client.snapshot(|s| s.phase == GuestPhase::Live && s.has_more_history()) {
                    wake.notified().await;
                }
            })
            .await
            .unwrap();
            let initial = client.snapshot(|s| s.transcript());
            assert!(initial.iter().any(|row| row.text == "Turn 12"));
            assert!(!initial.iter().any(|row| row.text == "Turn 1"));
            assert!(
                initial
                    .iter()
                    .any(|row| row.id == "cmd-e2e" && row.role == TranscriptRole::Command)
            );

            client.load_earlier().await;
            let paged = client.snapshot(|s| (s.transcript(), s.has_more_history()));
            assert!(!paged.1);
            assert!(paged.0.iter().any(|row| row.text == "Turn 1"));
            assert!(paged.0.iter().any(|row| row.text == "Turn 12"));

            let spawner: Spawner = Rc::new(|future| {
                tokio::task::spawn_local(future);
            });
            let lease = client.command_output("cmd-e2e", &spawner);
            let output = tokio::time::timeout(Duration::from_secs(10), async {
                use futures::StreamExt as _;
                let mut changes = lease.changes();
                loop {
                    if let Some(text) = lease.text() {
                        break text;
                    }
                    assert!(lease.error().is_none(), "{:?}", lease.error());
                    if changes.next().await.is_none() {
                        break lease.text().unwrap_or_default();
                    }
                }
            })
            .await
            .unwrap();
            assert_eq!(output, "all checks green\n");
            drop(lease);

            command(
                &close_runtime,
                json!({"type":"thread.guest-share.close","commandId":"test:e2e-history-close","threadId":"thread:shared"}),
            )
            .await;
            tokio::time::timeout(Duration::from_secs(10), running)
                .await
                .unwrap()
                .unwrap();
        })
        .await;
    server.abort();
    let _ = server.await;
    runtime.shutdown().await.unwrap();
}
