use crate::{
    DeviceService, Error, Result,
    io::{Http, HttpRequest, Secret},
};
use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, Method, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
};
use futures_util::{SinkExt, StreamExt};
use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio_tungstenite::tungstenite::{Message as UpMessage, client::IntoClientRequest};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Read,
    Operate,
}
#[derive(Debug, thiserror::Error)]
#[error("Device proxy authorization failed.")]
pub struct AuthFailure(pub StatusCode);
#[async_trait]
pub trait Authenticator: Send + Sync {
    async fn authorize(
        &self,
        headers: &HeaderMap,
        query: &[(String, String)],
        scope: Scope,
    ) -> std::result::Result<(), AuthFailure>;
}
#[derive(Clone)]
pub struct DeviceHubProxy {
    pub service: DeviceService,
    pub http: Arc<dyn Http>,
    pub auth: Arc<dyn Authenticator>,
    pub sockets: Arc<dyn SocketConnector>,
}
pub type SocketSink = std::pin::Pin<Box<dyn futures_util::Sink<UpMessage, Error = Error> + Send>>;
pub type SocketStream =
    std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<UpMessage>> + Send>>;
pub struct SocketConnection {
    pub sink: SocketSink,
    pub stream: SocketStream,
}
#[async_trait]
pub trait SocketConnector: Send + Sync {
    async fn connect(
        &self,
        url: String,
        headers: BTreeMap<String, Secret>,
        protocols: Vec<Secret>,
    ) -> Result<SocketConnection>;
}
pub struct NativeSocketConnector;
impl DeviceHubProxy {
    pub fn router(self) -> Router {
        Router::new()
            .route("/api/device-hub/{*path}", any(handler))
            .with_state(self)
    }
}
pub fn policy(path: &str, method: &Method, upgrade: bool) -> std::result::Result<Scope, u16> {
    static READ: OnceLock<regex::RegexSet> = OnceLock::new();
    static MUTATE: OnceLock<regex::RegexSet> = OnceLock::new();
    let read=READ.get_or_init(||regex::RegexSet::new([
        r"^/api/devices$",r"^/vendor/serve-sim/api$",r"^/vendor/serve-sim/api/screenshot$",r"^/vendor/serve-sim/api/event-log(/events)?$",r"^/vendor/serve-sim/helper/[^/]+/(stream\.mjpeg|stream\.avcc|config|health|ax|foreground)$",r"^/vendor/serve-sim/helper/[^/]+/panel/(1|3)/stream\.avcc$",r"^/vendor/serve-sim/helper/[^/]+/webrtc/(offer|close)$",r"^/vendor/serve-sim/appstate$",r"^/vendor/serve-emu/api$",r"^/vendor/serve-emu/api/(devices|screenshot|stream-mode|stream-settings|accessibility|fold)$",r"^/vendor/serve-emu/health$",
    ]).expect("static proxy read patterns"));
    let mutable = MUTATE.get_or_init(|| {
        regex::RegexSet::new([
            r"^/vendor/serve-sim/api/screenshot$",
            r"^/vendor/serve-sim/helper/[^/]+/webrtc/(offer|close)$",
            r"^/vendor/serve-emu/webrtc/(offer|close)$",
            r"^/vendor/serve-emu/api/(screenshot|stream-mode|stream-settings|fold)$",
        ])
        .expect("static proxy mutation patterns")
    });
    if upgrade {
        return match path {
            "/api/devices/ws" => Ok(Scope::Read),
            "/vendor/serve-sim/helper/ws" | "/vendor/serve-emu/ws" => Ok(Scope::Operate),
            _ => Err(404),
        };
    }
    if !read.is_match(path) && !mutable.is_match(path) {
        return Err(404);
    }
    let readonly = *method == Method::GET || *method == Method::HEAD;
    if readonly && !read.is_match(path) || !readonly && !mutable.is_match(path) {
        return Err(405);
    }
    Ok(
        if !readonly
            && (path.ends_with("/api/stream-mode")
                || path.ends_with("/api/stream-settings")
                || path.ends_with("/api/fold"))
        {
            Scope::Operate
        } else {
            Scope::Read
        },
    )
}
pub fn forward_headers(headers: &HeaderMap, origin: &str) -> BTreeMap<String, Secret> {
    const DROP: &[&str] = &[
        "host",
        "connection",
        "upgrade",
        "sec-websocket-key",
        "sec-websocket-version",
        "sec-websocket-extensions",
        "sec-websocket-protocol",
        "cookie",
        "authorization",
        "dpop",
        "content-length",
        "accept-encoding",
        "x-forwarded-proto",
    ];
    let mut result = BTreeMap::new();
    for (name, value) in headers {
        if DROP.contains(&name.as_str()) {
            continue;
        }
        if let Ok(value) = value.to_str() {
            result.insert(
                name.as_str().into(),
                Secret::new(if name == "origin" { origin } else { value }),
            );
        }
    }
    result
}
pub fn response_headers(headers: BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut headers = headers
        .into_iter()
        .filter(|(k, _)| {
            ![
                "content-encoding",
                "transfer-encoding",
                "connection",
                "set-cookie",
            ]
            .contains(&k.as_str())
        })
        .collect::<BTreeMap<_, _>>();
    headers.insert("cache-control".into(), "no-store, no-transform".into());
    headers
}

