//! Native host against loopback servers on ephemeral ports and temp directories.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used)]

use futures::{FutureExt, SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::BTreeMap, rc::Rc, sync::Arc, time::Duration};
use t3_client::platform::*;
use t3_platform::{EphemeralSecretStore, HostExtension, NativeConfig, NativePlatform, Timeouts};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};
use tokio_tungstenite::tungstenite::{
    Message,
    handshake::server::{Request as Upgrade, Response as UpgradeResponse},
    http::HeaderValue,
};

struct SharedSecrets(Arc<EphemeralSecretStore>);
impl t3_platform::SecretKeyStore for SharedSecrets {
    fn load(
        &self,
        id: &str,
    ) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, t3_platform::SecretStoreError> {
        self.0.load(id)
    }
    fn store(&self, id: &str, secret: &[u8]) -> Result<(), t3_platform::SecretStoreError> {
        self.0.store(id, secret)
    }
    fn delete(&self, id: &str) -> Result<(), t3_platform::SecretStoreError> {
        self.0.delete(id)
    }
}

fn config(dir: &std::path::Path, secrets: &Arc<EphemeralSecretStore>) -> NativeConfig {
    let mut config = NativeConfig::new(dir, Box::new(SharedSecrets(secrets.clone())));
    config.timeouts = Timeouts {
        connect: Duration::from_secs(5),
        http: Duration::from_millis(400),
        close: Duration::from_millis(200),
    };
    config
}

async fn platform() -> (NativePlatform, tempfile::TempDir, Arc<EphemeralSecretStore>) {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(EphemeralSecretStore::default());
    let platform = NativePlatform::open(config(dir.path(), &secrets))
        .await
        .unwrap();
    (platform, dir, secrets)
}

fn key(namespace: &str, record: &str) -> StorageKey {
    StorageKey::new(
        namespace,
        Some(serde_json::from_value(json!("env-1")).unwrap()),
        record,
    )
}
fn put(key: &StorageKey, value: &[u8]) -> StorageWrite {
    StorageWrite {
        key: key.clone(),
        value: Some(value.to_vec()),
    }
}

#[tokio::test]
async fn storage_round_trips_both_areas_and_survives_reopen() {
    let (platform, dir, secrets) = platform().await;
    let token = key("connection-credential", "route");
    let cache = key("cache.shell", "current");
    platform
        .write(StorageArea::Secure, vec![put(&token, b"secret-bearer")])
        .await
        .unwrap();
    platform
        .write(StorageArea::Ordinary, vec![put(&cache, b"{}")])
        .await
        .unwrap();
    assert_eq!(
        platform
            .get(StorageArea::Ordinary, token.clone())
            .await
            .unwrap(),
        None,
        "areas are separate"
    );
    drop(platform);
    let reopened = NativePlatform::open(config(dir.path(), &secrets))
        .await
        .unwrap();
    assert_eq!(reopened.open_report().discarded_secure_records, 0);
    assert_eq!(
        reopened
            .get(StorageArea::Secure, token.clone())
            .await
            .unwrap(),
        Some(b"secret-bearer".to_vec())
    );
    assert_eq!(
        reopened
            .list(StorageArea::Ordinary, "cache.shell".into())
            .await
            .unwrap(),
        vec![cache]
    );
    assert!(!format!("{reopened:?}").contains("secret-bearer"));
}

