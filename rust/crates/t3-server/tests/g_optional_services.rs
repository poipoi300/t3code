#![cfg(all(feature = "local-services", feature = "scheduler"))]
#![allow(clippy::unwrap_used)]
#[path = "c_support/mod.rs"]
mod support;
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::sync::Arc;
use t3_contracts::base_schemas::ThreadId;
use t3_rpc::{CallKind, Client};
use t3_server::{
    adapters::{
        config::{LocalServices, resolve},
        scheduler::{
            DispatchReceipt, DispatchRequest, Orchestration, RelayDeliveryProof,
            ScheduledTaskService, SchedulerAdapter, SchedulerOptions, SchedulerResult, TokioClock,
        },
    },
    services::{Domains, ServiceRegistry},
};
use tokio::sync::mpsc;

struct DispatchSink(mpsc::UnboundedSender<DispatchRequest>);
impl Orchestration for DispatchSink {
    fn dispatch<'life0, 'async_trait>(
        &'life0 self,
        request: DispatchRequest,
    ) -> BoxFuture<'async_trait, SchedulerResult<DispatchReceipt>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            self.0.send(request).unwrap();
            Ok(DispatchReceipt {
                thread_id: ThreadId("thread:dispatched".into()),
            })
        })
    }
}

struct Fixture {
    services: Arc<ServiceRegistry>,
    scheduler: Arc<SchedulerAdapter>,
    scheduled_service: Arc<ScheduledTaskService>,
    dispatched: mpsc::UnboundedReceiver<DispatchRequest>,
    task: tokio::task::JoinHandle<()>,
    http: reqwest::Client,
    base: String,
    cookie: String,
    _temp: tempfile::TempDir,
}
impl Fixture {
    async fn start() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let local = LocalServices::open(
            resolve(&support::inputs(temp.path())).unwrap(),
            address.port(),
            None,
        )
        .await
        .unwrap();
        let credential = local.auth.startup_pairing_credential().await.unwrap();
        let clock = Arc::new(TokioClock::new("2026-07-01T08:00:00Z".parse().unwrap()));
        let (sent, dispatched) = mpsc::unbounded_channel();
        let service = Arc::new(
            ScheduledTaskService::new(
                local.database().clone(),
                clock.clone(),
                Arc::new(DispatchSink(sent)),
                SchedulerOptions {
                    time_zone: "UTC".parse().unwrap(),
                    ..Default::default()
                },
            )
            .await
            .unwrap(),
        );
        let relay = RelayDeliveryProof::new(PUBLIC_KEY, "https://relay.test", "env:test").unwrap();
        let scheduler = Arc::new(SchedulerAdapter::new(service.clone(), clock, Some(relay)));
        let mut domains = Domains::default();
        domains.insert_runtime(scheduler.clone()).unwrap();
        #[cfg(feature = "usage")]
        {
            use t3_server::adapters::usage::{ServiceConfig, UsageAdapter, UsageService};
            let source = temp.path().join("transcripts");
            std::fs::create_dir(&source).unwrap();
            std::fs::write(source.join("fixture.jsonl"), format!("{}\n", json!({
                "type":"assistant", "timestamp":"2026-07-01T07:00:00Z", "sessionId":"s1",
                "message":{"id":"m1","model":"fixture-model","usage":{"input_tokens":100,"output_tokens":20}}
            }))).unwrap();
            let mut config = ServiceConfig::new(
                temp.path().join("usage"),
                temp.path().join("usage/state"),
                temp.path().join("account-home"),
                "fixture-host".into(),
            );
            config
                .sources
                .push(t3_server::adapters::usage::Source::transcripts(
                    t3_contracts::usage::UsageProviderKind::Claude,
                    source,
                ));
            let usage = UsageService::with_dependencies(
                config,
                Arc::new(UsageClock),
                Arc::new(Rates),
                Arc::new(t3_server::adapters::usage::NoProviderProbe),
            )
            .await
            .unwrap();
            domains
                .insert(Arc::new(UsageAdapter::new(Arc::new(usage))))
                .unwrap();
        }
        let handlers = t3_server::handlers::registry_with(&domains).unwrap();
        let services = Arc::new(ServiceRegistry {
            handlers,
            local,
            domains,
            shutdown: Default::default(),
            connections: Default::default(),
            lifecycle: Default::default(),
        });
        // The production router mounts the webhook routes for the scheduler domain.
        let app = t3_server::http::router(services.clone());
        let stop = services.shutdown.clone();
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(stop.cancelled_owned())
            .await
            .unwrap();
        });
        let http = reqwest::Client::new();
        let base = format!("http://{address}");
        let cookie = support::pair(&http, &base, &credential).await;
        Self {
            services,
            scheduler,
            scheduled_service: service,
            dispatched,
            task,
            http,
            base,
            cookie,
            _temp: temp,
        }
    }
    async fn connect(&self, cookie: &str) -> (support::Socket, Client) {
        let ticket = support::ticket(&self.http, &self.base, cookie, "rpc").await;
        let (socket, _) = tokio_tungstenite::connect_async(format!(
            "{}/ws?orchestrationProtocol=2&wsTicket={ticket}",
            self.base.replacen("http:", "ws:", 1)
        ))
        .await
        .unwrap();
        (socket, Client::new(0, Default::default()))
    }
    async fn scoped_cookie(&self, scopes: &[&str]) -> String {
        let grant = self
            .http
            .post(format!("{}/api/auth/pairing-token", self.base))
            .header("cookie", &self.cookie)
            .json(&json!({"scopes":scopes}))
            .send()
            .await
            .unwrap();
        assert_eq!(grant.status(), 200);
        let grant: Value = grant.json().await.unwrap();
        support::pair(
            &self.http,
            &self.base,
            grant["credential"].as_str().unwrap(),
        )
        .await
    }
    async fn finish(self) {
        self.services.shutdown.cancel();
        self.task.await.unwrap();
        self.services.connections.close();
        self.services.connections.wait().await;
        self.scheduler.stop().await;
        drop(self.scheduler);
        drop(self.scheduled_service);
        let services = Arc::try_unwrap(self.services).ok().unwrap();
        drop(services.handlers);
        drop(services.domains);
        services.local.shutdown().await.unwrap();
    }
}

