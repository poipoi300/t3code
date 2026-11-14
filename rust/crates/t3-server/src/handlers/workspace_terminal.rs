//! Workspace, signed-asset and terminal transports. Services own filesystem and PTY policy.
use crate::{registry::HandlerRegistry, services::Domains};
use t3_contracts::rpc::*;

pub fn register(registry: &mut HandlerRegistry) -> crate::Result<()> {
    register_with(registry, &Domains::default())
}

pub fn register_with(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    #[cfg(feature = "workspace")]
    match domains.get::<crate::adapters::workspace::WorkspaceAdapter>() {
        Some(adapter) => register_workspace(registry, adapter)?,
        None => workspace_unavailable(registry)?,
    }
    #[cfg(not(feature = "workspace"))]
    workspace_unavailable(registry)?;
    #[cfg(feature = "terminal")]
    match domains.get::<crate::adapters::terminal::TerminalAdapter>() {
        Some(adapter) => register_terminal(registry, adapter, domains.get::<TerminalTransport>())?,
        None => terminal_unavailable(registry)?,
    }
    #[cfg(not(feature = "terminal"))]
    terminal_unavailable(registry)?;
    let _ = domains;
    Ok(())
}

fn workspace_unavailable(registry: &mut HandlerRegistry) -> crate::Result<()> {
    registry.unimplemented::<WsProjectsSearchEntriesRpc>()?;
    registry.unimplemented::<WsProjectsSearchContentsRpc>()?;
    registry.unimplemented::<WsProjectsListEntriesRpc>()?;
    registry.unimplemented::<WsProjectsListIgnoredFilesRpc>()?;
    registry.unimplemented::<WsProjectsReadFileRpc>()?;
    registry.unimplemented::<WsProjectsWriteFileRpc>()?;
    registry.unimplemented::<WsFilesystemGetMetadataRpc>()?;
    registry.unimplemented::<WsFilesystemBrowseRpc>()?;
    registry.unimplemented::<WsAssetsCreateUrlRpc>()?;
    registry.unimplemented::<WsAssetsPersistChatAttachmentsRpc>()?;
    registry.unimplemented::<WsAttachmentsCreateUploadUrlRpc>()?;
    registry.unimplemented::<WsAttachmentsDeleteRpc>()?;
    Ok(())
}
fn terminal_unavailable(registry: &mut HandlerRegistry) -> crate::Result<()> {
    registry.unimplemented::<WsTerminalOpenRpc>()?;
    registry.unimplemented::<WsTerminalAttachRpc>()?;
    registry.unimplemented::<WsTerminalObserveRpc>()?;
    registry.unimplemented::<WsTerminalWriteRpc>()?;
    registry.unimplemented::<WsTerminalResizeRpc>()?;
    registry.unimplemented::<WsTerminalClearRpc>()?;
    registry.unimplemented::<WsTerminalCloseRpc>()?;
    registry.unimplemented::<WsSubscribeTerminalMetadataRpc>()?;
    Ok(())
}

#[cfg(any(feature = "workspace", feature = "terminal"))]
use crate::registry::{Failure, ServerDefect};
#[cfg(any(feature = "workspace", feature = "terminal"))]
use std::sync::Arc;