#[tokio::test]
async fn committed_writes_announce_their_keys_and_failed_transactions_announce_nothing() {
    let (platform, _dir, _) = platform().await;
    let mut events = platform.events();
    let guard = key("routing-grant", "current");
    let other = key("query", "a");
    let stale = StorageTransaction {
        conditions: vec![StorageCondition {
            area: StorageArea::Ordinary,
            key: guard.clone(),
            expected: Some(b"never".to_vec()),
        }],
        ordinary: vec![put(&other, b"1")],
        secure: vec![put(&guard, b"2")],
    };
    assert!(!platform.transaction(stale).await.unwrap());
    assert_eq!(
        platform
            .get(StorageArea::Ordinary, other.clone())
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        platform
            .get(StorageArea::Secure, guard.clone())
            .await
            .unwrap(),
        None
    );
    let fresh = StorageTransaction {
        conditions: vec![StorageCondition {
            area: StorageArea::Ordinary,
            key: guard.clone(),
            expected: None,
        }],
        ordinary: vec![put(&guard, b"g")],
        secure: vec![put(&other, b"s")],
    };
    assert!(platform.transaction(fresh).await.unwrap());
    platform.event_sender().emit(PlatformEvent::Foreground);
    let mut seen = Vec::new();
    while let Some(event) = events.next().await {
        if event == PlatformEvent::Foreground {
            break;
        }
        seen.push(event);
    }
    assert_eq!(
        seen,
        vec![
            PlatformEvent::StorageChanged(guard),
            PlatformEvent::StorageChanged(other)
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn sleep_and_monotonic_time_follow_the_runtime_clock() {
    let (platform, _dir, _) = platform().await;
    let before = platform.monotonic_ms();
    platform.sleep(60_000).await;
    assert!(platform.monotonic_ms() - before >= 60_000);
    assert!(platform.now_epoch_ms() > 1_600_000_000_000);
    let a = platform.random_bytes(32).unwrap();
    let b = platform.random_bytes(32).unwrap();
    assert_eq!(a.len(), 32);
    assert_ne!(a, b);
}

/// Reads one HTTP/1.1 request head and its fixed-length body.
async fn read_request(stream: &mut TcpStream) -> (String, Vec<u8>) {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    let head_end = loop {
        let read = stream.read(&mut chunk).await.unwrap();
        assert!(read > 0, "client closed before sending a request");
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let head = String::from_utf8(buffer[..head_end].to_vec()).unwrap();
    let length = head
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    let mut body = buffer[head_end..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut chunk).await.unwrap();
        body.extend_from_slice(&chunk[..read]);
    }
    (head, body)
}

async fn bind() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, format!("127.0.0.1:{port}"))
}

fn request(url: String, max: usize) -> HttpRequest {
    HttpRequest {
        method: "POST".into(),
        url,
        headers: BTreeMap::from([("x-probe".into(), "1".into())]),
        body: b"payload".to_vec(),
        credentials: Credentials::Omit,
        max_response_bytes: max,
    }
}

#[tokio::test]
async fn http_sends_method_headers_and_body_and_returns_status_headers_and_body() {
    let (platform, _dir, _) = platform().await;
    let (listener, address) = bind().await;
    let server = async {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (head, body) = read_request(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 5\r\nX-Reply: a\r\nX-Reply: b\r\n\r\nhello")
            .await
            .unwrap();
        (head, body)
    };
    let (response, (head, body)) = tokio::join!(
        platform.http(request(format!("http://{address}/api/x?y=1"), 1024)),
        server
    );
    let response = response.unwrap();
    assert!(head.starts_with("POST /api/x?y=1 HTTP/1.1\r\n"));
    assert!(head.to_ascii_lowercase().contains("x-probe: 1\r\n"));
    assert_eq!(body, b"payload");
    assert_eq!(response.status, 201);
    assert_eq!(response.body, b"hello");
    assert_eq!(
        response.headers.get("x-reply").map(String::as_str),
        Some("a, b")
    );
}

#[tokio::test]
async fn http_does_not_follow_redirects() {
    let (platform, _dir, _) = platform().await;
    let (listener, address) = bind().await;
    let server = async {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        stream
            .write_all(format!("HTTP/1.1 302 Found\r\nLocation: http://{address}/elsewhere\r\nContent-Length: 0\r\n\r\n").as_bytes())
            .await
            .unwrap();
        // A followed redirect would arrive as a second request on this listener.
        tokio::time::timeout(Duration::from_millis(300), listener.accept())
            .await
            .is_err()
    };
    let (response, no_second_request) = tokio::join!(
        platform.http(request(format!("http://{address}/"), 1024)),
        server
    );
    assert_eq!(response.unwrap().status, 302);
    assert!(no_second_request);
}

#[tokio::test]
async fn http_bodies_over_the_cap_fail_with_and_without_a_length() {
    let (platform, _dir, _) = platform().await;
    for declared in [true, false] {
        let (listener, address) = bind().await;
        let server = async {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            let head = if declared {
                "HTTP/1.1 200 OK\r\nContent-Length: 4096\r\n\r\n"
            } else {
                "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n"
            };
            stream.write_all(head.as_bytes()).await.unwrap();
            // The client may hang up mid-body; that is the point.
            let _ = stream.write_all(&[b'x'; 4096]).await;
        };
        let (response, ()) = tokio::join!(
            platform.http(request(format!("http://{address}/"), 1000)),
            server
        );
        assert_eq!(
            response,
            Err(PlatformError::TooLarge),
            "declared={declared}"
        );
    }
    // The cap is inclusive.
    let (listener, address) = bind().await;
    let server = async {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n0123456789")
            .await
            .unwrap();
    };
    let (response, ()) = tokio::join!(
        platform.http(request(format!("http://{address}/"), 10)),
        server
    );
    assert_eq!(response.unwrap().body, b"0123456789");
}

#[tokio::test]
async fn http_to_a_silent_server_times_out_instead_of_hanging() {
    let (platform, _dir, _) = platform().await;
    let (listener, address) = bind().await;
    let (hold, held) = oneshot::channel::<()>();
    let server = async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        let _ = held.await;
        drop(stream);
    };
    let client = async move {
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            platform.http(request(format!("http://{address}/"), 10)),
        )
        .await
        .expect("the HTTP timeout did not fire");
        drop(hold);
        result
    };
    let (result, ()) = tokio::join!(client, server);
    assert!(
        matches!(result, Err(PlatformError::Network(_))),
        "{result:?}"
    );
}

#[tokio::test]
async fn dropping_an_http_request_closes_its_connection() {
    // A long HTTP timeout, so only cancellation can close the connection within the bound.
    let dir = tempfile::tempdir().unwrap();
    let mut patient = config(dir.path(), &Arc::new(EphemeralSecretStore::default()));
    patient.timeouts.http = Duration::from_secs(600);
    let platform = NativePlatform::open(patient).await.unwrap();
    let (listener, address) = bind().await;
    let (received, request_seen) = oneshot::channel();
    let server = async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        received.send(()).unwrap();
        let mut rest = [0_u8; 16];
        // Bounded so a request that keeps running fails the test instead of hanging it.
        tokio::time::timeout(Duration::from_secs(5), stream.read(&mut rest))
            .await
            .expect("the dropped request kept its connection open")
            .unwrap_or(0)
    };
    let client = async move {
        let mut pending = platform.http(request(format!("http://{address}/"), 10));
        tokio::select! {
            _ = &mut pending => panic!("server never answers"),
            _ = request_seen => {}
        }
        drop(pending);
    };
    let ((), eof) = tokio::join!(client, server);
    assert_eq!(eof, 0, "the aborted request's socket reaches EOF");
}

#[tokio::test]
async fn http_rejects_non_http_urls_and_invalid_headers_before_sending() {
    let (platform, _dir, _) = platform().await;
    for url in ["file:///etc/hosts", "ws://127.0.0.1:1/"] {
        assert!(matches!(
            platform.http(request(url.into(), 10)).await,
            Err(PlatformError::InvalidData(_))
        ));
    }
    let mut bad = request("http://127.0.0.1:1/".into(), 10);
    bad.headers.insert("x-bad".into(), "line\nbreak".into());
    assert!(matches!(
        platform.http(bad).await,
        Err(PlatformError::InvalidData(_))
    ));
}

type ServerSocket = tokio_tungstenite::WebSocketStream<TcpStream>;

