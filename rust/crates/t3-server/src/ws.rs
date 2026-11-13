#![cfg(feature = "local-services")]
use crate::{
    registry::{Context, Response as RpcResponse, ServerDefect, Work},
    services::ServiceRegistry,
};
use axum::{
    Json,
    extract::{
        ConnectInfo, OriginalUri, State,
        ws::{
            CloseFrame, Message, WebSocket, WebSocketUpgrade, rejection::WebSocketUpgradeRejection,
        },
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures::{FutureExt, StreamExt, future::poll_fn};
use serde_json::{Value, json};
use std::{net::SocketAddr, panic::AssertUnwindSafe, sync::Arc, task::Poll, time::Duration};
use t3_rpc::{CallKind, Exit, Frame, RequestId, Server, ServerAction, ServerMessage};

/// The TypeScript server keeps the `ws` library default of 100 MiB per incoming message.
const MAX_MESSAGE: usize = t3_rpc::transport::MAX_INCOMING_FRAME_BYTES;
/// RFC 6455 close code 1009: message too big.
const CLOSE_TOO_BIG: u16 = 1009;
/// A peer that never reads must not keep the connection task, and shutdown, waiting.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
const CLIENT_CAPABILITIES: &str = "clientCapabilities";
const SECRET_REFERENCES: &str = "settingsSecretReferences";

fn query_pairs(uri: &axum::http::Uri) -> Vec<(String, String)> {
    url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
        .into_owned()
        .collect()
}

fn protocol_incompatible() -> Response {
    (
        StatusCode::UPGRADE_REQUIRED,
        Json(json!({
            "code": "orchestration_protocol_incompatible",
            "orchestrationProtocolVersion": 2,
            "message": "Update this client to support orchestration protocol 2."
        })),
    )
        .into_response()
}

pub async fn upgrade(
    State(s): State<Arc<ServiceRegistry>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let query = query_pairs(&uri);
    // The first occurrence wins, as URLSearchParams.get does.
    let first = |name: &str| {
        query
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    if first("orchestrationProtocol") != Some("2") {
        return protocol_incompatible();
    }
    let request = crate::adapters::auth::request("GET", &uri, &headers, Some(remote));
    let session = match s.local.auth.websocket(&request).await {
        Ok(session) => session,
        Err(e) => return crate::http::auth_error(e),
    };
    // A plain request fails here, after the protocol and credential checks, as in the source.
    let Ok(upgrade) = upgrade else {
        return (StatusCode::UPGRADE_REQUIRED, "WebSocket upgrade required").into_response();
    };
    let frame_parts = query.iter().any(|(k, _)| k == "frameParts");
    let secret_references = first(CLIENT_CAPABILITIES).is_some_and(|v| {
        v.split(',')
            .any(|capability| capability == SECRET_REFERENCES)
    });
    let guard = s.connections.token();
    upgrade
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| async move {
            let _guard = guard;
            run(socket, s, session, headers, frame_parts, secret_references).await;
        })
}

/// How the receive loop ended.
enum End {
    /// The peer went away or the stream failed; nothing more can be sent.
    Gone,
    /// An incoming message, or a set of parts, exceeded the limit.
    TooBig,
}

async fn run(
    mut socket: WebSocket,
    s: Arc<ServiceRegistry>,
    session: crate::adapters::auth::VerifiedSession,
    headers: HeaderMap,
    frame_parts: bool,
    secret_references: bool,
) {
    if s.local.auth.connected(&session.session_id).await.is_err() {
        return;
    }
    let context = Context {
        services: s.clone(),
        scopes: session.scopes.clone(),
        session_id: session.session_id.clone(),
        secret_references,
        call: crate::registry::CallScope::socket(crate::registry::SocketScope::new()),
    };
    // A panic anywhere in the loop must still reach mark_disconnected below.
    let end = AssertUnwindSafe(async {
        tokio::select! {
            _ = s.shutdown.cancelled() => None,
            _ = s.local.auth.invalidation(&session) => None,
            end = connection(&mut socket, context, headers, frame_parts) => Some(end),
        }
    })
    .catch_unwind()
    .await;
    let close = match end {
        Ok(Some(End::TooBig)) => Some(Message::Close(Some(CloseFrame {
            code: CLOSE_TOO_BIG,
            reason: "message too big".into(),
        }))),
        Ok(Some(End::Gone)) => None,
        Ok(None) | Err(_) => Some(Message::Close(None)),
    };
    if let Some(close) = close {
        close_socket(&mut socket, close).await;
    }
    drop(socket);
    s.local.auth.disconnected(&session.session_id).await;
}

/// Sends the close frame, then reads until the peer answers it. Dropping a socket that still
/// has unread input resets the connection and can destroy the close frame before the peer
/// reads it, so the input is drained, within a bound.
async fn close_socket(socket: &mut WebSocket, close: Message) {
    let work = async {
        if socket.send(close).await.is_err() {
            return;
        }
        while let Some(Ok(message)) = socket.recv().await {
            if matches!(message, Message::Close(_)) {
                break;
            }
        }
    };
    let _ = tokio::time::timeout(CLOSE_TIMEOUT, work).await;
}

struct Job {
    id: RequestId,
    work: Work,
}

fn panic_defect(method_context: &str) -> Exit {
    ServerDefect::Internal {
        message: format!("{method_context} panicked"),
    }
    .exit()
}

fn upgrade_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_owned())))
        .collect()
}

