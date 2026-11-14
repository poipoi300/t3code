use crate::{registry::HandlerRegistry, services::Domains};
use t3_contracts::rpc::*;

#[cfg(feature = "preview")]
mod live {
    use super::*;
    use crate::{
        adapters::preview::PreviewAdapter,
        registry::{Context, Failure, ServerDefect},
    };
    use std::sync::Arc;
    use t3_contracts::{auth::EnvironmentAuthorizationError, preview::*};
    use t3_preview::Error;

    fn internal<E>() -> Failure<E> {
        Failure::Defect(ServerDefect::Internal {
            message: "Preview operation failed.".into(),
        })
    }
    fn failure(error: Error, method: &str, thread: &str, tab: &str) -> Failure<WsPreviewOpenError> {
        let wire = match error {
            Error::TabNotFound => {
                PreviewError::SessionLookup(PreviewSessionLookupError::PreviewSessionLookupError {
                    thread_id: thread.into(),
                    tab_id: tab.into(),
                })
            }
            Error::ControlRequired => PreviewError::ControlRequired(
                PreviewControlRequiredError::PreviewControlRequiredError { tab_id: tab.into() },
            ),
            Error::InvalidUrl {
                reason,
                input_length,
                protocol,
            } => PreviewError::InvalidUrl(PreviewInvalidUrlError::PreviewInvalidUrlError {
                input_length: PreviewNumber::Finite(input_length.into()),
                reason: match reason.as_str() {
                    "empty" => PreviewInvalidUrlReason::Empty,
                    "parse" => PreviewInvalidUrlReason::Parse,
                    "unsupported-protocol" => PreviewInvalidUrlReason::UnsupportedProtocol,
                    _ => PreviewInvalidUrlReason::Unexpected,
                },
                protocol: protocol.map(Some),
                cause: serde_json::Value::Null,
            }),
            Error::InvalidInput(_) => {
                return Failure::Defect(ServerDefect::InvalidPayload {
                    method: method.into(),
                });
            }
            _ => return internal(),
        };
        Failure::Typed(WsPreviewOpenError::PreviewError(Box::new(wire)))
    }
    async fn thread(adapter: &PreviewAdapter, id: &str) -> Result<(), Failure<WsPreviewOpenError>> {
        adapter.require_thread(id).await.map_err(|error| {
            Failure::Typed(WsPreviewOpenError::EnvironmentAuthorizationError(Box::new(
                error,
            )))
        })
    }
    pub fn register(
        registry: &mut HandlerRegistry,
        adapter: Arc<PreviewAdapter>,
    ) -> crate::Result<()> {
        let a = adapter.clone();
        registry.register::<WsPreviewOpenRpc, _, _>(move |_, input| {
            let a = a.clone();
            async move {
                let id = input.thread_id.0.clone();
                thread(&a, &id).await?;
                a.browser
                    .manager()
                    .open(input)
                    .await
                    .map_err(|error| failure(error, "preview.open", &id, ""))
            }
        })?;
        macro_rules! tab {
            ($rpc:ty,$method:literal,$call:ident) => {{
                let a = adapter.clone();
                registry.register::<$rpc, _, _>(move |_, input| {
                    let a = a.clone();
                    async move {
                        let id = input.thread_id.0.clone();
                        let tab = input.tab_id.as_str().to_owned();
                        thread(&a, &id).await?;
                        a.require_tab(&id, &tab)
                            .await
                            .map_err(|error| failure(error, $method, &id, &tab))?;
                        a.browser
                            .manager()
                            .$call(input)
                            .await
                            .map_err(|error| failure(error, $method, &id, &tab))
                    }
                })?;
            }};
        }
        tab!(WsPreviewResizeRpc, "preview.resize", resize);
        tab!(WsPreviewAdjustRpc, "preview.adjust", adjust);
        tab!(
            WsPreviewReportStatusRpc,
            "preview.reportStatus",
            report_status
        );
        let a = adapter.clone();
        registry.register::<WsPreviewCloseRpc, _, _>(move |_, input| {
            let a = a.clone();
            async move {
                let id = input.thread_id.0.clone();
                thread(&a, &id).await?;
                if let Some(tab) = input.tab_id.as_ref().and_then(Option::as_ref)
                    && a.browser
                        .manager()
                        .tab_thread(tab.as_str())
                        .is_some_and(|owner| owner != id)
                {
                    return Err(failure(
                        Error::TabNotFound,
                        "preview.close",
                        &id,
                        tab.as_str(),
                    ));
                }
                a.browser
                    .manager()
                    .close(input)
                    .await
                    .map_err(|error| failure(error, "preview.close", &id, ""))
            }
        })?;
        let a = adapter.clone();
        registry.register::<WsPreviewListRpc, _, _>(move |_, input| {
            let a = a.clone();
            async move {
                a.require_thread(&input.thread_id.0)
                    .await
                    .map_err(Failure::Typed)?;
                Ok(a.browser.manager().list(input).await)
            }
        })?;
        let a = adapter.clone();
        registry.register::<WsPreviewClearProfileRpc, _, _>(move |_, input| {
            let a = a.clone();
            async move {
                a.browser
                    .clear_profile(&input.profile_id)
                    .await
                    .map_err(|_| {
                        Failure::Typed(WsPreviewClearProfileError::PreviewClearProfileError(
                            Box::new(PreviewClearProfileError::PreviewClearProfileError {
                                profile_id: input.profile_id,
                                cause: serde_json::Value::Null,
                            }),
                        ))
                    })
            }
        })?;
        let a = adapter.clone();
        registry.register::<WsPreviewReportProfilesRpc, _, _>(move |ctx, input| {
            let a = a.clone();
            async move {
                let socket = ctx.call.socket_id().ok_or_else(|| {
                    Failure::Typed(crate::adapters::preview::unavailable_thread())
                })?;
                a.reporter(socket).report(input).await.map_err(|_| {
                    Failure::Defect(ServerDefect::InvalidPayload {
                        method: "preview.reportProfiles".into(),
                    })
                })
            }
        })?;
        let a = adapter.clone();
        registry.register::<WsPreviewStopLocalServerRpc, _, _>(move |_, input| {
            let a = a.clone();
            async move {
                a.discovery.stop_server(input).await.map_err(|error| {
                    Failure::Typed(WsPreviewStopLocalServerError::PreviewStopLocalServerError(
                        Box::new(PreviewStopLocalServerError::PreviewStopLocalServerError {
                            reason: match error {
                                Error::Stop(reason) => reason,
                                _ => PreviewStopLocalServerReason::SignalFailed,
                            },
                            cause: None,
                        }),
                    ))
                })
            }
        })?;
        let a = adapter.clone();
        registry.register_stream::<WsSubscribePreviewEventsRpc,_,_,_>(move|ctx:Context,_|{
            let mut events=a.browser.manager().subscribe_events();async move {Ok(async_stream::stream! {
                loop {tokio::select! {_=ctx.services.shutdown.cancelled()=>break,event=events.recv()=>match event{Some(event)=>yield Ok(event),None=>break}}}
            })}
        })?;
        registry.register_stream::<WsSubscribeDiscoveredLocalServersRpc,_,_,_>(move|ctx,input|{let a=adapter.clone();async move {
            let mut subscription=a.discovery.subscribe(input.configured_urls.flatten().unwrap_or_default().into_iter().map(|url|url.0).collect()).await.map_err(|_|internal::<EnvironmentAuthorizationError>())?;
            Ok(async_stream::stream! {loop {tokio::select! {_=ctx.services.shutdown.cancelled()=>break,event=subscription.events.recv()=>match event{Some(event)=>yield Ok(event),None=>break}}}})
        }})?;
        Ok(())
    }
}