#[allow(
    clippy::result_large_err,
    reason = "the handshake callback's error type is tungstenite's"
)]
async fn accept(listener: &TcpListener, protocol: Option<&'static str>) -> ServerSocket {
    let (stream, _) = listener.accept().await.unwrap();
    tokio_tungstenite::accept_hdr_async(
        stream,
        move |_: &Upgrade, mut response: UpgradeResponse| {
            if let Some(protocol) = protocol {
                response
                    .headers_mut()
                    .insert("sec-websocket-protocol", HeaderValue::from_static(protocol));
            }
            Ok(response)
        },
    )
    .await
    .unwrap()
}

fn socket_request(address: &str, protocols: &[&str]) -> SocketRequest {
    SocketRequest {
        url: format!("ws://{address}/ws?wsTicket=secret"),
        protocols: protocols.iter().map(|p| (*p).to_owned()).collect(),
    }
}

struct Echo;
impl t3_rpc::Handler for Echo {
    fn handle(&mut self, request: t3_rpc::Request) -> t3_rpc::HandlerOutput {
        let payload = request.payload;
        t3_rpc::HandlerOutput::Unary(
            async move { t3_rpc::Exit::success(json!({ "echo": payload })) }.boxed_local(),
        )
    }
}

#[tokio::test]
async fn websocket_carries_effect_rpc_to_a_real_t3_rpc_server() {
    let (platform, _dir, _) = platform().await;
    let (listener, address) = bind().await;
    let server = async {
        let socket = accept(&listener, None).await;
        let (sink, stream) = socket.split();
        let sink = sink.with(|frame: t3_rpc::Frame| {
            futures::future::ready(Ok::<_, tokio_tungstenite::tungstenite::Error>(
                match frame {
                    t3_rpc::Frame::Text(text) => Message::text(text),
                    t3_rpc::Frame::Binary(bytes) => Message::binary(bytes),
                },
            ))
        });
        let stream = stream.filter_map(|message| {
            futures::future::ready(match message {
                Ok(Message::Text(text)) => Some(Ok(t3_rpc::Frame::Text(text.as_str().to_owned()))),
                Ok(Message::Binary(bytes)) => Some(Ok(t3_rpc::Frame::Binary(bytes.to_vec()))),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
        });
        let mut server = t3_rpc::AsyncServer::new(Box::pin(sink), Box::pin(stream), Echo);
        server.state.register("echo", t3_rpc::CallKind::Unary);
        let _ = server.run().await;
    };
    let client = async {
        let mut socket = platform
            .websocket(socket_request(&address, &[]))
            .await
            .unwrap();
        let request = t3_rpc::ClientMessage::Request(t3_rpc::Request::new(
            7_u64.into(),
            "echo",
            json!({"n": 1}),
        ));
        socket
            .send(t3_rpc::codec::encode(&request).unwrap())
            .await
            .unwrap();
        let reply = socket.receive().await.unwrap().unwrap();
        let messages: Vec<t3_rpc::ServerMessage> = t3_rpc::codec::decode(&reply).unwrap();
        socket.close().await.unwrap();
        assert_eq!(socket.receive().await.unwrap(), None);
        assert_eq!(
            socket.send(t3_rpc::Frame::Text("late".into())).await,
            Err(PlatformError::Closed)
        );
        messages
    };
    let (messages, ()) = tokio::join!(client, server);
    let encoded = serde_json::to_value(&messages[0]).unwrap();
    assert_eq!(encoded["_tag"], "Exit");
    assert_eq!(
        encoded["requestId"],
        serde_json::to_value(t3_rpc::RequestId::from(7_u64)).unwrap()
    );
    assert_eq!(encoded["exit"]["value"], json!({"echo": {"n": 1}}));
}

#[tokio::test]
async fn a_dropped_receive_loses_no_frame_and_peer_close_ends_the_stream() {
    let (platform, _dir, _) = platform().await;
    let (listener, address) = bind().await;
    let (go, start) = oneshot::channel::<()>();
    let server = async {
        let mut socket = accept(&listener, None).await;
        start.await.unwrap();
        socket.send(Message::text("one")).await.unwrap();
        socket.send(Message::binary(vec![2_u8])).await.unwrap();
        socket.close(None).await.unwrap();
    };
    let client = async {
        let mut socket = platform
            .websocket(socket_request(&address, &[]))
            .await
            .unwrap();
        assert!(socket.receive().now_or_never().is_none());
        go.send(()).unwrap();
        let mut frames = Vec::new();
        let drain = async {
            while let Some(frame) = socket.receive().await.unwrap() {
                frames.push(frame);
            }
        };
        tokio::time::timeout(Duration::from_secs(5), drain)
            .await
            .expect("peer close did not end the stream");
        frames
    };
    let (frames, ()) = tokio::join!(client, server);
    assert_eq!(
        frames,
        vec![
            t3_rpc::Frame::Text("one".into()),
            t3_rpc::Frame::Binary(vec![2])
        ]
    );
}

#[tokio::test]
async fn close_returns_within_its_timeout_when_the_peer_never_answers() {
    let (platform, _dir, _) = platform().await;
    let (listener, address) = bind().await;
    let (done, finished) = oneshot::channel::<()>();
    let server = async {
        let socket = accept(&listener, None).await;
        // Hold the TCP connection open without ever reading the close frame.
        let _ = finished.await;
        drop(socket);
    };
    let client = async {
        let mut socket = platform
            .websocket(socket_request(&address, &[]))
            .await
            .unwrap();
        let closed = tokio::time::timeout(Duration::from_secs(5), socket.close()).await;
        done.send(()).unwrap();
        closed
    };
    let (closed, ()) = tokio::join!(client, server);
    assert_eq!(
        closed.expect("close is bounded by the close timeout"),
        Ok(())
    );
}

#[tokio::test]
async fn a_server_choosing_an_unoffered_protocol_is_rejected() {
    let (platform, _dir, _) = platform().await;
    let (listener, address) = bind().await;
    let server = async {
        let _socket = accept(&listener, Some("other")).await;
    };
    let (result, ()) = tokio::join!(
        platform.websocket(socket_request(&address, &["t3"])),
        server
    );
    match result {
        Err(PlatformError::Network(message)) => assert!(!message.contains("secret")),
        Err(other) => panic!("unexpected error {other:?}"),
        Ok(_) => panic!("unoffered protocol accepted"),
    }
    let (listener, address) = bind().await;
    let server = async {
        let _socket = accept(&listener, Some("t3")).await;
    };
    let (result, ()) = tokio::join!(
        platform.websocket(socket_request(&address, &["t3"])),
        server
    );
    assert!(result.is_ok());
}

#[tokio::test]
async fn a_rejected_upgrade_does_not_leak_the_ticket_url() {
    let (platform, _dir, _) = platform().await;
    let (listener, address) = bind().await;
    let server = async {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
    };
    let (result, ()) = tokio::join!(platform.websocket(socket_request(&address, &[])), server);
    match result {
        Err(PlatformError::Network(message)) => {
            assert!(message.contains("401"));
            assert!(!message.contains("secret"));
        }
        Err(other) => panic!("unexpected error {other:?}"),
        Ok(_) => panic!("upgrade should fail"),
    }
}

#[tokio::test]
async fn dpop_key_signs_verifiable_es256_and_persists_across_reopen() {
    let (platform, dir, secrets) = platform().await;
    let platform = Rc::new(platform);
    let jwk = platform.dpop_public_jwk().await.unwrap();
    let thumbprint = platform.dpop_thumbprint().await.unwrap().unwrap();
    // The client's own signer accepts the key and finds the thumbprints equal.
    let signer = t3_client::relay::PlatformDpopSigner::from_host(platform.clone())
        .await
        .unwrap();
    use t3_client::relay::DpopSigner;
    assert_eq!(signer.thumbprint().await.unwrap(), thumbprint);

    let message = b"header.payload".to_vec();
    let signature = platform.dpop_sign(message.clone()).await.unwrap();
    assert_eq!(signature.len(), 64);
    use base64::Engine;
    let decode = |v: &Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(v.as_str().unwrap())
            .unwrap()
    };
    let point = [vec![0x04], decode(&jwk["x"]), decode(&jwk["y"])].concat();
    let public =
        ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, point);
    public.verify(&message, &signature).unwrap();
    assert!(public.verify(b"tampered", &signature).is_err());
    assert!(jwk.get("d").is_none());

    drop(signer);
    drop(platform);
    let reopened = NativePlatform::open(config(dir.path(), &secrets))
        .await
        .unwrap();
    assert_eq!(
        reopened.dpop_thumbprint().await.unwrap().unwrap(),
        thumbprint
    );
}

