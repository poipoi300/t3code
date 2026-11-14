//! Outbound requests behave as Node's fetch: ambient proxy variables are ignored, and the
//! certificates `NODE_EXTRA_CA_CERTS` names are trusted. Environment variables are read when a
//! client is built, so the check runs in a child process with a deployment-like environment.
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use t3_config::config::{SignalEndpoint, SignalExport, TelemetryConfig};
use t3_contracts::server::{OtlpEndpointCheckInput, OtlpEndpointCheckResult, OtlpSignal};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
};
use tokio_rustls::rustls;

const CHILD_URL: &str = "T3_TELEMETRY_OUTBOUND_CHILD_URL";
const TEST: &str = "exports_ignore_ambient_proxies_and_trust_node_extra_ca_certs";

async fn answer(mut stream: impl AsyncRead + AsyncWrite + Unpin) {
    let mut request = Vec::new();
    let mut byte = [0u8; 1];
    while !request.ends_with(b"\r\n\r\n") {
        if stream.read_exact(&mut byte).await.is_err() {
            return;
        }
        request.push(byte[0]);
    }
    let length = String::from_utf8_lossy(&request)
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    let mut body = vec![0; length];
    if stream.read_exact(&mut body).await.is_err() {
        return;
    }
    let _ = stream
        .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
        .await;
    let _ = stream.shutdown().await;
}

/// Counts connections and answers each request, over TLS when an acceptor is given.
async fn listen(tls: Option<tokio_rustls::TlsAcceptor>) -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("address").port();
    let connections = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&connections);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
            let tls = tls.clone();
            tokio::spawn(async move {
                match tls {
                    Some(acceptor) => {
                        if let Ok(stream) = acceptor.accept(stream).await {
                            answer(stream).await;
                        }
                    }
                    None => answer(stream).await,
                }
            });
        }
    });
    (port, connections)
}

fn telemetry(url: &str) -> TelemetryConfig {
    let export = SignalExport {
        protocol: "http/json".into(),
        headers: None,
        export_interval_ms: 600_000,
    };
    TelemetryConfig {
        disabled: false,
        warnings: vec![],
        resource_attributes: Default::default(),
        traces: Some(SignalEndpoint {
            url: format!("{url}/v1/traces"),
            export: export.clone(),
            source: "settings".into(),
            variable: None,
        }),
        metrics: None,
        logs: None,
        default_export: export,
    }
}

async fn child(url: &str) {
    let config = telemetry(url);
    let checked = t3_telemetry::otlp::OtlpEndpointCheck::new(config.clone())
        .expect("check")
        .check(&OtlpEndpointCheckInput {
            signal: OtlpSignal::Traces,
            url: format!("{url}/v1/traces").into(),
        })
        .await;
    assert!(
        matches!(checked, OtlpEndpointCheckResult::Accepted(_)),
        "{checked:?}"
    );
    let directory = tempfile::tempdir().expect("temp");
    let (dispatch, handle) = t3_telemetry::logging::build(t3_telemetry::logging::LoggingOptions {
        trace: t3_telemetry::logging::TraceOptions {
            file_path: directory.path().join("server.trace.ndjson"),
            max_bytes: 100_000,
            max_files: 1,
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
        let _span = tracing::info_span!("exported.over.tls").entered();
    });
    handle
        .force_flush()
        .await
        .expect("OTLP export reached the collector");
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn exports_ignore_ambient_proxies_and_trust_node_extra_ca_certs() {
    if let Ok(url) = std::env::var(CHILD_URL) {
        child(&url).await;
        return;
    }
    let certified =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).expect("certificate");
    let directory = tempfile::tempdir().expect("temp");
    let extra = directory.path().join("extra-ca.pem");
    std::fs::write(&extra, certified.cert.pem()).expect("write CA");
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let server = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("protocols")
        .with_no_client_auth()
        .with_single_cert(
            vec![certified.cert.der().clone()],
            rustls::pki_types::PrivateKeyDer::Pkcs8(certified.signing_key.serialize_der().into()),
        )
        .expect("server certificate");
    let (collector, collector_connections) =
        listen(Some(tokio_rustls::TlsAcceptor::from(Arc::new(server)))).await;
    let (proxy, proxy_connections) = listen(None).await;
    let proxy = format!("http://127.0.0.1:{proxy}");
    let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"));
    child
        .args([TEST, "--exact", "--test-threads=1"])
        .env(CHILD_URL, format!("https://127.0.0.1:{collector}"))
        .env("NODE_EXTRA_CA_CERTS", &extra)
        .env("HTTPS_PROXY", &proxy)
        .env("HTTP_PROXY", &proxy)
        .env("ALL_PROXY", &proxy)
        .env_remove("NO_PROXY")
        .env_remove("no_proxy");
    let output = tokio::task::spawn_blocking(move || child.output())
        .await
        .expect("join")
        .expect("child test");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(proxy_connections.load(Ordering::SeqCst), 0);
    // The endpoint check (async client) and the trace export (blocking client) each connect.
    assert!(collector_connections.load(Ordering::SeqCst) >= 2);
}