pub fn register_with(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    #[cfg(feature = "preview")]
    if let Some(adapter) = domains.get::<crate::adapters::preview::PreviewAdapter>() {
        return live::register(registry, adapter);
    }
    let _ = domains;
    registry.unimplemented::<WsPreviewOpenRpc>()?;
    registry.unimplemented::<WsPreviewResizeRpc>()?;
    registry.unimplemented::<WsPreviewAdjustRpc>()?;
    registry.unimplemented::<WsPreviewCloseRpc>()?;
    registry.unimplemented::<WsPreviewListRpc>()?;
    registry.unimplemented::<WsPreviewClearProfileRpc>()?;
    registry.unimplemented::<WsPreviewReportProfilesRpc>()?;
    registry.unimplemented::<WsPreviewReportStatusRpc>()?;
    registry.unimplemented::<WsPreviewStopLocalServerRpc>()?;
    registry.unimplemented::<WsSubscribePreviewEventsRpc>()?;
    registry.unimplemented::<WsSubscribeDiscoveredLocalServersRpc>()
}

#[cfg(feature = "preview")]
pub fn http_routes() -> axum::Router<std::sync::Arc<crate::services::ServiceRegistry>> {
    axum::Router::new().route(
        "/api/preview-stream/ws",
        axum::routing::get(viewer::upgrade),
    )
}

