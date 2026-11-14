#![allow(clippy::unwrap_used)]
mod common;
use async_trait::async_trait;
use axum::{
    body::Body,
    http::{HeaderMap, Request, StatusCode},
};
use bytes::Bytes;
use common::*;
use futures_util::StreamExt;
use http_body_util::BodyExt;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use t3_contracts::device::DevicePlatform;
use t3_device::{
    Result,
    host::{Routing, Upstream},
    io::{Http, HttpRequest, HttpResponse, Secret},
    mjpeg::{PartTracker, repeat_still_frame},
    proxy::{AuthFailure, Authenticator, DeviceHubProxy, Scope, SocketConnection, SocketConnector},
};
use tokio::sync::Mutex;
use tower::ServiceExt;

struct Auth {
    operate: bool,
    requests: Mutex<Vec<Scope>>,
}
#[async_trait]
impl Authenticator for Auth {
    async fn authorize(
        &self,
        _: &HeaderMap,
        _: &[(String, String)],
        scope: Scope,
    ) -> std::result::Result<(), AuthFailure> {
        self.requests.lock().await.push(scope);
        if scope == Scope::Operate && !self.operate {
            Err(AuthFailure(StatusCode::FORBIDDEN))
        } else {
            Ok(())
        }
    }
}
struct NoSocket;
#[async_trait]
impl SocketConnector for NoSocket {
    async fn connect(
        &self,
        _: String,
        _: BTreeMap<String, Secret>,
        _: Vec<Secret>,
    ) -> Result<SocketConnection> {
        panic!("HTTP test attempted WebSocket connection")
    }
}
struct HttpFixture {
    requests: Mutex<Vec<HttpRequest>>,
    dropped: Arc<AtomicUsize>,
}
struct DropStream {
    chunk: Option<Bytes>,
    dropped: Arc<AtomicUsize>,
}
impl Drop for DropStream {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}
impl futures_util::Stream for DropStream {
    type Item = Result<Bytes>;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::task::Poll::Ready(self.chunk.take().map(Ok))
    }
}
#[async_trait]
impl Http for HttpFixture {
    async fn execute(&self, mut request: HttpRequest) -> Result<HttpResponse> {
        if let Some(mut body) = request.body_stream.take() {
            let mut collected = Vec::new();
            while let Some(chunk) = body.next().await {
                collected.extend_from_slice(&chunk?);
            }
            request.body = Some(collected.into());
        }
        self.requests.lock().await.push(request);
        Ok(HttpResponse {
            status: 200,
            headers: BTreeMap::from([
                ("content-type".into(), "text/plain".into()),
                ("set-cookie".into(), "upstream-secret".into()),
            ]),
            body: Box::pin(DropStream {
                chunk: Some(Bytes::from_static(b"frame")),
                dropped: self.dropped.clone(),
            }),
        })
    }
}
async fn fixture(operate: bool) -> (axum::Router, Arc<HttpFixture>, Arc<Auth>) {
    let http = Arc::new(HttpFixture {
        requests: Mutex::new(vec![]),
        dropped: Arc::new(AtomicUsize::new(0)),
    });
    let host = FakeHost::new(
        "local",
        FakeOps::new("local", DevicePlatform::Ios, true),
        FakeCommands::encoder(),
        None,
    );
    let service = service(vec![host], true, false).await;
    let auth = Arc::new(Auth {
        operate,
        requests: Mutex::new(vec![]),
    });
    (
        DeviceHubProxy {
            service,
            http: http.clone(),
            auth: auth.clone(),
            sockets: Arc::new(NoSocket),
        }
        .router(),
        http,
        auth,
    )
}
#[tokio::test]
async fn http_proxy_strips_tickets_and_credentials_and_drops_completed_response() {
    let (router, http, auth) = fixture(false).await;
    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/device-hub/api/devices?hostId=local&wsTicket=private-ticket")
                .header("authorization", "Bearer environment-token")
                .header("cookie", "private-cookie")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key("set-cookie"));
    assert_eq!(
        response.headers()["cache-control"],
        "no-store, no-transform"
    );
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        "frame"
    );
    assert_eq!(http.dropped.load(Ordering::SeqCst), 1);
    let requests = http.requests.lock().await;
    assert_eq!(requests[0].url.as_str(), "http://hub.test/api/devices");
    assert!(!requests[0].headers.contains_key("authorization"));
    assert!(!requests[0].headers.contains_key("cookie"));
    assert_eq!(auth.requests.lock().await.as_slice(), [Scope::Read]);
}
#[tokio::test]
async fn http_proxy_requires_operate_for_fold_and_preserves_json_and_same_origin() {
    let (router, http, _auth) = fixture(false).await;
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/device-hub/vendor/serve-emu/api/fold")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(http.requests.lock().await.is_empty());
    let (router, http, _auth) = fixture(true).await;
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/device-hub/vendor/serve-emu/api/fold")
                .header("origin", "https://remote.t3.test")
                .header("x-forwarded-proto", "https")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"fold":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.into_body().collect().await.unwrap();
    let requests = http.requests.lock().await;
    assert_eq!(requests[0].headers["origin"].expose(), "http://hub.test");
    assert_eq!(
        requests[0].headers["content-type"].expose(),
        "application/json"
    );
    assert!(!requests[0].headers.contains_key("x-forwarded-proto"));
    assert_eq!(requests[0].body.as_deref().unwrap(), br#"{"fold":true}"#);
}
#[tokio::test]
async fn blocked_proxy_paths_never_authenticate_or_reach_upstream() {
    for (path, method, expected) in [
        ("/vendor/serve-sim/exec", "POST", 404),
        ("/vendor/serve-emu/api", "POST", 405),
        ("/vendor/serve-emu/webrtc/offer", "GET", 405),
        ("/vendor/serve-sim/helper/../config", "GET", 404),
        ("/vendor/serve-sim/helper/%2e%2e/config", "GET", 404),
        ("/vendor/serve-sim/helper/%2E%2e/config", "GET", 404),
        ("/vendor/serve-sim/helper/%252e%252E/config", "GET", 404),
        (
            "/vendor/serve-sim/helper/..\\..\\anything/config",
            "GET",
            404,
        ),
        ("/vendor/serve-sim/helper/%5c..%5c/config", "GET", 404),
    ] {
        let (router, http, auth) = fixture(true).await;
        let response = router
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/api/device-hub{path}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), expected);
        assert!(http.requests.lock().await.is_empty());
        assert!(auth.requests.lock().await.is_empty());
    }
}
#[tokio::test]
async fn screenshot_uses_service_instead_of_vendor_route() {
    let (router, http, _auth) = fixture(false).await;
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/device-hub/vendor/serve-sim/api/screenshot?device=phone")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/png");
    assert_eq!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .as_ref(),
        [137, 80, 78, 71]
    );
    assert!(http.requests.lock().await.is_empty());
}
struct Routed;
#[async_trait]
impl Routing for Routed {
    async fn upstream(&self, path: &str, query: &[(String, String)]) -> Result<Option<Upstream>> {
        assert!(
            !query
                .iter()
                .any(|(k, _)| ["hostId", "wsTicket"].contains(&k.as_str()))
        );
        Ok((path == "/vendor/serve-sim/api").then(|| Upstream {
            url: "https://preview.test/api?device=UDID-1".into(),
            headers: BTreeMap::from([(
                "authorization".into(),
                Secret::new("Bearer preview-secret"),
            )]),
            protocols: vec![],
        }))
    }
}
#[tokio::test]
async fn host_router_selects_its_own_upstream_and_credentials() {
    let http = Arc::new(HttpFixture {
        requests: Mutex::new(vec![]),
        dropped: Arc::new(AtomicUsize::new(0)),
    });
    let mut host = FakeHost::new(
        "local",
        FakeOps::new("local", DevicePlatform::Ios, true),
        FakeCommands::encoder(),
        None,
    );
    Arc::get_mut(&mut host).unwrap().ready.routing = Some(Arc::new(Routed));
    let service = service(vec![host], true, false).await;
    let router = DeviceHubProxy {
        service,
        http: http.clone(),
        auth: Arc::new(Auth {
            operate: false,
            requests: Mutex::new(vec![]),
        }),
        sockets: Arc::new(NoSocket),
    }
    .router();
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/device-hub/vendor/serve-sim/api?device=session-1&wsTicket=private")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.into_body().collect().await.unwrap();
    assert_eq!(
        http.requests.lock().await[0].url.as_str(),
        "https://preview.test/api?device=UDID-1"
    );
    assert_eq!(
        http.requests.lock().await[0].headers["authorization"].expose(),
        "Bearer preview-secret"
    );
    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/device-hub/vendor/serve-sim/helper/other/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(http.requests.lock().await.len(), 1);
}