#[tokio::test]
async fn device_identity_is_a_persisted_uuid_and_desktop_sends_no_relay_device() {
    let (platform, dir, secrets) = platform().await;
    let id = platform.analytics_device_id().await.unwrap().unwrap();
    assert_eq!(id.len(), 36);
    assert_eq!(
        platform.analytics_device_id().await.unwrap(),
        Some(id.clone())
    );
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    assert_eq!(platform.relay_device_id().await.unwrap(), None);
    drop(platform);
    let reopened = NativePlatform::open(config(dir.path(), &secrets))
        .await
        .unwrap();
    assert_eq!(reopened.analytics_device_id().await.unwrap(), Some(id));
}

struct Haptics;
impl HostExtension for Haptics {
    fn host(&self, action: HostAction) -> HostFuture<'_, HostReply> {
        async move {
            match action {
                HostAction::Haptic(_) => Ok(HostReply::Unit),
                other => Err(PlatformError::Unsupported(format!("{other:?}"))),
            }
        }
        .boxed_local()
    }
}

#[tokio::test]
async fn unanswerable_host_actions_are_typed_unsupported_or_go_to_the_shell_extension() {
    let (platform, _dir, _) = platform().await;
    for action in [
        HostAction::Haptic(Haptic::Selection),
        HostAction::NotificationPermission,
        HostAction::PickResources {
            mime: vec![],
            multiple: false,
        },
        HostAction::CloudToken,
        HostAction::Registrations,
        HostAction::ClientPresentation,
    ] {
        let name = format!("{action:?}");
        match platform.host(action).await {
            Err(PlatformError::Unsupported(message)) => assert!(message.contains(&name)),
            other => panic!("{name}: {other:?}"),
        }
    }
    // Review L2: the host applies the client's one predicate for its surface, so a link the
    // client refuses is refused by the host too. Only refusals run here: an allowed link would
    // open a real browser.
    use t3_client::persistence::external_links::{ExternalUrlSurface, safe_external_url};
    assert_eq!(platform.external_url_surface(), ExternalUrlSurface::Desktop);
    for refused in [
        "file:///etc/passwd",
        "mailto:someone@example.com",
        "javascript:alert(1)",
        "zed://file/x",
    ] {
        assert!(safe_external_url(refused, ExternalUrlSurface::Desktop).is_none());
        assert!(
            matches!(
                platform.open_url(refused.into()).await,
                Err(PlatformError::InvalidData(_))
            ),
            "{refused}"
        );
        assert!(
            matches!(
                platform.host(HostAction::OpenUrl(refused.into())).await,
                Err(PlatformError::Denied(_))
            ),
            "{refused}"
        );
    }

    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(EphemeralSecretStore::default());
    let mut configured = config(dir.path(), &secrets);
    configured.extension = Some(Rc::new(Haptics));
    configured.presentation = Some(serde_json::from_value(json!({"label": "Smoke"})).unwrap());
    let extended = NativePlatform::open(configured).await.unwrap();
    assert_eq!(extended.haptic(Haptic::Impact).await, Ok(()));
    let presentation = extended.client_presentation().await.unwrap();
    assert_eq!(presentation.label.map(|l| l.0), Some("Smoke".into()));
    assert!(matches!(
        extended.host(HostAction::StopAudio).await,
        Err(PlatformError::Unsupported(_))
    ));
}

#[tokio::test]
async fn legacy_storage_without_a_shell_source_is_unsupported() {
    let (platform, _dir, _) = platform().await;
    assert!(matches!(
        platform.list_legacy("mobile".into(), "secure".into()).await,
        Err(PlatformError::Unsupported(_))
    ));
}