#[cfg(feature = "preview")]
mod viewer {
    use crate::services::ServiceRegistry;
    use axum::{
        extract::{
            ConnectInfo, OriginalUri, State,
            ws::{
                CloseFrame, Message, WebSocket, WebSocketUpgrade,
                rejection::WebSocketUpgradeRejection,
            },
        },
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Response},
    };
    use futures::{SinkExt, StreamExt};
    use std::{net::SocketAddr, sync::Arc, time::Duration};
    use t3_contracts::auth::{AuthEnvironmentScope as Scope, AuthWebSocketTicketUse};
    use t3_preview::{
        browser::{PreviewViewer, ViewerOptions},
        driver::TabKey,
        stream::{FrameFlow, ViewerOutput},
    };

    pub async fn upgrade(
        State(services): State<Arc<ServiceRegistry>>,
        ConnectInfo(remote): ConnectInfo<SocketAddr>,
        OriginalUri(uri): OriginalUri,
        headers: HeaderMap,
        upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    ) -> Response {
        if headers
            .get("upgrade")
            .and_then(|value| value.to_str().ok())
            .is_none_or(|value| !value.eq_ignore_ascii_case("websocket"))
        {
            return StatusCode::NOT_FOUND.into_response();
        }
        let request = crate::adapters::auth::request("GET", &uri, &headers, Some(remote));
        let session = match services
            .local
            .auth
            .inner()
            .authenticate_websocket(&request, AuthWebSocketTicketUse::Media)
            .await
        {
            Ok(session) => session,
            Err(error) => return crate::http::auth_error(error),
        };
        if let Err(error) =
            t3_auth::authorization::require_scope(&session.scopes, Scope::OrchestrationRead)
        {
            return crate::http::auth_error(error);
        }
        let Some(adapter) = services.domain::<crate::adapters::preview::PreviewAdapter>() else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let params = url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
            .into_owned()
            .collect::<Vec<_>>();
        let first = |key: &str| {
            params
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
        };
        let key = TabKey::new(
            first("threadId").unwrap_or_default(),
            first("tabId").unwrap_or_default(),
        );
        if key.thread_id.is_empty() || key.tab_id.is_empty() {
            return StatusCode::NOT_FOUND.into_response();
        }
        let allowed = adapter.require_thread(&key.thread_id).await.is_ok()
            && adapter
                .require_tab(&key.thread_id, &key.tab_id)
                .await
                .is_ok();
        let number = |name: &str, fallback: u32, max: u32| {
            first(name)
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|value| value.is_finite() && *value > 0.0)
                .map_or(fallback, |value| {
                    value.round().clamp(1.0, f64::from(max)) as u32
                })
        };
        let options = ViewerOptions {
            can_operate: session.scopes.contains(&Scope::PreviewOperate)
                && first("interactive") != Some("false"),
            max_width: number("maxWidth", 1280, 7680),
            max_height: number("maxHeight", 800, 4320),
            quality: number("quality", 70, 100) as u8,
        };
        let Ok(upgrade) = upgrade else {
            return (StatusCode::UPGRADE_REQUIRED, "WebSocket upgrade required").into_response();
        };
        let guard = services.connections.token();
        upgrade.max_message_size(100*1024*1024).max_frame_size(100*1024*1024).write_buffer_size(0).max_write_buffer_size(t3_preview::stream::MAX_SOCKET_BUFFER_BYTES).on_upgrade(move|mut socket|async move {
            let _guard=guard;
            let viewer=if allowed {tokio::select! {_=services.shutdown.cancelled()=>return,_=services.local.auth.invalidation(&session)=>return,result=adapter.browser.attach_viewer(key,options)=>result}}else{Err(t3_preview::Error::TabNotFound)};
            match viewer {
                Ok(viewer)=>{tokio::select! {_=services.shutdown.cancelled()=>{},_=services.local.auth.invalidation(&session)=>{},_=pump(&mut socket,&viewer)=>{}}}
                Err(error)=>{
                    let (code,reason)=if let Some(setup)=t3_preview::stream::host_setup(&error){(t3_contracts::preview::PREVIEW_STREAM_HOST_SETUP_CLOSE_CODE,serde_json::to_string(&setup).unwrap_or_default())}else if matches!(error,t3_preview::Error::TabNotFound){(t3_preview::stream::TAB_GONE_CODE,"tab closed".into())}else{(1011,"Preview unavailable".into())};
                    let _=tokio::time::timeout(Duration::from_secs(2),socket.send(Message::Close(Some(CloseFrame {code,reason:reason.into()})))).await;
                    return;
                }
            }
            let _=tokio::time::timeout(Duration::from_secs(2),socket.close()).await;
        })
    }
    async fn pump(socket: &mut WebSocket, viewer: &PreviewViewer) -> Result<(), ()> {
        let mut flow = FrameFlow::default();
        loop {
            tokio::select! {
                output=viewer.output.recv()=>match output {
                    Some(ViewerOutput::Frame{data,frame_id})=>{let (send,dropped)=flow.offer(data,frame_id);if let Some(id)=dropped{let _=viewer.acknowledge(id).await;}if let Some(data)=send{socket.send(Message::Binary(data.into())).await.map_err(|_|())?;}},
                    Some(ViewerOutput::Message(value))=>socket.send(Message::Text(value.to_string().into())).await.map_err(|_|())?,
                    Some(ViewerOutput::Gone)=>{socket.send(Message::Close(Some(CloseFrame{code:4404,reason:"tab closed".into()}))).await.map_err(|_|())?;return Ok(());},
                    Some(ViewerOutput::Reconnect)=>{socket.send(Message::Close(Some(CloseFrame{code:1012,reason:"preview reconnect".into()}))).await.map_err(|_|())?;return Ok(());},
                    None=>return Ok(()),
                },
                input=socket.next()=>match input {
                    Some(Ok(Message::Text(text)))=>{if let Ok(value)=serde_json::from_str::<serde_json::Value>(&text){receive(socket,viewer,&mut flow,value).await?;}},
                    Some(Ok(Message::Ping(data)))=>socket.send(Message::Pong(data)).await.map_err(|_|())?,
                    Some(Ok(Message::Pong(_)))=>{},
                    Some(Ok(Message::Binary(bytes)))=>{if let Ok(value)=serde_json::from_slice::<serde_json::Value>(&bytes){receive(socket,viewer,&mut flow,value).await?;}},
                    _=>return Ok(()),
                }
            }
        }
    }
    async fn receive(
        socket: &mut WebSocket,
        viewer: &PreviewViewer,
        flow: &mut FrameFlow,
        value: serde_json::Value,
    ) -> Result<(), ()> {
        if value["type"] == "ack" {
            let (send, acked) = flow.ack();
            if let Some(id) = acked {
                let _ = viewer.acknowledge(id).await;
            }
            if let Some(data) = send {
                socket
                    .send(Message::Binary(data.into()))
                    .await
                    .map_err(|_| ())?;
            }
        } else {
            let _ = viewer.input(value).await;
        }
        Ok(())
    }
}