fn part(body: &str) -> Vec<u8> {
    format!(
        "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n{body}\r\n",
        body.len()
    )
    .into_bytes()
}
#[test]
fn multipart_tracking_handles_every_split_and_retains_last_complete_part() {
    let p = part("frame-one");
    for split in 0..=p.len() {
        let mut tracker = PartTracker::default();
        tracker.feed(&p[..split]);
        if split > 0 && split < p.len() {
            assert!(tracker.take_still().is_none());
        }
        tracker.feed(&p[split..]);
        assert_eq!(tracker.take_still().unwrap().as_ref(), p);
    }
    let mut tracker = PartTracker::default();
    let both = [part("one"), part("two")].concat();
    tracker.feed(&both);
    assert_eq!(tracker.take_still().unwrap().as_ref(), part("two"));
}
#[test]
fn malformed_multipart_disables_tracking_and_bounds_retention() {
    for bad in [
        b"bad framing\r\n\r\n".to_vec(),
        b"--frame\r\nContent-Length: 999999999\r\n\r\n".to_vec(),
        b"--frame\r\nContent-Length: 1\r\n\r\nxZZ".to_vec(),
        vec![b'x'; 16 * 1024 + 5],
    ] {
        let mut tracker = PartTracker::default();
        tracker.feed(&bad);
        tracker.feed(&part("valid"));
        assert!(tracker.take_still().is_none());
    }
}
#[tokio::test(start_paused = true)]
async fn still_frame_repeats_once_and_preserves_pending_input() {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Bytes>();
    let upstream =
        futures_util::stream::unfold(
            rx,
            |mut rx| async move { rx.recv().await.map(|b| (Ok(b), rx)) },
        );
    let mut stream = repeat_still_frame(Box::pin(upstream));
    let one = Bytes::from(part("one"));
    tx.send(one.clone()).unwrap();
    assert_eq!(stream.next().await.unwrap().unwrap(), one);
    assert_eq!(stream.next().await.unwrap().unwrap(), one);
    let mut next = Box::pin(stream.next());
    assert!(futures_util::poll!(&mut next).is_pending());
    let two = Bytes::from(part("two"));
    tx.send(two.clone()).unwrap();
    assert_eq!(next.await.unwrap().unwrap(), two);
    drop(tx);
    assert!(stream.next().await.is_none());
}
#[tokio::test]
async fn real_loopback_proxy_preserves_binary_admission_and_input_frames() {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message;
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream.local_addr().unwrap();
    let hub = tokio::spawn(async move {
        let (socket, _) = upstream.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async_with_config(
            socket,
            Some(
                tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
                    .max_frame_size(None)
                    .max_message_size(None),
            ),
        )
        .await
        .unwrap();
        socket
            .send(Message::Binary(Bytes::from_static(&[0x83])))
            .await
            .unwrap();
        for expected in [[3, 1], [3, 2]] {
            let frame = socket.next().await.unwrap().unwrap();
            assert_eq!(frame.into_data().as_ref(), expected);
            socket
                .send(Message::Binary(Bytes::copy_from_slice(&expected)))
                .await
                .unwrap();
        }
        let payload = socket.next().await.unwrap().unwrap().into_data();
        assert_eq!(payload.len(), 17 * 1024 * 1024);
        assert!(payload.iter().all(|byte| *byte == 7));
        socket.send(Message::Binary(payload)).await.unwrap();
        let _ = socket.close(None).await;
    });
    let mut host = FakeHost::new(
        "local",
        FakeOps::new("local", DevicePlatform::Ios, true),
        FakeCommands::encoder(),
        None,
    );
    Arc::get_mut(&mut host).unwrap().ready.origin = format!("http://{upstream_addr}");
    let service = service(vec![host], true, false).await;
    let router = DeviceHubProxy {
        service,
        http: FakeHttp::new([]),
        auth: Arc::new(Auth {
            operate: true,
            requests: Mutex::new(vec![]),
        }),
        sockets: Arc::new(t3_device::proxy::NativeSocketConnector),
    }
    .router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopping.await;
            })
            .await
            .unwrap();
    });
    let (mut client, _) = tokio_tungstenite::connect_async_with_config(
        format!(
            "ws://{address}/api/device-hub/vendor/serve-sim/helper/ws?device=phone&wsTicket=private"
        ),
        Some(
            tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
                .max_frame_size(None)
                .max_message_size(None),
        ),
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        client.next().await.unwrap().unwrap().into_data().as_ref(),
        [0x83]
    );
    for frame in [[3, 1], [3, 2]] {
        client
            .send(Message::Binary(Bytes::copy_from_slice(&frame)))
            .await
            .unwrap();
        assert_eq!(
            client.next().await.unwrap().unwrap().into_data().as_ref(),
            frame
        );
    }
    let payload = Bytes::from(vec![7; 17 * 1024 * 1024]);
    client.send(Message::Binary(payload.clone())).await.unwrap();
    assert_eq!(client.next().await.unwrap().unwrap().into_data(), payload);
    let _ = client.close(None).await;
    drop(client);
    hub.await.unwrap();
    let _ = stop.send(());
    server.await.unwrap();
}