/// How a saved key reads back: its bytes changed, or an error.
type Damage = fn(Vec<u8>) -> Result<Vec<u8>, t3_platform::SecretStoreError>;

/// A key store whose saved key reads back damaged, until something stores a new one.
struct DamagedSecrets {
    inner: Arc<EphemeralSecretStore>,
    damage: std::sync::Mutex<Option<Damage>>,
}
impl t3_platform::SecretKeyStore for DamagedSecrets {
    fn load(
        &self,
        id: &str,
    ) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, t3_platform::SecretStoreError> {
        let loaded = self.inner.load(id)?;
        match (loaded, *self.damage.lock().unwrap()) {
            (Some(secret), Some(damage)) => {
                damage(secret.to_vec()).map(|s| Some(zeroize::Zeroizing::new(s)))
            }
            (loaded, _) => Ok(loaded),
        }
    }
    fn store(&self, id: &str, secret: &[u8]) -> Result<(), t3_platform::SecretStoreError> {
        self.damage.lock().unwrap().take();
        self.inner.store(id, secret)
    }
}

#[tokio::test]
async fn an_unreadable_or_wrong_length_storage_key_is_replaced_instead_of_failing_every_launch() {
    // Platform review M1. A truncated Android wrapped key reads as `Corrupt`.
    let damages: [Damage; 2] = [
        |secret| Ok(secret[..16].to_vec()),
        |_| Err(t3_platform::SecretStoreError::Corrupt),
    ];
    for damage in damages {
        let (platform, dir, secrets) = platform().await;
        let secure = key("connection-credential", "c");
        platform
            .write(StorageArea::Secure, vec![put(&secure, b"token")])
            .await
            .unwrap();
        drop(platform);
        let damaged = DamagedSecrets {
            inner: secrets.clone(),
            damage: std::sync::Mutex::new(Some(damage)),
        };
        let mut config = config(dir.path(), &secrets);
        config.secrets = Box::new(damaged);
        let reopened = NativePlatform::open(config)
            .await
            .expect("opens with a new key");
        let report = reopened.open_report();
        assert_eq!(report.discarded_secure_records, 1);
        assert!(report.replaced_unreadable_key);
        assert_eq!(
            reopened
                .get(StorageArea::Secure, secure.clone())
                .await
                .unwrap(),
            None
        );
        reopened
            .write(StorageArea::Secure, vec![put(&secure, b"new")])
            .await
            .unwrap();
        assert_eq!(
            reopened
                .get(StorageArea::Secure, secure)
                .await
                .unwrap()
                .as_deref(),
            Some(&b"new"[..])
        );
    }
}

#[tokio::test]
async fn a_file_that_is_not_a_database_is_moved_aside_and_storage_starts_afresh() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(EphemeralSecretStore::default());
    std::fs::write(
        dir.path().join("client-storage.sqlite"),
        b"this is not sqlite at all",
    )
    .unwrap();
    let platform = NativePlatform::open(config(dir.path(), &secrets))
        .await
        .expect("opens a fresh database");
    assert!(platform.open_report().recreated_database);
    let aside: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().contains(".corrupt-"))
        .collect();
    assert_eq!(aside.len(), 1, "the damaged file is kept for diagnosis");
    assert_eq!(
        std::fs::read(aside[0].path()).unwrap(),
        b"this is not sqlite at all"
    );
    let ordinary = key("thread", "t");
    platform
        .write(StorageArea::Ordinary, vec![put(&ordinary, b"x")])
        .await
        .unwrap();
    assert_eq!(
        platform
            .get(StorageArea::Ordinary, ordinary.clone())
            .await
            .unwrap()
            .as_deref(),
        Some(&b"x"[..])
    );
    drop(platform);
    // Re-review N6: the next launch deletes the reported copy, sealed rows and all.
    let platform = NativePlatform::open(config(dir.path(), &secrets))
        .await
        .unwrap();
    assert!(!platform.open_report().recreated_database);
    let aside = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().contains(".corrupt-"))
        .count();
    assert_eq!(aside, 0);
    assert!(
        platform
            .get(StorageArea::Ordinary, ordinary)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn damage_past_the_file_header_is_moved_aside_too() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(EphemeralSecretStore::default());
    {
        let platform = NativePlatform::open(config(dir.path(), &secrets))
            .await
            .unwrap();
        let writes = (0..200)
            .map(|index| put(&key("thread", &format!("t{index}")), &[7_u8; 1024]))
            .collect();
        platform.write(StorageArea::Ordinary, writes).await.unwrap();
    }
    let path = dir.path().join("client-storage.sqlite");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
    }
    // Page 1 (the header and schema) stays valid; the tables' root pages after it do not.
    let mut bytes = std::fs::read(&path).unwrap();
    let page = 4096;
    assert!(bytes.len() > page * 8, "{}", bytes.len());
    bytes[page..page * 4].fill(0xA5);
    std::fs::write(&path, bytes).unwrap();

    let platform = NativePlatform::open(config(dir.path(), &secrets))
        .await
        .expect("opens a fresh database");
    assert!(platform.open_report().recreated_database);
    assert_eq!(
        platform
            .list(StorageArea::Ordinary, "thread".into())
            .await
            .unwrap()
            .len(),
        0
    );
}

/// A key store that fails once with `Unavailable`, or reports its key unreadable once.
struct FlakySecrets {
    inner: Arc<EphemeralSecretStore>,
    unavailable_once: std::sync::atomic::AtomicBool,
    corrupt_once: std::sync::atomic::AtomicBool,
}
impl t3_platform::SecretKeyStore for FlakySecrets {
    fn load(
        &self,
        id: &str,
    ) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, t3_platform::SecretStoreError> {
        use std::sync::atomic::Ordering::SeqCst;
        if self.unavailable_once.swap(false, SeqCst) {
            return Err(t3_platform::SecretStoreError::Unavailable("keystore busy".into()));
        }
        if self.corrupt_once.swap(false, SeqCst) {
            return Err(t3_platform::SecretStoreError::Corrupt);
        }
        self.inner.load(id)
    }
    fn store(&self, id: &str, secret: &[u8]) -> Result<(), t3_platform::SecretStoreError> {
        self.inner.store(id, secret)
    }
    fn delete(&self, id: &str) -> Result<(), t3_platform::SecretStoreError> {
        self.inner.delete(id)
    }
}