fn input(schedule: Value) -> Value {
    json!({"id":"task:one","title":"Fixture","prompt":"Process {{body.text}}","enabled":true,"schedule":schedule,"projectId":"project:test","workspaceStrategy":{"type":"root"},"modelSelection":{"instanceId":"codex","model":"test-model"},"runtimeMode":"full-access","interactionMode":"default"})
}

async fn assert_hook(response: reqwest::Response, status: u16, outcome: &str, body: Value) {
    assert_eq!(response.status().as_u16(), status);
    assert_eq!(response.headers()["x-t3-hook-outcome"], outcome);
    assert_eq!(response.json::<Value>().await.unwrap(), body);
}

#[tokio::test]
async fn scheduler_rpc_stream_mutations_errors_and_scope_guards() {
    let mut f = Fixture::start().await;
    for method in [
        "scheduledTasks.subscribe",
        "scheduledTasks.upsert",
        "scheduledTasks.setEnabled",
        "scheduledTasks.delete",
        "scheduledTasks.runNow",
        "scheduledTasks.rotateWebhookToken",
        "scheduledTasks.listWebhookDeliveries",
        "scheduledTasks.getWebhookDelivery",
    ] {
        assert_eq!(
            f.services.handlers.disposition(method),
            Some(true),
            "{method}"
        );
    }
    let reader = f.scoped_cookie(&["orchestration:read"]).await;
    let (mut read_socket, mut read_client) = f.connect(&reader).await;
    let (mut socket, mut client) = f.connect(&f.cookie).await;
    let stream_id = client
        .start("scheduledTasks.subscribe", json!({}), CallKind::Stream)
        .unwrap();
    let initial = support::chunk(&mut socket, &mut client).await;
    assert_eq!(initial, json!({"tasks":[],"timeZone":"UTC"}));
    let task = input(json!({"type":"interval","everyMs":60000}));
    let denied = support::first_cause(
        support::unary(
            &mut read_socket,
            &mut read_client,
            "scheduledTasks.upsert",
            task.clone(),
        )
        .await,
    );
    assert_eq!(denied["_tag"], "Fail");
    assert_eq!(denied["error"]["_tag"], "EnvironmentAuthorizationError");
    let upserted = support::unary_ok(
        &mut read_socket,
        &mut read_client,
        "server.probe",
        json!({}),
    )
    .await;
    assert_eq!(upserted, json!({}));
    // A separate socket mutates so stream chunks cannot overtake the unary helper's result.
    let (mut mutator, mut writer) = f.connect(&f.cookie).await;
    let upserted =
        support::unary_ok(&mut mutator, &mut writer, "scheduledTasks.upsert", task).await;
    assert_eq!(upserted["task"]["id"], "task:one");
    let update = support::chunk(&mut socket, &mut client).await;
    assert_eq!(update["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(update["tasks"][0]["id"], "task:one");
    client.interrupt(&stream_id);
    support::flush(&mut socket, &mut client).await;
    let run = support::unary_ok(
        &mut mutator,
        &mut writer,
        "scheduledTasks.runNow",
        json!({"id":"task:one"}),
    )
    .await;
    let dispatched = f.dispatched.recv().await.unwrap();
    assert_eq!(dispatched.task.id.0, "task:one");
    assert_eq!(run["task"]["runCount"], 1);
    let enabled = support::unary_ok(
        &mut mutator,
        &mut writer,
        "scheduledTasks.setEnabled",
        json!({"id":"task:one","enabled":false}),
    )
    .await;
    assert_eq!(enabled["task"]["enabled"], false);
    let missing = support::first_cause(
        support::unary(
            &mut mutator,
            &mut writer,
            "scheduledTasks.runNow",
            json!({"id":"task:missing"}),
        )
        .await,
    );
    assert_eq!(missing["error"]["_tag"], "ScheduledTaskError");
    assert_eq!(missing["error"]["taskId"], "task:missing");
    let removed = support::unary_ok(
        &mut mutator,
        &mut writer,
        "scheduledTasks.delete",
        json!({"id":"task:one"}),
    )
    .await;
    assert_eq!(removed, json!({"id":"task:one"}));
    drop((socket, mutator, read_socket));
    f.finish().await;
}

#[tokio::test]
async fn composition_registers_the_scheduler_commands() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = t3_server::runtime::start(support::inputs(temp.path()))
        .await
        .unwrap();
    let http = reqwest::Client::new();
    let base = format!("http://{}", runtime.address);
    let cookie = support::pair(&http, &base, &runtime.pairing_credential).await;
    let ticket = support::ticket(&http, &base, &cookie, "rpc").await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!(
        "ws://{}/ws?orchestrationProtocol=2&wsTicket={ticket}",
        runtime.address
    ))
    .await
    .unwrap();
    let mut client = Client::new(0, Default::default());
    for (tag, payload) in [
        (
            "scheduledTasks.upsert",
            input(json!({"type":"interval","everyMs":60000})),
        ),
        (
            "scheduledTasks.setEnabled",
            json!({"id":"task:one","enabled":true}),
        ),
        ("scheduledTasks.runNow", json!({"id":"task:one"})),
    ] {
        // Served now: whatever the answer, it is not the unknown-method defect.
        let cause =
            support::first_cause(support::unary(&mut socket, &mut client, tag, payload).await);
        assert_ne!(
            cause["defect"],
            format!("Unknown request tag: {tag}"),
            "{tag} is registered"
        );
    }
    #[cfg(feature = "usage")]
    {
        let cause = support::first_cause(
            support::unary(
                &mut socket,
                &mut client,
                "server.refreshUsageRates",
                json!({}),
            )
            .await,
        );
        assert_ne!(
            cause["defect"], "Unknown request tag: server.refreshUsageRates",
            "server.refreshUsageRates is registered"
        );
    }
    client
        .start("scheduledTasks.subscribe", json!({}), CallKind::Stream)
        .unwrap();
    let snapshot = support::chunk(&mut socket, &mut client).await;
    // The upsert above is served, so the snapshot holds the task it saved.
    let tasks = snapshot["tasks"].as_array().expect("task list");
    assert_eq!(tasks.len(), 1, "{snapshot}");
    assert_eq!(tasks[0]["id"], "task:one");
    drop(socket);
    runtime.shutdown().await.unwrap();
}