#[cfg(feature = "workspace")]
fn domain_error<E: serde::de::DeserializeOwned>(
    tag: &str,
    error: impl std::fmt::Display,
    input: &impl serde::Serialize,
) -> Failure<E> {
    let mut value = serde_json::to_value(input).unwrap_or(serde_json::json!({}));
    if let Some(value) = value.as_object_mut() {
        value.insert("_tag".into(), tag.into());
        value.insert("message".into(), error.to_string().into());
        value.insert("cause".into(), error.to_string().into());
    }
    match serde_json::from_value(value) {
        Ok(error) => Failure::Typed(error),
        Err(error) => Failure::Defect(ServerDefect::Internal {
            message: format!("workspace error encoding failed: {error}"),
        }),
    }
}
#[cfg(feature = "workspace")]
pub fn register_workspace(
    registry: &mut HandlerRegistry,
    adapter: Arc<crate::adapters::workspace::WorkspaceAdapter>,
) -> crate::Result<()> {
    macro_rules! unary {
        ($marker:ty, $method:ident, $error:literal) => {{
            let service = adapter.clone();
            registry.register::<$marker, _, _>(move |_, input| {
                let service = service.clone();
                async move {
                    service
                        .$method(input.clone())
                        .await
                        .map_err(|error| domain_error($error, error, &input))
                }
            })?;
        }};
    }
    unary!(
        WsProjectsListEntriesRpc,
        list_entries,
        "ProjectListEntriesError"
    );
    unary!(
        WsProjectsListIgnoredFilesRpc,
        list_ignored_files,
        "ProjectListEntriesError"
    );
    unary!(WsProjectsReadFileRpc, read_file, "ProjectReadFileError");
    unary!(
        WsProjectsSearchContentsRpc,
        search_contents,
        "ProjectSearchContentsError"
    );
    unary!(
        WsProjectsSearchEntriesRpc,
        search_entries,
        "ProjectSearchEntriesError"
    );
    unary!(WsFilesystemBrowseRpc, browse, "FilesystemBrowseError");
    let service = adapter.clone();
    registry.register::<WsProjectsWriteFileRpc, _, _>(move |ctx, input| {
        let service = service.clone();
        async move {
            let _admitted =
                crate::writers::admit(&ctx.services, "a file write", &input.cwd.0).await;
            let context =
                serde_json::json!({"cwd": input.cwd, "relativePath": input.relative_path});
            service
                .write_file(input)
                .await
                .map_err(|error| domain_error("ProjectWriteFileError", error, &context))
        }
    })?;
    let service = adapter.clone();
    registry.register::<WsAssetsPersistChatAttachmentsRpc, _, _>(move |_, input| {
        let service = service.clone();
        async move {
            service.persist_attachments(input).await.map_err(|error| {
                domain_error("PersistChatAttachmentsError", error, &serde_json::json!({}))
            })
        }
    })?;
    let service = adapter.clone();
    registry.register::<WsFilesystemGetMetadataRpc, _, _>(move |_, input| {
        let service = service.clone();
        async move {
            service.metadata(input).await.map_err(|error| {
                Failure::Defect(ServerDefect::Internal {
                    message: error.to_string(),
                })
            })
        }
    })?;
    let service = adapter.clone();
    registry.register::<WsAssetsCreateUrlRpc, _, _>(move |_, input| {
        let service = service.clone();
        async move {
            service
                .create_url(input, now_ms())
                .await
                .map_err(|error| Failure::Typed(WsAssetsCreateUrlError::AssetAccessError(error)))
        }
    })?;
    let service = adapter.clone();
    registry.register::<WsAttachmentsCreateUploadUrlRpc, _, _>(move |_, input| {
        let service = service.clone();
        async move {
            service
                .create_upload_url(input, now_ms())
                .await
                .map_err(|error| {
                    Failure::Typed(
                        WsAttachmentsCreateUploadUrlError::AttachmentUploadSigningKeyError(
                            Box::new(t3_contracts::assets::AttachmentUploadSigningKeyError {
                                tag:
                                    t3_contracts::assets::AttachmentUploadSigningKeyErrorTag::Error,
                                cause: error.to_string().into(),
                            }),
                        ),
                    )
                })
        }
    })?;
    registry.register::<WsAttachmentsDeleteRpc, _, _>(move |_, input| {
        let service = adapter.clone();
        async move {
            service.delete_attachment(input).await.map_err(|error| {
                Failure::Defect(ServerDefect::Internal {
                    message: error.to_string(),
                })
            })
        }
    })?;
    Ok(())
}

#[cfg(feature = "workspace")]
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().try_into().unwrap_or(u64::MAX)
        })
}