async fn handler(
    State(proxy): State<DeviceHubProxy>,
    upgrade: std::result::Result<
        WebSocketUpgrade,
        axum::extract::ws::rejection::WebSocketUpgradeRejection,
    >,
    request: Request<Body>,
) -> Response {
    match handle(proxy, upgrade.ok(), request).await {
        Ok(response) => response,
        Err(_) => (StatusCode::BAD_GATEWAY, "Device hub request failed").into_response(),
    }
}
async fn handle(
    proxy: DeviceHubProxy,
    upgrade: Option<WebSocketUpgrade>,
    request: Request<Body>,
) -> Result<Response> {
    let path = request
        .uri()
        .path()
        .strip_prefix(crate::DEVICE_HUB_ROUTE_PREFIX)
        .unwrap_or("/");
    let mut query = url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
        .into_owned()
        .collect::<Vec<_>>();
    let ws = request
        .headers()
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    let scope = match policy(path, request.method(), ws) {
        Ok(scope) => scope,
        Err(status) => {
            return Ok((
                StatusCode::from_u16(status).unwrap_or(StatusCode::NOT_FOUND),
                if status == 405 {
                    "Method Not Allowed"
                } else {
                    "Not Found"
                },
            )
                .into_response());
        }
    };
    if let Err(error) = proxy.auth.authorize(request.headers(), &query, scope).await {
        return Ok((error.0, "Device proxy authorization failed").into_response());
    }
    let host = query
        .iter()
        .find(|(k, _)| k == "hostId")
        .map(|(_, v)| v.as_str());
    let Some(ready) = proxy.service.current_readiness(host).await else {
        return Ok((StatusCode::SERVICE_UNAVAILABLE, "Device hub is not running").into_response());
    };
    let resolved_host = host
        .unwrap_or(t3_contracts::device::LOCAL_DEVICE_HOST_ID)
        .to_owned();
    query.retain(|(k, _)| k != "hostId" && k != "wsTicket");
    if [
        "/vendor/serve-sim/api/screenshot",
        "/vendor/serve-emu/api/screenshot",
    ]
    .contains(&path)
        && let Some((_, device)) = query.iter().find(|(k, _)| k == "device")
    {
        let (_, bytes) = proxy
            .service
            .screenshot(Some(&resolved_host), device)
            .await?;
        return Ok((
            [
                ("content-type", "image/png"),
                ("cache-control", "no-store, no-transform"),
            ],
            bytes,
        )
            .into_response());
    }
    let upstream = if let Some(routing) = ready.routing {
        match routing.upstream(path, &query).await? {
            Some(upstream) => upstream,
            None => return Ok(StatusCode::NOT_FOUND.into_response()),
        }
    } else {
        let mut url = crate::io::validated_url(&format!("{}{path}", ready.origin))?;
        if !query.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (k, v) in &query {
                pairs.append_pair(k, v);
            }
        }
        crate::host::Upstream {
            url: url.into(),
            headers: BTreeMap::new(),
            protocols: vec![],
        }
    };
    if ws {
        let Some(upgrade) = upgrade else {
            return Ok(StatusCode::BAD_REQUEST.into_response());
        };
        let mut url = crate::io::validated_url(&upstream.url)?;
        url.set_scheme(if url.scheme() == "https" { "wss" } else { "ws" })
            .map_err(|_| crate::error::invalid("WebSocket URL"))?;
        return Ok(upgrade
            .on_upgrade(move |client| async move {
                let _ = proxy_socket(
                    client,
                    url.to_string(),
                    upstream.headers,
                    upstream.protocols,
                    proxy.sockets,
                )
                .await;
            })
            .into_response());
    }
    let origin = crate::io::validated_url(&upstream.url)?
        .origin()
        .ascii_serialization();
    let mut headers = forward_headers(request.headers(), &origin);
    headers.extend(upstream.headers);
    let method = request.method().clone();
    let readonly = method == Method::GET || method == Method::HEAD;
    let mut upstream_request = HttpRequest::new(method, &upstream.url, Duration::from_secs(60))?;
    upstream_request.timeout = None;
    upstream_request.headers = headers;
    if !readonly {
        upstream_request.body_stream =
            Some(Box::pin(request.into_body().into_data_stream().map(
                |chunk| chunk.map_err(|_| crate::error::invalid("proxy body")),
            )));
    }
    let response = proxy.http.execute(upstream_request).await?;
    let headers = response_headers(response.headers);
    let body = if headers
        .get("content-type")
        .is_some_and(|s| s.starts_with("multipart/x-mixed-replace"))
    {
        crate::mjpeg::repeat_still_frame(response.body)
    } else {
        response.body
    };
    let mut result = Response::builder().status(response.status);
    for (k, v) in headers {
        result = result.header(k, v);
    }
    result
        .body(Body::from_stream(body))
        .map_err(|_| crate::error::invalid("proxy response"))
}
async fn proxy_socket(
    client: WebSocket,
    url: String,
    headers: BTreeMap<String, Secret>,
    protocols: Vec<Secret>,
    connector: Arc<dyn SocketConnector>,
) -> Result<()> {
    let upstream = tokio::time::timeout(
        Duration::from_secs(10),
        connector.connect(url, headers, protocols),
    )
    .await
    .map_err(|_| Error::Unavailable("Device stream connection timed out.".into()))??;
    let (mut client_tx, mut client_rx) = client.split();
    let (mut up_tx, mut up_rx) = (upstream.sink, upstream.stream);
    let to_upstream = async {
        while let Some(Ok(message)) = client_rx.next().await {
            let message = match message {
                Message::Text(v) => UpMessage::Text(v.to_string().into()),
                Message::Binary(v) => UpMessage::Binary(v),
                Message::Ping(v) => UpMessage::Ping(v),
                Message::Pong(v) => UpMessage::Pong(v),
                Message::Close(_) => UpMessage::Close(None),
            };
            if up_tx.send(message).await.is_err() {
                break;
            }
        }
    };
    let to_client = async {
        while let Some(Ok(message)) = up_rx.next().await {
            let message = match message {
                UpMessage::Text(v) => Message::Text(v.to_string().into()),
                UpMessage::Binary(v) => Message::Binary(v),
                UpMessage::Ping(v) => Message::Ping(v),
                UpMessage::Pong(v) => Message::Pong(v),
                UpMessage::Close(_) => Message::Close(None),
                UpMessage::Frame(_) => continue,
            };
            if client_tx.send(message).await.is_err() {
                break;
            }
        }
    };
    tokio::select! {_=to_upstream=>(),_=to_client=>()};
    Ok(())
}