#[tokio::test]
async fn a_key_store_that_is_unavailable_once_keeps_every_record_for_the_next_open() {
    // Re-review N1: a temporary Keystore failure must not discard saved credentials.
    let (platform, dir, secrets) = platform().await;
    let secure = key("connection-credential", "c");
    platform
        .write(StorageArea::Secure, vec![put(&secure, b"token")])
        .await
        .unwrap();
    drop(platform);
    let mut flaky = config(dir.path(), &secrets);
    flaky.secrets = Box::new(FlakySecrets {
        inner: secrets.clone(),
        unavailable_once: true.into(),
        corrupt_once: false.into(),
    });
    assert!(
        NativePlatform::open(flaky).await.is_err(),
        "a temporary failure fails this launch"
    );
    let reopened = NativePlatform::open(config(dir.path(), &secrets))
        .await
        .unwrap();
    assert_eq!(reopened.open_report().discarded_secure_records, 0);
    assert_eq!(
        reopened
            .get(StorageArea::Secure, secure)
            .await
            .unwrap()
            .as_deref(),
        Some(&b"token"[..])
    );
}

#[tokio::test]
async fn an_unreadable_key_keeps_its_records_until_it_reads_again() {
    // Second re-review A2/C1: a misread key (a transient Keystore error classed as permanent)
    // must not cost the records; its entry is never overwritten, and its rows come back.
    let (platform, dir, secrets) = platform().await;
    let secure = key("connection-credential", "c");
    platform
        .write(StorageArea::Secure, vec![put(&secure, b"token")])
        .await
        .unwrap();
    drop(platform);
    let mut damaged = config(dir.path(), &secrets);
    damaged.secrets = Box::new(FlakySecrets {
        inner: secrets.clone(),
        unavailable_once: false.into(),
        corrupt_once: true.into(),
    });
    let rekeyed = NativePlatform::open(damaged).await.unwrap();
    let report = rekeyed.open_report();
    assert!(report.replaced_unreadable_key);
    assert_eq!(report.retained_secure_records, 1);
    assert_eq!(report.discarded_secure_records, 0);
    assert_eq!(rekeyed.get(StorageArea::Secure, secure.clone()).await.unwrap(), None);
    drop(rekeyed);
    let reopened = NativePlatform::open(config(dir.path(), &secrets))
        .await
        .unwrap();
    assert_eq!(reopened.open_report().recovered_secure_records, 1);
    assert_eq!(
        reopened
            .get(StorageArea::Secure, secure)
            .await
            .unwrap()
            .as_deref(),
        Some(&b"token"[..])
    );
}

/// Hides the keys it is told to, as a roaming credential or a locked collection does.
struct HidingSecrets {
    inner: Arc<EphemeralSecretStore>,
    hidden: Arc<std::sync::Mutex<Vec<String>>>,
    deleted: Arc<std::sync::Mutex<Vec<String>>>,
}
impl t3_platform::SecretKeyStore for HidingSecrets {
    fn load(
        &self,
        id: &str,
    ) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, t3_platform::SecretStoreError> {
        if self.hidden.lock().unwrap().iter().any(|hidden| hidden == id) {
            return Ok(None);
        }
        self.inner.load(id)
    }
    fn store(&self, id: &str, secret: &[u8]) -> Result<(), t3_platform::SecretStoreError> {
        self.inner.store(id, secret)
    }
    fn delete(&self, id: &str) -> Result<(), t3_platform::SecretStoreError> {
        self.deleted.lock().unwrap().push(id.to_owned());
        self.inner.delete(id)
    }
}

