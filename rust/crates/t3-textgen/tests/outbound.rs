//! Outbound requests behave as Node's fetch: ambient proxy variables are ignored, and the
//! certificates `NODE_EXTRA_CA_CERTS` names are trusted. Environment variables are read when a
//! client is built, so the check runs in a child process with a deployment-like environment.
use async_trait::async_trait;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use t3_textgen::{
    api::{ApiTextRunner, GeneratorSecrets},
    generation::GenerationError,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
};
use tokio_rustls::rustls;

const CHILD_URL: &str = "T3_TEXTGEN_OUTBOUND_CHILD_URL";
const TEST: &str = "requests_ignore_ambient_proxies_and_trust_node_extra_ca_certs";

struct Secrets;
#[async_trait]
impl GeneratorSecrets for Secrets {
    async fn resolve(
        &self,
        _: &t3_contracts::settings_secret_value::SettingsSecretValue,
    ) -> Result<String, GenerationError> {
        Ok("test-key".into())
    }
}

async fn answer(mut stream: impl AsyncRead + AsyncWrite + Unpin) {
    let mut request = Vec::new();
    let mut byte = [0u8; 1];
    while !request.ends_with(b"\r\n\r\n") {
        if stream.read_exact(&mut byte).await.is_err() {
            return;
        }
        request.push(byte[0]);
    }
    let body = br#"{"data":[]}"#;
    let head = format!(
        "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body).await;
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

#[tokio::test]
async fn requests_ignore_ambient_proxies_and_trust_node_extra_ca_certs() {
    if let Ok(url) = std::env::var(CHILD_URL) {
        let config = serde_json::from_value(
            serde_json::json!({"kind":"openaiCompatible","baseUrl":url,"apiKey":"test-key","enabled":true}),
        )
        .expect("config");
        let runner = ApiTextRunner::new(config, Arc::new(Secrets)).expect("runner");
        assert_eq!(runner.list_models().await, Ok(Vec::new()));
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
    let (api, api_connections) =
        listen(Some(tokio_rustls::TlsAcceptor::from(Arc::new(server)))).await;
    let (proxy, proxy_connections) = listen(None).await;
    let proxy = format!("http://127.0.0.1:{proxy}");
    let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"));
    child
        .args([TEST, "--exact", "--test-threads=1"])
        .env(CHILD_URL, format!("https://127.0.0.1:{api}"))
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
    assert_eq!(api_connections.load(Ordering::SeqCst), 1);
}