#[async_trait]
impl SocketConnector for NativeSocketConnector {
    async fn connect(
        &self,
        url: String,
        headers: BTreeMap<String, Secret>,
        protocols: Vec<Secret>,
    ) -> Result<SocketConnection> {
        let mut request = url
            .into_client_request()
            .map_err(|_| crate::error::invalid("WebSocket request"))?;
        for (k, v) in headers {
            request.headers_mut().insert(
                http::HeaderName::try_from(k)
                    .map_err(|_| crate::error::invalid("WebSocket header"))?,
                http::HeaderValue::try_from(v.expose())
                    .map_err(|_| crate::error::invalid("WebSocket header"))?,
            );
        }
        if !protocols.is_empty() {
            request.headers_mut().insert(
                "sec-websocket-protocol",
                http::HeaderValue::try_from(
                    protocols
                        .iter()
                        .map(Secret::expose)
                        .collect::<Vec<_>>()
                        .join(", "),
                )
                .map_err(|_| crate::error::invalid("WebSocket protocol"))?,
            );
        }
        let (upstream, _) = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(request),
        )
        .await
        .map_err(|_| Error::Unavailable("Device stream connection timed out.".into()))?
        .map_err(|_| Error::Unavailable("Device stream connection failed.".into()))?;
        let (sink, stream) = upstream.split();
        Ok(SocketConnection {
            sink: Box::pin(
                sink.sink_map_err(|_| Error::Unavailable("Device stream write failed.".into())),
            ),
            stream: Box::pin(
                stream.map(|r| {
                    r.map_err(|_| Error::Unavailable("Device stream read failed.".into()))
                }),
            ),
        })
    }
}
