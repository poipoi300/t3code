use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU16, Ordering},
    },
    time::Duration,
};
use t3_config::config::{SignalEndpoint, SignalExport, TelemetryConfig};
use t3_contracts::server::{OtlpEndpointCheckInput, OtlpEndpointCheckResult, OtlpSignal};
use t3_telemetry::{analytics::*, otlp::OtlpEndpointCheck};

#[derive(Clone, Debug)]
struct Request {
    uri: String,
    headers: HeaderMap,
    body: Vec<u8>,
}
type CollectorState = (Arc<Mutex<Vec<Request>>>, Arc<AtomicU16>);
struct Collector {
    url: String,
    requests: Arc<Mutex<Vec<Request>>>,
    status: Arc<AtomicU16>,
    cancel: tokio_util::sync::CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Collector {
    async fn new() -> Self {
        let requests = Arc::new(Mutex::new(vec![]));
        let status = Arc::new(AtomicU16::new(204));
        async fn collect(
            State((requests, status)): State<CollectorState>,
            axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
            headers: HeaderMap,
            body: Bytes,
        ) -> (StatusCode, HeaderMap) {
            requests.lock().expect("requests").push(Request {
                uri: uri.to_string(),
                headers,
                body: body.to_vec(),
            });
            let mut headers = HeaderMap::new();
            headers.insert("location", "/stolen".parse().expect("header"));
            (
                StatusCode::from_u16(status.load(Ordering::Acquire)).expect("status"),
                headers,
            )
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        let app = Router::new()
            .fallback(post(collect))
            .with_state((Arc::clone(&requests), Arc::clone(&status)));
        let cancel = tokio_util::sync::CancellationToken::new();
        let token = cancel.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(token.cancelled_owned())
                .await
                .expect("collector");
        });
        Self {
            url,
            requests,
            status,
            cancel,
            task: Some(task),
        }
    }
    fn requests(&self) -> Vec<Request> {
        self.requests.lock().expect("requests").clone()
    }
    async fn close(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.await.expect("join");
        }
    }
}
impl Drop for Collector {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
fn telemetry() -> TelemetryConfig {
    TelemetryConfig {
        disabled: false,
        warnings: vec![],
        resource_attributes: BTreeMap::new(),
        traces: None,
        metrics: None,
        logs: None,
        default_export: SignalExport {
            protocol: "http/json".into(),
            headers: None,
            export_interval_ms: 10000,
        },
    }
}
fn allowed() -> Consent {
    Consent {
        enabled: true,
        policy_allows: true,
        shell_discovery_pending: false,
        settings_readable: true,
    }
}
fn analytics(
    collector: &Collector,
    consent: Arc<RwLock<Option<Consent>>>,
) -> Arc<AnalyticsService> {
    let mut config = AnalyticsConfig::from_environment(
        &BTreeMap::new(),
        "1.0.0".into(),
        "stable".into(),
        "web".into(),
    )
    .expect("config");
    config.host = collector.url.clone();
    config.batch_size = 2;
    config.max_buffered_events = 3;
    AnalyticsService::new(
        config,
        Identity {
            distinct_id: Some(hash_telemetry_id("person")),
            install_id: Some(hash_telemetry_id("install")),
        },
        consent,
    )
    .expect("analytics")
}

#[tokio::test]
async fn opt_out_policy_pending_or_unreadable_settings_emit_no_requests() {
    let collector = Collector::new().await;
    let consent = Arc::new(RwLock::new(None));
    let service = analytics(&collector, Arc::clone(&consent));
    let denied = [
        None,
        Some(Consent {
            enabled: false,
            ..allowed()
        }),
        Some(Consent {
            policy_allows: false,
            ..allowed()
        }),
        Some(Consent {
            shell_discovery_pending: true,
            ..allowed()
        }),
        Some(Consent {
            settings_readable: false,
            ..allowed()
        }),
    ];
    for state in denied {
        *consent.write().expect("consent") = state;
        service.record("feature.used", BTreeMap::new()).await;
        service.flush().await.expect("silent flush");
        assert_eq!(service.buffered_count().await, 0);
        assert!(collector.requests().is_empty());
    }
    *consent.write().expect("consent") = Some(allowed());
    service.record("feature.used", BTreeMap::new()).await;
    *consent.write().expect("consent") = Some(Consent {
        enabled: false,
        ..allowed()
    });
    service.flush().await.expect("opted out after buffering");
    assert!(collector.requests().is_empty());
    *consent.write().expect("consent") = Some(allowed());
    service.record("feature.used", BTreeMap::new()).await;
    service.flush().await.expect("allowed");
    assert_eq!(collector.requests().len(), 1);
    collector.close().await;
}

#[tokio::test]
async fn retries_preserve_uuid_and_timestamp_and_expire_or_reject() {
    let collector = Collector::new().await;
    let service = analytics(&collector, Arc::new(RwLock::new(Some(allowed()))));
    collector.status.store(503, Ordering::Release);
    service
        .record_at(
            "feature.used",
            [
                ("feature".into(), json!("terminal.open")),
                ("prompt".into(), json!("PRIVATE")),
                ("inputTokens".into(), json!(12)),
            ]
            .into_iter()
            .collect(),
            1000,
        )
        .await;
    assert!(service.flush_at(1000, false).await.is_err());
    service.flush_at(1001, true).await.expect("backoff");
    assert_eq!(collector.requests().len(), 1);
    collector.status.store(204, Ordering::Release);
    service.flush_at(120_000, false).await.expect("recover");
    let requests = collector.requests();
    let first: Value = serde_json::from_slice(&requests[0].body).expect("json");
    let next: Value = serde_json::from_slice(&requests[1].body).expect("json");
    assert_eq!(first, next);
    assert_eq!(first["batch"][0]["properties"]["inputTokens"], 12);
    assert!(!String::from_utf8_lossy(&requests[0].body).contains("PRIVATE"));
    collector.status.store(503, Ordering::Release);
    service.record("feature.used", BTreeMap::new()).await;
    assert!(service.flush_at(1, false).await.is_err());
    assert!(service.flush_at(1_800_001, false).await.is_err());
    assert_eq!(service.buffered_count().await, 0);
    collector.status.store(413, Ordering::Release);
    service.record("feature.used", BTreeMap::new()).await;
    assert!(service.flush_at(2_000_000, false).await.is_err());
    assert_eq!(service.buffered_count().await, 0);
    collector.close().await;
}

#[tokio::test]
async fn bounds_buffer_and_deduplicates_hashed_owners_separately() {
    let collector = Collector::new().await;
    let service = analytics(&collector, Arc::new(RwLock::new(Some(allowed()))));
    for index in 0..5 {
        service
            .record(
                "feature.used",
                [("index".into(), json!(index))].into_iter().collect(),
            )
            .await;
    }
    service.flush().await.expect("drain");
    let requests = collector.requests();
    let events: Vec<Value> = requests
        .iter()
        .flat_map(|r| {
            serde_json::from_slice::<Value>(&r.body).expect("json")["batch"]
                .as_array()
                .expect("batch")
                .clone()
        })
        .collect();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["properties"]["index"], 2);
    let owner = TelemetryOwner {
        source: OwnerSource::Device,
        owner_id: hash_telemetry_id("device"),
    };
    service
        .record_owners(std::slice::from_ref(&owner), "client")
        .await;
    service
        .record_owners(std::slice::from_ref(&owner), "client")
        .await;
    service.record_owners(&[owner], "server").await;
    assert_eq!(service.buffered_count().await, 2);
    service.flush().await.expect("owners");
    collector.close().await;
}