#[cfg(feature = "terminal")]
pub struct TerminalRequest {
    /// Unique to the socket, reused by attach and resize. Never a session or client-supplied ID.
    pub viewer_id: String,
    /// Unique to this RPC; the socket must deliver real ACKs to it.
    pub output_window: Arc<crate::adapters::terminal::TerminalOutputWindow>,
}
#[cfg(feature = "terminal")]
pub type TerminalRequestResolver =
    dyn Fn(&crate::registry::Context) -> Option<TerminalRequest> + Send + Sync;
#[cfg(feature = "terminal")]
pub struct TerminalTransport {
    pub resolve: Arc<TerminalRequestResolver>,
}
#[cfg(feature = "terminal")]
fn terminal_request(
    transport: &TerminalTransport,
    context: &crate::registry::Context,
    method: &str,
) -> std::result::Result<TerminalRequest, Failure<WsTerminalOpenError>> {
    (transport.resolve)(context)
        .filter(|request| !request.viewer_id.is_empty())
        .ok_or_else(|| {
            Failure::Defect(ServerDefect::NotImplemented {
                method: method.into(),
            })
        })
}
#[cfg(feature = "terminal")]
pub fn register_terminal(
    registry: &mut HandlerRegistry,
    adapter: Arc<crate::adapters::terminal::TerminalAdapter>,
    transport: Option<Arc<TerminalTransport>>,
) -> crate::Result<()> {
    macro_rules! unary {
        ($marker:ty, $method:ident) => {{
            let service = adapter.clone();
            registry.register::<$marker, _, _>(move |_, input| {
                let service = service.clone();
                async move { service.$method(input).await }
            })?;
        }};
    }
    unary!(WsTerminalOpenRpc, open);
    unary!(WsTerminalWriteRpc, write);
    unary!(WsTerminalClearRpc, clear);
    unary!(WsTerminalCloseRpc, close);
    let service = adapter.clone();
    registry.register_stream::<WsSubscribeTerminalMetadataRpc, _, _, _>(move |_, _| {
        let service = service.clone();
        async move { Ok(service.metadata()) }
    })?;
    let Some(transport) = transport else {
        registry.unimplemented::<WsTerminalAttachRpc>()?;
        registry.unimplemented::<WsTerminalObserveRpc>()?;
        registry.unimplemented::<WsTerminalResizeRpc>()?;
        return Ok(());
    };
    let service = adapter.clone();
    let socket = transport.clone();
    registry.register_stream::<WsTerminalAttachRpc, _, _, _>(move |context, input| {
        let service = service.clone();
        let socket = socket.clone();
        async move {
            let request = terminal_request(&socket, &context, WsTerminalAttachRpc::TAG)?;
            let stream = service.attach(input, request.viewer_id).await?;
            Ok(crate::adapters::terminal::output_stream(
                stream,
                request.output_window,
            ))
        }
    })?;
    let service = adapter.clone();
    let socket = transport.clone();
    registry.register_stream::<WsTerminalObserveRpc, _, _, _>(move |context, input| {
        let service = service.clone();
        let socket = socket.clone();
        async move {
            let request = terminal_request(&socket, &context, WsTerminalObserveRpc::TAG)?;
            let stream = service.observe(input).await?;
            Ok(crate::adapters::terminal::output_stream(
                stream,
                request.output_window,
            ))
        }
    })?;
    registry.register::<WsTerminalResizeRpc, _, _>(move |context, input| {
        let service = adapter.clone();
        let socket = transport.clone();
        async move {
            let request = terminal_request(&socket, &context, WsTerminalResizeRpc::TAG)?;
            service.resize(input, request.viewer_id).await
        }
    })?;
    Ok(())
}

#[cfg(feature = "workspace")]
mod http_transport {
    use super::*;
    use crate::{
        adapters::workspace::{ResolvedAsset, WorkspaceAdapter},
        services::ServiceRegistry,
    };
    use axum::{
        Router,
        body::Body,
        extract::{OriginalUri, Request, State},
        http::{HeaderMap, HeaderValue, Method, StatusCode},
        response::{IntoResponse, Response},
        routing::{get, post},
    };
    use futures::TryStreamExt;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    use tokio_util::io::{ReaderStream, StreamReader};