// Fixed Ed25519 fixture: seed [7; 32], bound to task:one at the fixture clock.
const PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEA6kpsY+KcUgq+9VB7Ey7F+ZVHdq6+vnuSQh7qaRRG0iw=\n-----END PUBLIC KEY-----\n";
const PROOF: &str = "eyJhbGciOiJFZERTQSIsInR5cCI6InQzLXJlbGF5LWhvb2stZGVsaXZlcnkrand0In0.eyJpc3MiOiJodHRwczovL3JlbGF5LnRlc3QiLCJhdWQiOiJ0My1lbnY6ZW52OnRlc3QiLCJzdWIiOiJyZWxheSIsImp0aSI6InByb29mOm9uZSIsImlhdCI6MTc4Mjg5MjgwMCwiZXhwIjoxNzgyODk2NDAwLCJlbnZpcm9ubWVudElkIjoiZW52OnRlc3QiLCJkZWxpdmVyeUlkIjoiZGVsaXZlcnk6b25lIiwicmVjZWl2ZWRBdCI6IjIwMjYtMDctMDFUMDc6NTk6MDBaIiwiaG9va0lkIjoidGFzazpvbmUifQ.Qql3Mxhl-Lo_CCi6VRWJU6vBnpj9b5bm7XMkRTCxgu2p65FL0en5a7WhshckxTGHMP5165aQXwPBnqA9XxzOAg";
const BODY: &str = "{\"text\":\"café\"}";
const HMAC: &str = "sha256=8fe02057647cfb1a6d2f74959e1a26e50f6eaa9cfeb78d17d8e5684634dfb1e1";