#[tokio::test]
async fn endpoint_checks_never_forward_credentials_to_another_url_or_redirect() {
    let collector = Collector::new().await;
    let mut config = telemetry();
    config.traces = Some(SignalEndpoint {
        url: format!("{}/v1/traces", collector.url),
        export: SignalExport {
            headers: Some(
                [("x-api-key".into(), "SECRET".into())]
                    .into_iter()
                    .collect(),
            ),
            ..config.default_export.clone()
        },
        source: "environment".into(),
        variable: None,
    });
    let checker = OtlpEndpointCheck::new(config).expect("checker");
    for path in ["/v1/traces", "/v1/traces?tenant=other", "/v1/traces/other"] {
        let result = checker
            .check(&OtlpEndpointCheckInput {
                signal: OtlpSignal::Traces,
                url: format!("{}{path}", collector.url).into(),
            })
            .await;
        assert!(matches!(result, OtlpEndpointCheckResult::Accepted(_)));
    }
    let requests = collector.requests();
    assert_eq!(requests[0].headers["x-api-key"], "SECRET");
    assert!(!requests[1].headers.contains_key("x-api-key"));
    assert!(!requests[2].headers.contains_key("x-api-key"));
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[0].body).expect("body"),
        json!({"resourceSpans":[]})
    );
    collector.status.store(307, Ordering::Release);
    assert!(matches!(
        checker
            .check(&OtlpEndpointCheckInput {
                signal: OtlpSignal::Traces,
                url: format!("{}/v1/traces", collector.url).into()
            })
            .await,
        OtlpEndpointCheckResult::Rejected(_)
    ));
    assert_eq!(collector.requests().len(), 4);
    collector.close().await;
}