#[tokio::test]
async fn a_missing_key_keeps_its_records_for_when_it_returns() {
    // Second re-review C1: a key that is missing now (a roaming credential, a collection still
    // unlocking) is not proof it is gone; its records wait under a new key's id.
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(EphemeralSecretStore::default());
    let hidden = Arc::new(std::sync::Mutex::new(Vec::new()));
    let deleted = Arc::new(std::sync::Mutex::new(Vec::new()));
    let hiding = || -> NativeConfig {
        let mut config = config(dir.path(), &secrets);
        config.secrets = Box::new(HidingSecrets {
            inner: secrets.clone(),
            hidden: hidden.clone(),
            deleted: deleted.clone(),
        });
        config
    };
    let secure = key("connection-credential", "c");
    let platform = NativePlatform::open(hiding()).await.unwrap();
    platform
        .write(StorageArea::Secure, vec![put(&secure, b"token")])
        .await
        .unwrap();
    drop(platform);
    let first_id = {
        let connection =
            rusqlite::Connection::open(dir.path().join("client-storage.sqlite")).unwrap();
        connection
            .query_row("SELECT value FROM meta WHERE name = 'secure-key-id'", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap()
    };
    hidden.lock().unwrap().push(first_id.clone());
    let while_missing = NativePlatform::open(hiding()).await.unwrap();
    assert_eq!(while_missing.open_report().retained_secure_records, 1);
    assert_eq!(
        while_missing.get(StorageArea::Secure, secure.clone()).await.unwrap(),
        None
    );
    let newer = key("connection-credential", "d");
    while_missing
        .write(StorageArea::Secure, vec![put(&newer, b"newer")])
        .await
        .unwrap();
    drop(while_missing);
    assert!(
        t3_platform::SecretKeyStore::load(&*secrets, &first_id)
            .unwrap()
            .is_some(),
        "never overwritten"
    );
    hidden.lock().unwrap().clear();
    let returned = NativePlatform::open(hiding()).await.unwrap();
    assert_eq!(returned.open_report().recovered_secure_records, 1);
    for (record, value) in [(secure, &b"token"[..]), (newer, &b"newer"[..])] {
        assert_eq!(
            returned
                .get(StorageArea::Secure, record)
                .await
                .unwrap()
                .as_deref(),
            Some(value)
        );
    }
    assert_eq!(*deleted.lock().unwrap(), vec![first_id], "the recovered entry goes");
}

#[tokio::test]
async fn two_instances_opening_a_new_directory_share_one_key() {
    // Second re-review A4: both read "no key" unlocked; only one may create it.
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(EphemeralSecretStore::default());
    let reads = Arc::new(std::sync::Barrier::new(2));
    let opened = Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = ["a", "b"]
        .into_iter()
        .map(|name| {
            let (dir, secrets) = (dir.path().to_owned(), secrets.clone());
            let (reads, opened) = (reads.clone(), opened.clone());
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.block_on(async move {
                    let mut config = config(&dir, &secrets);
                    config.secrets = Box::new(BarrierSecrets {
                        inner: secrets.clone(),
                        first_read: Some(reads).into(),
                    });
                    let platform = NativePlatform::open(config).await.unwrap();
                    opened.wait();
                    platform
                        .write(
                            StorageArea::Secure,
                            vec![put(&key("connection-credential", name), name.as_bytes())],
                        )
                        .await
                        .unwrap();
                });
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let platform = NativePlatform::open(config(dir.path(), &secrets))
        .await
        .unwrap();
    for name in ["a", "b"] {
        assert_eq!(
            platform
                .get(StorageArea::Secure, key("connection-credential", name))
                .await
                .unwrap()
                .as_deref(),
            Some(name.as_bytes()),
            "{name}'s credential"
        );
    }
}

/// Holds each instance's first key read until both have read, as two instances started together.
struct BarrierSecrets {
    inner: Arc<EphemeralSecretStore>,
    first_read: std::sync::Mutex<Option<Arc<std::sync::Barrier>>>,
}
impl t3_platform::SecretKeyStore for BarrierSecrets {
    fn load(
        &self,
        id: &str,
    ) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, t3_platform::SecretStoreError> {
        let loaded = self.inner.load(id);
        if let Some(barrier) = self.first_read.lock().unwrap().take() {
            barrier.wait();
        }
        loaded
    }
    fn store(&self, id: &str, secret: &[u8]) -> Result<(), t3_platform::SecretStoreError> {
        self.inner.store(id, secret)
    }
}

#[tokio::test]
async fn a_leftover_that_cannot_be_deleted_does_not_stop_the_open() {
    // Second re-review C3: a directory of that name, or a file another program holds.
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(EphemeralSecretStore::default());
    std::fs::create_dir(dir.path().join("client-storage.sqlite.corrupt-1")).unwrap();
    assert!(NativePlatform::open(config(dir.path(), &secrets)).await.is_ok());
}

#[tokio::test]
async fn deleting_a_moved_aside_database_deletes_its_key_too() {
    // Second re-review N6: each recreate otherwise leaves one more entry in the OS store.
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(EphemeralSecretStore::default());
    let deleted = Arc::new(std::sync::Mutex::new(Vec::new()));
    let tracked = || -> NativeConfig {
        let mut config = config(dir.path(), &secrets);
        config.secrets = Box::new(HidingSecrets {
            inner: secrets.clone(),
            hidden: Default::default(),
            deleted: deleted.clone(),
        });
        config
    };
    let platform = NativePlatform::open(tracked()).await.unwrap();
    let writes = (0..200)
        .map(|index| put(&key("thread", &format!("t{index}")), &[7_u8; 1024]))
        .collect();
    platform.write(StorageArea::Ordinary, writes).await.unwrap();
    drop(platform);
    let path = dir.path().join("client-storage.sqlite");
    let old_id = {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        connection
            .query_row("SELECT value FROM meta WHERE name = 'secure-key-id'", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap()
    };
    // Damage the records' pages at the end of the file; the key id stays readable.
    let mut bytes = std::fs::read(&path).unwrap();
    let length = bytes.len();
    bytes[length - 4096 * 4..].fill(0xA5);
    std::fs::write(&path, bytes).unwrap();
    let recreated = NativePlatform::open(tracked()).await.unwrap();
    assert!(recreated.open_report().recreated_database);
    assert!(deleted.lock().unwrap().is_empty(), "kept while its copy exists");
    drop(recreated);
    drop(NativePlatform::open(tracked()).await.unwrap());
    assert_eq!(*deleted.lock().unwrap(), vec![old_id]);
}

#[tokio::test]
async fn the_app_location_starts_at_the_launch_link_and_follows_opened_links_and_navigation() {
    use t3_client::navigation::AppUrlRequest;
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(EphemeralSecretStore::default());
    let mut launched = config(dir.path(), &secrets);
    launched.launch_url = Some("t3code://settings".into());
    let platform = NativePlatform::open(launched).await.unwrap();
    assert_eq!(
        platform.app_url().await.unwrap().as_deref(),
        Some("t3code://settings")
    );
    let mut locations = platform.app_url_changes();
    assert_eq!(locations.next().await.unwrap().unwrap().path, "/settings");
    let mut events = platform.events();
    platform
        .open_link("t3code://pair?pairingUrl=https%3A%2F%2Fhost.test%2Fpair%23token%3Dabc".into());
    assert_eq!(
        locations.next().await.unwrap().unwrap().path,
        "/pair?pairingUrl=https%3A%2F%2Fhost.test%2Fpair%23token%3Dabc"
    );
    let Some(PlatformEvent::OpenedLink(link)) = events.next().await else {
        panic!("the link is announced");
    };
    assert_eq!(
        t3_client::connection::pairing_link(&link)
            .unwrap()
            .pairing_url
            .as_deref(),
        Some("https://host.test/pair#token=abc")
    );
    platform
        .navigate_app_url(AppUrlRequest::Go(-1))
        .await
        .unwrap();
    let back = locations.next().await.unwrap().unwrap();
    assert_eq!(back.path, "/settings");
    assert!(back.can_go_forward && !back.can_go_back);
}

/// Records every key id it is asked to store; the first `store` fails, as a declined keychain
/// prompt or a crash does, and every `store` can probe the database lock.
struct RecordingSecrets {
    inner: Arc<EphemeralSecretStore>,
    stored_ids: Arc<std::sync::Mutex<Vec<String>>>,
    fail_first_store: std::sync::atomic::AtomicBool,
    database: Option<std::path::PathBuf>,
    lock_free: Arc<std::sync::Mutex<Option<bool>>>,
}
impl t3_platform::SecretKeyStore for RecordingSecrets {
    fn load(
        &self,
        id: &str,
    ) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, t3_platform::SecretStoreError> {
        // The first read of each open: another process taking the write lock while the OS
        // store may be prompting.
        if let Some(path) = &self.database
            && self.lock_free.lock().unwrap().is_none()
        {
            let other = rusqlite::Connection::open(path).unwrap();
            other.busy_timeout(Duration::from_millis(0)).unwrap();
            let free = other.execute_batch("BEGIN IMMEDIATE; COMMIT;").is_ok();
            *self.lock_free.lock().unwrap() = Some(free);
        }
        self.inner.load(id)
    }
    fn store(&self, id: &str, secret: &[u8]) -> Result<(), t3_platform::SecretStoreError> {
        self.stored_ids.lock().unwrap().push(id.to_owned());
        if self
            .fail_first_store
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(t3_platform::SecretStoreError::Unavailable(
                "prompt declined".into(),
            ));
        }
        self.inner.store(id, secret)
    }
}

#[tokio::test]
async fn a_failed_first_open_reuses_its_key_id_and_the_os_store_is_read_without_the_database_lock() {
    // L7: the key id is committed before the secret is stored, and no prompt holds the lock.
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(EphemeralSecretStore::default());
    let stored_ids = Arc::new(std::sync::Mutex::new(Vec::new()));
    let lock_free = Arc::new(std::sync::Mutex::new(None));
    let recording = |fail| RecordingSecrets {
        inner: secrets.clone(),
        stored_ids: stored_ids.clone(),
        fail_first_store: std::sync::atomic::AtomicBool::new(fail),
        database: Some(dir.path().join("client-storage.sqlite")),
        lock_free: lock_free.clone(),
    };
    let mut first = config(dir.path(), &secrets);
    first.secrets = Box::new(recording(true));
    assert!(NativePlatform::open(first).await.is_err());
    assert_eq!(
        *lock_free.lock().unwrap(),
        Some(true),
        "no write lock while the OS store is read"
    );
    let mut second = config(dir.path(), &secrets);
    second.secrets = Box::new(recording(false));
    let platform = NativePlatform::open(second).await.unwrap();
    let ids = stored_ids.lock().unwrap().clone();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], ids[1], "the same key id, so no orphaned OS entry");
    let secure = key("connection-credential", "c");
    platform
        .write(StorageArea::Secure, vec![put(&secure, b"token")])
        .await
        .unwrap();
    drop(platform);
    // A later open with the same key keeps the records (the stored fingerprint matches).
    let reopened = NativePlatform::open(config(dir.path(), &secrets))
        .await
        .unwrap();
    assert_eq!(reopened.open_report().discarded_secure_records, 0);
    assert!(
        reopened
            .get(StorageArea::Secure, secure)
            .await
            .unwrap()
            .is_some()
    );
}

/// A loopback HTTPS server whose certificate chains to a CA made for this test only.
async fn tls_server() -> (String, Vec<u8>, tokio::task::JoinHandle<()>) {
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca.distinguished_name
        .push(rcgen::DnType::CommonName, "t3 platform test CA");
    ca.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    let ca_der = ca.self_signed(&ca_key).unwrap().der().to_vec();
    let issuer = rcgen::Issuer::new(ca, ca_key);
    let server_key = rcgen::KeyPair::generate().unwrap();
    let mut leaf = rcgen::CertificateParams::new(vec!["127.0.0.1".to_owned()]).unwrap();
    leaf.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    let leaf = leaf.signed_by(&server_key, &issuer).unwrap();
    let server = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![leaf.der().clone()],
        rustls::pki_types::PrivateKeyDer::Pkcs8(server_key.serialize_der().into()),
    )
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server));
    let (listener, address) = bind().await;
    let serve = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            // A client that does not trust the chain aborts the handshake; take the next one.
            let Ok(mut stream) = acceptor.accept(stream).await else {
                continue;
            };
            let mut head = Vec::new();
            let mut chunk = [0_u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                let read = stream.read(&mut chunk).await.unwrap();
                assert!(read > 0, "client closed before sending a request");
                head.extend_from_slice(&chunk[..read]);
            }
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\ntrusted",
                )
                .await
                .unwrap();
            stream.shutdown().await.unwrap();
        }
    });
    (format!("https://{address}/"), ca_der, serve)
}

#[tokio::test]
async fn https_trusts_a_deployment_ca_only_when_it_is_configured() {
    let (url, ca, serve) = tls_server().await;
    let get = |url: &str| HttpRequest {
        method: "GET".into(),
        url: url.to_owned(),
        headers: BTreeMap::new(),
        body: Vec::new(),
        credentials: Credentials::Omit,
        max_response_bytes: 1024,
    };

    // Neither the OS store nor the Mozilla roots know this CA.
    let (untrusting, _dir, _) = platform().await;
    let refused = untrusting.http(get(&url)).await;
    assert!(
        matches!(refused, Err(PlatformError::Network(_))),
        "{refused:?}"
    );

    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(EphemeralSecretStore::default());
    let mut trusting = config(dir.path(), &secrets);
    trusting.extra_roots = vec![ca];
    let trusting = NativePlatform::open(trusting).await.unwrap();
    let response = trusting.http(get(&url)).await.unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"trusted");
    serve.abort();
}