async fn send_frames(
    socket: &mut WebSocket,
    message: &ServerMessage<Value>,
    frame_parts: bool,
) -> Result<(), ()> {
    let frame = t3_rpc::codec::encode(message).map_err(|_| ())?;
    let parts = if frame_parts {
        t3_rpc::framing::split(frame)
    } else {
        vec![frame]
    };
    for part in parts {
        let message = match part {
            Frame::Text(t) => Message::Text(t.into()),
            Frame::Binary(b) => Message::Binary(b.into()),
        };
        socket.send(message).await.map_err(|_| ())?;
    }
    Ok(())
}

async fn connection(
    socket: &mut WebSocket,
    ctx: Context,
    headers: HeaderMap,
    frame_parts: bool,
) -> End {
    let mut server = Server::new();
    for m in t3_contracts::rpc::WS_METHODS {
        server.register(
            m.tag,
            if m.streams {
                CallKind::Stream
            } else {
                CallKind::Unary
            },
        );
    }
    server.set_upgrade_headers(upgrade_headers(&headers));
    let mut jobs: Vec<Job> = Vec::new();
    let mut pending_parts = String::new();
    let sockets = ctx.call.socket_scope();
    let end = 'serve: loop {
        while let Some(action) = server.pop_action() {
            match action {
                ServerAction::Dispatch(request) => {
                    let id = request.id.clone();
                    let mut call = ctx.clone();
                    call.call = ctx.call.for_request(id.clone());
                    let dispatched = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        ctx.services
                            .handlers
                            .dispatch(call, &request.tag, request.payload)
                    }));
                    let result = match dispatched {
                        Ok(Ok(work)) => server.started(&id).map(|()| jobs.push(Job { id, work })),
                        Ok(Err(exit)) => {
                            sockets.release(&id);
                            server.finish(&id, exit)
                        }
                        Err(_) => {
                            sockets.release(&id);
                            server.finish(&id, panic_defect("RPC dispatch"))
                        }
                    };
                    if result.is_err() {
                        break 'serve End::Gone;
                    }
                }
                ServerAction::Cancel(id) => {
                    sockets.release(&id);
                    jobs.retain(|job| job.id != id);
                }
                ServerAction::Ended => {}
            }
        }
        let mut failed = false;
        while let Some(message) = server.pop_outgoing() {
            if send_frames(socket, &message, frame_parts).await.is_err() {
                failed = true;
                break;
            }
        }
        if failed {
            break 'serve End::Gone;
        }
        tokio::select! {
            incoming = socket.recv() => {
                let frame = match incoming {
                    Some(Ok(Message::Text(t))) => Frame::Text(t.to_string()),
                    Some(Ok(Message::Binary(b))) => Frame::Binary(b.to_vec()),
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                    // The transport's own size limit fires before this loop sees the message.
                    Some(Err(error)) if error.to_string().contains("Message too long") => {
                        break End::TooBig;
                    }
                    _ => break End::Gone,
                };
                let frame = match frame {
                    Frame::Text(text) => {
                        let partial = text.starts_with('\u{1e}');
                        let part = text.strip_prefix('\u{1e}').unwrap_or(&text);
                        if pending_parts.len().saturating_add(part.len()) > MAX_MESSAGE {
                            break End::TooBig;
                        }
                        pending_parts.push_str(part);
                        if partial {
                            continue;
                        }
                        Frame::Text(std::mem::take(&mut pending_parts))
                    }
                    binary => binary,
                };
                match t3_rpc::codec::decode(&frame) {
                    Ok(messages) => {
                        for message in messages {
                            if let t3_rpc::ClientMessage::Ack { request_id } = &message {
                                sockets.acknowledged(request_id);
                            }
                            server.receive(message);
                        }
                    }
                    Err(error) => {
                        // A malformed frame fails that frame only; the connection carries on.
                        let defect = ServerMessage::<Value>::Defect {
                            defect: Value::String(error.to_string()),
                        };
                        if send_frames(socket, &defect, frame_parts).await.is_err() {
                            break End::Gone;
                        }
                    }
                }
            }
            (index, item) = poll_fn(|cx| {
                for (index, job) in jobs.iter_mut().enumerate() {
                    if !server.can_poll(&job.id) {
                        continue;
                    }
                    match std::panic::catch_unwind(AssertUnwindSafe(|| job.work.poll_next_unpin(cx))) {
                        Ok(Poll::Ready(item)) => return Poll::Ready((index, item)),
                        Ok(Poll::Pending) => {}
                        Err(_) => {
                            return Poll::Ready((index, Some(RpcResponse::Exit(panic_defect("RPC handler")))));
                        }
                    }
                }
                Poll::Pending
            }) => {
                let id = jobs[index].id.clone();
                // An Inactive error means the call was interrupted meanwhile; nothing to send.
                match item {
                    Some(RpcResponse::Chunk(values)) => {
                        if let Err(error) = server.send_chunk(&id, values) {
                            jobs.swap_remove(index);
                            sockets.release(&id);
                            let _ = server.fail_defect(&id, error.to_string());
                        }
                    }
                    Some(RpcResponse::Exit(exit)) => {
                        jobs.swap_remove(index);
                        sockets.release(&id);
                        let _ = server.finish(&id, exit);
                    }
                    None => {
                        jobs.swap_remove(index);
                        sockets.release(&id);
                        let _ = server.finish(&id, Exit::void());
                    }
                }
            }
        }
    };
    // Dropping the jobs releases every producer; disconnect cancels the protocol state.
    server.disconnect();
    drop(jobs);
    end
}