#[tokio::test]
async fn identity_is_hashed_persistent_and_provider_priority_is_compatible() {
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("anonymous-id");
    let codex = json!({"tokens":{"account_id":"TEAM"}});
    let claude = json!({"userID":"CLAUDE"});
    let first = resolve_identity(&path, Some(&codex), Some(&claude)).await;
    let again = resolve_identity(&path, None, None).await;
    assert_eq!(first.distinct_id, Some(hash_telemetry_id("TEAM")));
    assert_eq!(first.install_id, again.install_id);
    assert_eq!(again.distinct_id, again.install_id);
    assert!(first.install_id.as_deref().is_some_and(is_telemetry_hash));
    let bad = resolve_identity(dir.path(), None, None).await;
    assert!(bad.distinct_id.is_none());
    let api = json!({"auth_mode":"apikey"});
    let fallback = resolve_identity(&path, Some(&api), Some(&claude)).await;
    assert_eq!(fallback.distinct_id, Some(hash_telemetry_id("CLAUDE")));
    assert_eq!(retry_delay_ms(1, 0.0), 1000);
    assert_eq!(retry_delay_ms(50, 1.0), 300000);
}

#[tokio::test]
async fn tracing_exports_all_signals_and_flushes_redacted_local_traces() {
    for protocol in ["http/json", "http/protobuf"] {
        export_signals(protocol).await;
    }
}
async fn export_signals(protocol: &str) {
    use opentelemetry::metrics::MeterProvider;
    let collector = Collector::new().await;
    let dir = tempfile::tempdir().expect("temp");
    let mut config = telemetry();
    let endpoint = |path: &str| SignalEndpoint {
        url: format!("{}{path}", collector.url),
        export: SignalExport {
            protocol: protocol.into(),
            headers: None,
            export_interval_ms: 600_000,
        },
        source: "settings".into(),
        variable: None,
    };
    config.traces = Some(endpoint("/v1/traces"));
    config.metrics = Some(endpoint("/v1/metrics"));
    config.logs = Some(endpoint("/v1/logs"));
    let (dispatch, handle) = t3_telemetry::logging::build(t3_telemetry::logging::LoggingOptions {
        trace: t3_telemetry::logging::TraceOptions {
            file_path: dir.path().join("server.trace.ndjson"),
            max_bytes: 100_000,
            max_files: 2,
            batch_window: Duration::from_secs(60),
        },
        log_level: "Info".into(),
        trace_min_level: "Info".into(),
        trace_timing_enabled: true,
        telemetry: config,
        version: "test".into(),
        release_channel: "dev".into(),
        host_name: "test-host".into(),
        mode: t3_config::config::RuntimeMode::Web,
        console: false,
    })
    .await
    .expect("logging");
    tracing::dispatcher::with_default(&dispatch, || {
        let span = tracing::info_span!(
            "TraceDiagnostics.read",
            authorization = "Bearer VERY_SECRET",
            prompt = "PROMPT_SECRET",
            url = "https://example.test/ws?wsTicket=TICKET_SECRET&dpop=DPOP_SECRET"
        );
        let _guard = span.enter();
        tracing::warn!("safe operational message");
    });
    use tracing::instrument::WithSubscriber;
    fn nested_span(phase: &str) {
        let _span = tracing::info_span!("diagnostic.nested", phase).entered();
    }
    async {
        t3_telemetry::rpc::poll(
            "server.getTraceDiagnostics",
            async {
                nested_span("disabled");
            },
            tracing::Span::none(),
        )
        .await;
        let span = t3_telemetry::rpc::request_span("server.probe");
        t3_telemetry::rpc::poll(
            "server.probe",
            async {
                nested_span("enabled");
            },
            span,
        )
        .await;
    }
    .with_subscriber(dispatch.clone())
    .await;
    let meter = handle
        .meter_provider
        .as_ref()
        .expect("metrics")
        .meter("test");
    meter
        .u64_counter("t3_rpc_requests_total")
        .build()
        .add(1, &[]);
    handle.force_flush().await.expect("flush");
    let trace = std::fs::read_to_string(dir.path().join("server.trace.ndjson")).expect("trace");
    assert!(trace.contains("TraceDiagnostics.read"));
    let nested: Vec<Value> = trace
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| record["name"] == "diagnostic.nested")
        .collect();
    assert_eq!(nested.len(), 1);
    assert_eq!(nested[0]["attributes"]["phase"], "enabled");
    assert!(trace.contains("ws.rpc.server.probe"));
    for secret in [
        "VERY_SECRET",
        "PROMPT_SECRET",
        "TICKET_SECRET",
        "DPOP_SECRET",
    ] {
        assert!(!trace.contains(secret), "{secret}");
    }
    let requests = collector.requests();
    assert!(requests.len() >= 3);
    use prost::Message;
    let decoded:Vec<Value>=requests.iter().map(|r| {
        if protocol=="http/json" {return serde_json::from_slice::<Value>(&r.body).expect("OTLP JSON");}
        assert_eq!(r.headers["content-type"],"application/x-protobuf");
        match r.uri.as_str() {
            "/v1/traces"=>serde_json::to_value(opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest::decode(r.body.as_slice()).expect("protobuf traces")).expect("trace JSON"),
            "/v1/metrics"=>serde_json::to_value(opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest::decode(r.body.as_slice()).expect("protobuf metrics")).expect("metric JSON"),
            "/v1/logs"=>serde_json::to_value(opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest::decode(r.body.as_slice()).expect("protobuf logs")).expect("log JSON"),
            path=>panic!("unexpected path {path}"),
        }
    }).collect();
    let bodies = serde_json::to_string(&decoded).expect("decoded exports");
    assert!(bodies.contains("resourceSpans"));
    assert!(bodies.contains("resourceMetrics"));
    assert!(bodies.contains("resourceLogs"));
    for secret in [
        "VERY_SECRET",
        "PROMPT_SECRET",
        "TICKET_SECRET",
        "DPOP_SECRET",
    ] {
        assert!(!bodies.contains(secret), "{secret}");
    }
    collector.status.store(503, Ordering::Release);
    tracing::dispatcher::with_default(&dispatch, || {
        let _span = tracing::info_span!("offline.span").entered();
    });
    assert!(handle.force_flush().await.is_err());
    let local = std::fs::read_to_string(dir.path().join("server.trace.ndjson"))
        .expect("local trace despite exporter failure");
    assert!(local.contains("offline.span"));
    collector.status.store(204, Ordering::Release);
    handle.shutdown().await.expect("shutdown");
    collector.close().await;
}