    /// Merge before the static fallback; keep the application's outer CORS/compression layers.
    pub fn routes() -> Router<Arc<ServiceRegistry>> {
        Router::new()
            .route("/api/assets/{*suffix}", get(asset))
            .route("/api/attachments/upload/{*token}", post(upload))
    }
    async fn asset(
        State(services): State<Arc<ServiceRegistry>>,
        OriginalUri(uri): OriginalUri,
        method: Method,
        headers: HeaderMap,
    ) -> Response {
        let Some(adapter) = services.domain::<WorkspaceAdapter>() else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Some((token, suffix)) = uri
            .path()
            .strip_prefix("/api/assets/")
            .and_then(|path| path.split_once('/'))
            .filter(|(token, _)| !token.is_empty())
        else {
            return StatusCode::NOT_FOUND.into_response();
        };
        match adapter.resolve_asset(token, suffix, now_ms()).await {
            Ok(Some(asset)) => asset_response(asset, &method, &headers)
                .await
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
            Ok(None) | Err(_) => StatusCode::NOT_FOUND.into_response(),
        }
    }
    async fn upload(
        State(services): State<Arc<ServiceRegistry>>,
        OriginalUri(uri): OriginalUri,
        request: Request,
    ) -> Response {
        let Some(adapter) = services.domain::<WorkspaceAdapter>() else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Some(token) = uri
            .path()
            .strip_prefix("/api/attachments/upload/")
            .filter(|token| !token.is_empty())
        else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let now = now_ms();
        let Some(claims) = adapter.validate_upload(token, now) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        if request.headers().get_all("content-length").iter().count() > 1
            || request
                .headers()
                .get("content-length")
                .is_some_and(|value| {
                    value
                        .to_str()
                        .ok()
                        .and_then(|value| value.trim().parse::<u64>().ok())
                        != Some(claims.size_bytes)
                })
        {
            return (
                StatusCode::BAD_REQUEST,
                "Content-Length must match the upload size.",
            )
                .into_response();
        }
        let body = request
            .into_body()
            .into_data_stream()
            .map_err(std::io::Error::other);
        match adapter.upload(token, now, StreamReader::new(body)).await {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(crate::adapters::workspace::Error::Upload { status, detail }) => (
                StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                detail,
            )
                .into_response(),
            Err(crate::adapters::workspace::Error::AssetDenied) => {
                StatusCode::NOT_FOUND.into_response()
            }
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
    pub async fn asset_response(
        asset: ResolvedAsset,
        method: &Method,
        request_headers: &HeaderMap,
    ) -> std::io::Result<Response> {
        let mut headers = HeaderMap::new();
        headers.insert(
            "cache-control",
            HeaderValue::from_static("private, max-age=3600"),
        );
        headers.insert(
            "x-content-type-options",
            HeaderValue::from_static("nosniff"),
        );
        match asset {
            ResolvedAsset::Bytes { bytes, mime_type } => {
                insert(&mut headers, "content-type", &mime_type)?;
                insert(&mut headers, "content-length", &bytes.len().to_string())?;
                Ok(response(
                    StatusCode::OK,
                    headers,
                    if method == Method::HEAD {
                        Body::empty()
                    } else {
                        Body::from(bytes)
                    },
                ))
            }
            ResolvedAsset::SourceControlMedia { .. } => {
                headers.insert(
                    "cache-control",
                    HeaderValue::from_static("private, no-store"),
                );
                Ok(response(StatusCode::BAD_GATEWAY, headers, Body::empty()))
            }
            ResolvedAsset::File {
                path,
                download,
                file_name,
                mime_type,
                file,
                render_intent,
            } => {
                let path_text = path.to_string_lossy().to_lowercase();
                let inline = mime_type
                    .as_deref()
                    .map(|mime| mime.split(';').next().unwrap_or("").trim());
                let mime = if download {
                    mime_type
                        .as_deref()
                        .filter(|mime| safe_download_mime(mime))
                        .unwrap_or("application/octet-stream")
                        .to_owned()
                } else if let Some(mime) = inline.filter(|mime| safe_media_mime(mime)) {
                    mime.into()
                } else if inline.is_some_and(|mime| mime.eq_ignore_ascii_case("text/html"))
                    || path_text.ends_with(".html")
                    || path_text.ends_with(".htm")
                {
                    "text/html; charset=utf-8".into()
                } else if inline.is_some_and(|mime| mime.eq_ignore_ascii_case("application/pdf")) {
                    "application/pdf".into()
                } else {
                    mime_guess::from_path(&path)
                        .first_or_octet_stream()
                        .to_string()
                };
                if download {
                    insert(
                        &mut headers,
                        "content-disposition",
                        &download_disposition(file_name.as_deref()),
                    )?;
                    headers.insert(
                        "content-security-policy",
                        HeaderValue::from_static("default-src 'none'; sandbox"),
                    );
                } else if mime == "text/html; charset=utf-8" {
                    headers.insert(
                        "content-security-policy",
                        HeaderValue::from_static(
                            "sandbox allow-scripts allow-forms allow-popups allow-downloads",
                        ),
                    );
                }
                if !download && path_text.ends_with(".svg") {
                    headers.insert(
                        "content-security-policy",
                        HeaderValue::from_static(
                            "default-src 'none'; style-src 'unsafe-inline'; sandbox",
                        ),
                    );
                }
                insert(&mut headers, "content-type", &mime)?;
                // The MCP HTML bootstrap belongs to the application service, which is not ported.
                if !download
                    && render_intent.as_deref() == Some("mcp-app")
                    && mime == "text/html; charset=utf-8"
                {
                    return Ok(response(
                        StatusCode::NOT_IMPLEMENTED,
                        headers,
                        Body::empty(),
                    ));
                }
                let is_media = safe_media_mime(&mime);
                if is_media {
                    headers.insert(
                        "cache-control",
                        HeaderValue::from_static("private, no-store"),
                    );
                }
                let mut file = tokio::fs::File::from_std(file.file);
                let metadata = file.metadata().await?;
                let size = metadata.len();
                if !is_media && let Ok(modified) = metadata.modified() {
                    insert(
                        &mut headers,
                        "last-modified",
                        &httpdate::fmt_http_date(modified),
                    )?;
                    let millis = modified
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis();
                    insert(&mut headers, "etag", &format!("W/\"{size:x}-{millis:x}\""))?;
                }
                headers.insert("accept-ranges", HeaderValue::from_static("bytes"));
                let mut status = StatusCode::OK;
                let (mut offset, mut length) = (0, size);
                if method == Method::GET
                    && !request_headers.contains_key("if-range")
                    && let Some(range) = request_headers
                        .get("range")
                        .and_then(|value| value.to_str().ok())
                        .and_then(|value| byte_range(value, size))
                {
                    match range {
                        ByteRange::Unsatisfiable => {
                            insert(&mut headers, "content-range", &format!("bytes */{size}"))?;
                            return Ok(response(
                                StatusCode::RANGE_NOT_SATISFIABLE,
                                headers,
                                Body::empty(),
                            ));
                        }
                        ByteRange::Range { start, end } => {
                            status = StatusCode::PARTIAL_CONTENT;
                            offset = start;
                            length = end - start + 1;
                            insert(
                                &mut headers,
                                "content-range",
                                &format!("bytes {start}-{end}/{size}"),
                            )?;
                        }
                    }
                }
                insert(&mut headers, "content-length", &length.to_string())?;
                if method == Method::HEAD || length == 0 {
                    return Ok(response(status, headers, Body::empty()));
                }
                if offset
                    .checked_add(length)
                    .is_none_or(|end| end > 9_007_199_254_740_991)
                {
                    return Ok((
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "File is too large to preview.",
                    )
                        .into_response());
                }
                file.seek(std::io::SeekFrom::Start(offset)).await?;
                Ok(response(
                    status,
                    headers,
                    Body::from_stream(ReaderStream::new(file.take(length))),
                ))
            }
        }
    }
    fn response(status: StatusCode, headers: HeaderMap, body: Body) -> Response {
        let mut response = Response::new(body);
        *response.status_mut() = status;
        *response.headers_mut() = headers;
        response
    }
    fn insert(headers: &mut HeaderMap, name: &'static str, value: &str) -> std::io::Result<()> {
        headers.insert(
            name,
            HeaderValue::from_str(value).map_err(std::io::Error::other)?,
        );
        Ok(())
    }
    fn mime_token(value: &str) -> bool {
        let Some((kind, subtype)) = value.split_once('/') else {
            return false;
        };
        !kind.is_empty()
            && !subtype.is_empty()
            && [kind, subtype].iter().all(|value| {
                value
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || b"_!#$&^.+-".contains(&ch))
            })
    }
    fn safe_download_mime(value: &str) -> bool {
        let lower = value.to_ascii_lowercase();
        mime_token(value)
            && lower != "text/html"
            && !lower.ends_with("+xml")
            && !lower.ends_with("/xml")
            && !lower.contains("/xml-")
    }
    fn safe_media_mime(value: &str) -> bool {
        let lower = value.to_ascii_lowercase();
        mime_token(value) && (lower.starts_with("audio/") || lower.starts_with("video/"))
    }
    pub fn download_disposition(name: Option<&str>) -> String {
        let Some(name) = name else {
            return "attachment".into();
        };
        let sanitized: String = name
            .chars()
            .map(|ch| {
                if ch.is_control() || matches!(ch, '"' | '\\') {
                    '_'
                } else {
                    ch
                }
            })
            .collect();
        let ascii: String = sanitized
            .chars()
            .map(|ch| if ch.is_ascii() { ch } else { '_' })
            .collect();
        let mut value = format!("attachment; filename=\"{ascii}\"");
        if ascii != sanitized {
            let encoded: String = sanitized
                .bytes()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() || b"-_.!~".contains(&ch) {
                        char::from(ch).to_string()
                    } else {
                        format!("%{ch:02X}")
                    }
                })
                .collect();
            value.push_str(&format!("; filename*=UTF-8''{encoded}"));
        }
        value
    }
    enum ByteRange {
        Unsatisfiable,
        Range { start: u64, end: u64 },
    }
    fn byte_range(header: &str, size: u64) -> Option<ByteRange> {
        let (unit, range) = header.trim().split_once('=')?;
        if !unit.eq_ignore_ascii_case("bytes") {
            return None;
        }
        let (first, last) = range.split_once('-')?;
        if first.is_empty() && last.is_empty()
            || !first
                .bytes()
                .chain(last.bytes())
                .all(|ch| ch.is_ascii_digit())
        {
            return None;
        }
        if !first.is_empty() && !last.is_empty() {
            let (first, last) = (first.trim_start_matches('0'), last.trim_start_matches('0'));
            if (last.len(), last) < (first.len(), first) {
                return None;
            }
        }
        let number = |value: &str| value.parse::<u128>().unwrap_or(u128::MAX);
        let first = (!first.is_empty()).then(|| number(first));
        let last = (!last.is_empty()).then(|| number(last));
        if first.zip(last).is_some_and(|(first, last)| last < first) {
            return None;
        }
        let size = u128::from(size);
        if size == 0
            || first.is_some_and(|first| first >= size)
            || first.is_none() && last == Some(0)
        {
            return Some(ByteRange::Unsatisfiable);
        }
        let start = first.unwrap_or_else(|| size.saturating_sub(last.unwrap_or(0)));
        let end = if first.is_none() {
            size - 1
        } else {
            last.unwrap_or(size - 1).min(size - 1)
        };
        if start > 9_007_199_254_740_991 || end > 9_007_199_254_740_991 {
            return Some(ByteRange::Unsatisfiable);
        }
        Some(ByteRange::Range {
            start: start as u64,
            end: end as u64,
        })
    }
}
#[cfg(feature = "workspace")]
pub use http_transport::{asset_response, download_disposition, routes as http_routes};