#[tokio::test]
async fn cancelling_connecting_websocket_drops_owned_tcp_connection() {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let connecting = tokio::spawn(async move {
        t3_device::proxy::NativeSocketConnector
            .connect(
                format!("ws://{address}"),
                BTreeMap::from([("authorization".into(), Secret::new("Bearer fixture"))]),
                vec![],
            )
            .await
    });
    let (mut accepted, _) = listener.accept().await.unwrap();
    let mut request = [0u8; 4096];
    let count = accepted.read(&mut request).await.unwrap();
    assert!(count > 0);
    connecting.abort();
    let Err(error) = connecting.await else {
        panic!("connection completed before cancellation")
    };
    assert!(error.is_cancelled());
    assert_eq!(accepted.read(&mut request).await.unwrap(), 0);
}

#[tokio::test]
async fn upstream_payload_ceiling_rejects_an_oversize_header_before_its_body() {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        let mut header = vec![0x82, 127];
        header.extend_from_slice(&(100u64 * 1024 * 1024 + 1).to_be_bytes());
        socket.get_mut().write_all(&header).await.unwrap();
        let _ = stopping.await;
    });
    let mut connection = t3_device::proxy::NativeSocketConnector
        .connect(format!("ws://{address}"), BTreeMap::new(), vec![])
        .await
        .unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), connection.stream.next())
        .await
        .expect("must reject the declared length without waiting for a body");
    assert!(result.unwrap().is_err());
    let _ = stop.send(());
    server.await.unwrap();
}