#[cfg(feature = "usage")]
struct UsageClock;
#[cfg(feature = "usage")]
impl t3_server::adapters::usage::Clock for UsageClock {
    fn now_ms(&self) -> f64 {
        1_782_892_800_000.0
    }
}
#[cfg(feature = "usage")]
struct Rates;
#[cfg(feature = "usage")]
impl t3_server::adapters::usage::RateFetcher for Rates {
    fn fetch<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
    ) -> BoxFuture<'a, Result<Value, t3_server::adapters::usage::UsageError>> {
        Box::pin(async {
            Ok(
                json!({"fixture-model":{"input_cost_per_token":0.000001,"output_cost_per_token":0.000002}}),
            )
        })
    }
}

#[cfg(feature = "usage")]
#[tokio::test]
async fn usage_rpc_reads_current_aliases_and_prices_refreshes_rates_and_returns_typed_errors() {
    let f = Fixture::start().await;
    let (mut socket, mut client) = f.connect(&f.cookie).await;
    let window = json!({"sinceDay":"2026-07-01","untilDay":"2026-07-01","timeZone":"UTC"});
    let rates = support::unary_ok(
        &mut socket,
        &mut client,
        "server.refreshUsageRates",
        json!({}),
    )
    .await;
    assert_eq!(rates["status"], "fresh");
    assert_eq!(rates["knownModels"], 1);
    let first = support::unary_ok(
        &mut socket,
        &mut client,
        "server.getUsageSummary",
        window.clone(),
    )
    .await;
    assert_eq!(first["buckets"].as_array().unwrap().len(), 1);
    assert_eq!(first["buckets"][0]["model"], "fixture-model");
    assert_eq!(first["buckets"][0]["totals"]["uncachedInputTokens"], 100);
    assert_eq!(first["buckets"][0]["totals"]["outputTokens"], 20);
    support::unary_ok(&mut socket, &mut client, "server.updateSettings", json!({"patch":{
        "usageModelAliases":{"fixture-model":"priced-model"},
        "usagePriceOverrides":{"priced-model":{"inputCostPerMillionTokens":100,"outputCostPerMillionTokens":200}}
    }})).await;
    let updated = support::unary_ok(
        &mut socket,
        &mut client,
        "server.getUsageSummary",
        window.clone(),
    )
    .await;
    assert_eq!(updated["buckets"][0]["model"], "priced-model");
    assert_eq!(updated["buckets"][0]["costUsd"], 0.014);
    assert_ne!(
        updated["buckets"][0]["costUsd"],
        first["buckets"][0]["costUsd"]
    );
    let failure = support::first_cause(
        support::unary(
            &mut socket,
            &mut client,
            "server.getUsageSummary",
            json!({"sinceDay":"2026-07-02","untilDay":"2026-07-01","timeZone":"UTC"}),
        )
        .await,
    );
    assert_eq!(failure["_tag"], "Fail");
    assert_eq!(failure["error"]["_tag"], "UsageReadError");
    assert_eq!(failure["error"]["reason"], "invalidWindow");
    let no_diagnostics = f.scoped_cookie(&["access:read"]).await;
    let (mut restricted, mut restricted_client) = f.connect(&no_diagnostics).await;
    for (tag, payload) in [
        ("server.getUsageSummary", window),
        ("server.refreshUsageRates", json!({})),
    ] {
        let denied = support::first_cause(
            support::unary(&mut restricted, &mut restricted_client, tag, payload).await,
        );
        assert_eq!(
            denied["error"]["_tag"], "EnvironmentAuthorizationError",
            "{denied}"
        );
    }
    drop((socket, restricted));
    f.finish().await;
}