struct PendingSocket;
#[async_trait]
impl SocketConnector for PendingSocket {
    async fn connect(
        &self,
        _: String,
        _: BTreeMap<String, Secret>,
        _: Vec<Secret>,
    ) -> Result<SocketConnection> {
        use futures_util::SinkExt;
        Ok(SocketConnection {
            sink: Box::pin(
                futures_util::sink::drain()
                    .sink_map_err(|never: std::convert::Infallible| match never {}),
            ),
            stream: Box::pin(futures_util::stream::pending()),
        })
    }
}
#[tokio::test]
async fn client_payload_ceiling_rejects_an_oversize_header_before_its_body() {
    use tokio::io::AsyncWriteExt;
    let host = FakeHost::new(
        "local",
        FakeOps::new("local", DevicePlatform::Ios, true),
        FakeCommands::encoder(),
        None,
    );
    let router = DeviceHubProxy {
        service: service(vec![host], true, false).await,
        http: FakeHttp::new([]),
        auth: Arc::new(Auth {
            operate: false,
            requests: Mutex::new(vec![]),
        }),
        sockets: Arc::new(PendingSocket),
    }
    .router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopping.await;
            })
            .await
            .unwrap();
    });
    let (mut client, _) =
        tokio_tungstenite::connect_async(format!("ws://{address}/api/device-hub/api/devices/ws"))
            .await
            .unwrap();
    let mut header = vec![0x82, 0xff];
    header.extend_from_slice(&(100u64 * 1024 * 1024 + 1).to_be_bytes());
    header.extend_from_slice(&[0; 4]);
    client.get_mut().write_all(&header).await.unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), client.next())
        .await
        .expect("must reject the declared length without waiting for a body");
    assert!(!matches!(
        result,
        Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(_)
            | tokio_tungstenite::tungstenite::Message::Text(_)))
    ));
    drop(client);
    let _ = stop.send(());
    server.await.unwrap();
}