#[tokio::test]
async fn webhook_http_authentication_bounds_replay_delivery_queries_and_rotation() {
    let mut f = Fixture::start().await;
    let (mut socket, mut client) = f.connect(&f.cookie).await;
    let task = support::unary_ok(&mut socket, &mut client, "scheduledTasks.upsert", input(json!({"type":"webhook","signature":{"header":"X-Signature","encoding":"hex","prefix":"sha256=","secret":"fixture-secret"}}))).await;
    let path = task["task"]["webhook"]["path"].as_str().unwrap();
    let url = format!("{}{path}", f.base);
    assert_hook(
        f.http
            .post(format!("{}/api/hooks/task:one/wrong", f.base))
            .body(BODY)
            .header("X-Signature", HMAC)
            .send()
            .await
            .unwrap(),
        404,
        "not_found",
        json!({"error":"hook_not_found"}),
    )
    .await;
    for signature in [None, Some("sha256=wrong")] {
        let mut request = f.http.post(&url).body(BODY);
        if let Some(value) = signature {
            request = request.header("X-SIGNATURE", value);
        }
        assert_hook(
            request.send().await.unwrap(),
            401,
            "rejected_signature",
            json!({"error":"invalid_signature"}),
        )
        .await;
    }
    assert_hook(
        f.http
            .post(&url)
            .body(BODY)
            .header("X-Signature", "sha256=wrong")
            .header("X-Signature", HMAC)
            .send()
            .await
            .unwrap(),
        401,
        "rejected_signature",
        json!({"error":"invalid_signature"}),
    )
    .await;
    let head = f.http.head(&url).send().await.unwrap();
    assert_eq!(head.status(), 405);
    assert_eq!(head.headers()["x-t3-hook-outcome"], "method_not_allowed");
    assert_eq!(head.headers()["allow"], "GET, POST, PUT, PATCH");
    assert!(head.bytes().await.unwrap().is_empty());
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut declared = tokio::net::TcpStream::connect(f.base.trim_start_matches("http://"))
        .await
        .unwrap();
    declared.write_all(format!("POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1048577\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut rejected = String::new();
    declared.read_to_string(&mut rejected).await.unwrap();
    assert!(rejected.starts_with("HTTP/1.1 413"), "{rejected}");
    assert!(
        rejected.contains("x-t3-hook-outcome: body_too_large"),
        "{rejected}"
    );
    // No Content-Length: the live HTTP streaming reader must enforce the same cap.
    let mut raw = tokio::net::TcpStream::connect(f.base.trim_start_matches("http://"))
        .await
        .unwrap();
    raw.write_all(format!("POST {path} HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    // Exactly one byte over the cap and nothing after it: the server reads every byte sent
    // before it answers and closes. Bytes it never read would make Windows reset the
    // connection and drop the response, which made this test flaky.
    for _ in 0..16 {
        raw.write_all(b"10000\r\n").await.unwrap();
        raw.write_all(&vec![b'x'; 65536]).await.unwrap();
        raw.write_all(b"\r\n").await.unwrap();
    }
    raw.write_all(b"1\r\nx").await.unwrap();
    let mut response = String::new();
    raw.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 413"), "{response}");
    assert!(
        response.contains("x-t3-hook-outcome: body_too_large"),
        "{response}"
    );
    let request = || {
        f.http
            .patch(format!("{url}?delivery=one%20two"))
            .body(BODY)
            .header("Content-Type", "application/json")
            .header("X-SIGNATURE", HMAC)
            .header("X-T3-Relay-Delivery", PROOF)
            .header("X-T3-Relay-Delivery-Id", "delivery:one")
            .header("X-T3-Relay-Received-At", "2026-07-01T07:59:00Z")
    };
    let accepted = request().send().await.unwrap();
    assert_eq!(accepted.status(), 202);
    assert_eq!(accepted.headers()["x-t3-hook-outcome"], "accepted");
    let accepted: Value = accepted.json().await.unwrap();
    let delivery = accepted["deliveryId"].as_str().unwrap();
    let dispatched = f.dispatched.recv().await.unwrap();
    assert_eq!(dispatched.task.id.0, "task:one");
    assert!(dispatched.prompt.contains("café"));
    assert_hook(
        request().send().await.unwrap(),
        202,
        "duplicate",
        json!({"deliveryId":delivery}),
    )
    .await;
    f.scheduled_service.drain_background().await;
    assert!(matches!(
        f.dispatched.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    let log = support::unary_ok(
        &mut socket,
        &mut client,
        "scheduledTasks.listWebhookDeliveries",
        json!({"id":"task:one","acceptAnyOutcome":true}),
    )
    .await;
    assert!(
        log["deliveries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == delivery)
    );
    let detail = support::unary_ok(
        &mut socket,
        &mut client,
        "scheduledTasks.getWebhookDelivery",
        json!({"id":"task:one","deliveryId":delivery,"acceptAnyOutcome":true}),
    )
    .await;
    assert_eq!(detail["delivery"]["signatureVerified"], true);
    assert_eq!(detail["delivery"]["body"], BODY);
    assert_eq!(detail["delivery"]["method"], "PATCH");
    assert_eq!(detail["delivery"]["query"], "delivery=one%20two");
    // Forged relay headers do not claim the signed delivery's replay identity.
    for method in [reqwest::Method::GET, reqwest::Method::PUT] {
        let direct = f
            .http
            .request(method, &url)
            .body(BODY)
            .header("X-Signature", HMAC)
            .header("X-T3-Relay-Delivery", "forged-proof")
            .header("X-T3-Relay-Delivery-Id", "delivery:one")
            .header("X-T3-Relay-Received-At", "2026-07-01T07:59:00Z")
            .send()
            .await
            .unwrap();
        assert_eq!(direct.status(), 202);
        assert_eq!(direct.headers()["x-t3-hook-outcome"], "accepted");
        let dispatched = f.dispatched.recv().await.unwrap();
        assert_eq!(dispatched.task.id.0, "task:one");
        f.scheduled_service.drain_background().await;
    }
    let rotated = support::unary_ok(
        &mut socket,
        &mut client,
        "scheduledTasks.rotateWebhookToken",
        json!({"id":"task:one"}),
    )
    .await;
    assert_ne!(rotated["task"]["webhook"]["path"], path);
    assert_hook(
        f.http
            .post(&url)
            .body(BODY)
            .header("X-Signature", HMAC)
            .send()
            .await
            .unwrap(),
        404,
        "not_found",
        json!({"error":"hook_not_found"}),
    )
    .await;
    drop(socket);
    f.finish().await;
}
