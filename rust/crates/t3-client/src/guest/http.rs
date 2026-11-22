//! The guest HTTP shape every host shares: request building, SSE decoding and rejection mapping.
//!
//! A host supplies only the bytes ([`GuestHttp`]): the browser's same-origin `fetch` (whose cookie
//! jar holds the per-share cookie) or a native client with its own jar. Nothing here sees a cookie,
//! and the invite credential travels only in the JSON body of `redeem`, never in a URL.

use futures::future::LocalBoxFuture;
use serde_json::Value;
use t3_contracts::orchestration_v2::OrchestrationV2ThreadHistoryPage;

use super::transport::{GuestError, GuestFailure, GuestFuture, GuestStreamHandle, GuestTransport};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestMethod {
    Get,
    Post,
}

/// A request relative to the page's origin. The body of `redeem` holds the invite, so `Debug`
/// never prints a body.
#[derive(Clone, PartialEq, Eq)]
pub struct GuestRequest {
    pub method: GuestMethod,
    /// Path and query, starting with `/api/guest/`.
    pub path: String,
    /// A JSON body; hosts send it with `content-type: application/json`.
    pub json: Option<String>,
}

impl std::fmt::Debug for GuestRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuestRequest")
            .field("method", &self.method)
            .field("path", &self.path)
            .field("json", &self.json.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub retry_after: Option<String>,
}

/// The body of an open response, one network read at a time. Dropping it releases the connection.
pub trait GuestChunks {
    /// `None` when the server closed the body cleanly; `Err` when the connection broke.
    fn next(&mut self) -> GuestFuture<'_, Option<Vec<u8>>>;
}

pub struct GuestStreamResponse {
    pub status: u16,
    /// The response's `content-type`, if it sent one.
    pub content_type: Option<String>,
    pub retry_after: Option<String>,
    pub chunks: Box<dyn GuestChunks>,
}

/// What a host provides. A network failure is `GuestError::Network`; HTTP statuses are answers.
pub trait GuestHttp {
    /// A request whose whole body is read (`redeem`, `session`, `messages`, `history`, `item`).
    fn send(&self, request: GuestRequest) -> GuestFuture<'_, GuestResponse>;
    /// A request whose body is read incrementally (`stream`, `command-output`).
    fn open(&self, request: GuestRequest) -> GuestFuture<'_, GuestStreamResponse>;
    fn sleep(&self, millis: u64) -> LocalBoxFuture<'_, ()>;
    fn random(&self) -> f64 {
        0.5
    }
    fn now_epoch_ms(&self) -> Option<u64> {
        None
    }
}

impl<T: GuestHttp + ?Sized> GuestHttp for std::rc::Rc<T> {
    fn send(&self, request: GuestRequest) -> GuestFuture<'_, GuestResponse> {
        (**self).send(request)
    }
    fn open(&self, request: GuestRequest) -> GuestFuture<'_, GuestStreamResponse> {
        (**self).open(request)
    }
    fn sleep(&self, millis: u64) -> LocalBoxFuture<'_, ()> {
        (**self).sleep(millis)
    }
    fn random(&self) -> f64 {
        (**self).random()
    }
    fn now_epoch_ms(&self) -> Option<u64> {
        (**self).now_epoch_ms()
    }
}

pub mod requests {
    use serde_json::json;
    use t3_contracts::guest_share::{
        GUEST_COMMAND_OUTPUT_ROUTE, GUEST_HISTORY_ROUTE, GUEST_ITEM_ROUTE, GUEST_ROUTE_PREFIX,
        GUEST_SHARE_PARAM,
    };

    use super::{GuestMethod, GuestRequest};

    fn encode(value: &str) -> String {
        url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
    }

    fn get(route: &str, share: &str, extra: &[(&str, &str)]) -> GuestRequest {
        let mut path = format!("{route}?{GUEST_SHARE_PARAM}={}", encode(share));
        for (key, value) in extra {
            path.push_str(&format!("&{key}={}", encode(value)));
        }
        GuestRequest {
            method: GuestMethod::Get,
            path,
            json: None,
        }
    }

    /// The invite goes in the body only.
    pub fn redeem(credential: &str, name: &str) -> GuestRequest {
        GuestRequest {
            method: GuestMethod::Post,
            path: format!("{GUEST_ROUTE_PREFIX}/redeem"),
            json: Some(json!({"credential": credential, "name": name}).to_string()),
        }
    }

    pub fn session(share: &str) -> GuestRequest {
        get(&format!("{GUEST_ROUTE_PREFIX}/session"), share, &[])
    }

    pub fn stream(share: &str) -> GuestRequest {
        get(&format!("{GUEST_ROUTE_PREFIX}/stream"), share, &[])
    }

    pub fn submit(share: &str, text: &str) -> GuestRequest {
        GuestRequest {
            method: GuestMethod::Post,
            path: get(&format!("{GUEST_ROUTE_PREFIX}/messages"), share, &[]).path,
            json: Some(json!({"shareId": share, "text": text}).to_string()),
        }
    }

    pub fn history(share: &str, cursor: &str) -> GuestRequest {
        get(GUEST_HISTORY_ROUTE, share, &[("cursor", cursor)])
    }

    pub fn item(share: &str, item: &str) -> GuestRequest {
        get(GUEST_ITEM_ROUTE, share, &[("item", item)])
    }

    pub fn command_output(share: &str, item: &str) -> GuestRequest {
        get(GUEST_COMMAND_OUTPUT_ROUTE, share, &[("item", item)])
    }
}

/// Parses `Retry-After` as either integer delay-seconds or an IMF-fixdate HTTP-date.
pub fn parse_retry_after_ms(header: &str, now_epoch_ms: Option<u64>) -> Option<u64> {
    let trimmed = header.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
        return trimmed
            .parse::<u64>()
            .ok()
            .map(|seconds| seconds.saturating_mul(1_000));
    }
    let now_epoch_ms = now_epoch_ms?;
    let target_ms = parse_http_date_epoch_ms(trimmed)?;
    let target_u64 = u64::try_from(target_ms).unwrap_or(0);
    Some(target_u64.saturating_sub(now_epoch_ms))
}

fn parse_http_date_epoch_ms(value: &str) -> Option<i64> {
    let rest = value
        .split_once(',')
        .map_or(value, |(_, after)| after)
        .trim();
    let mut parts = rest.split_ascii_whitespace();
    let day: u8 = parts.next()?.parse().ok()?;
    let month = match parts.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year: u16 = parts.next()?.parse().ok()?;
    let time = parts.next()?;
    if parts.next()? != "GMT" || parts.next().is_some() {
        return None;
    }
    let iso = format!("{year:04}-{month:02}-{day:02}T{time}Z");
    crate::time::epoch_ms(&iso)
}

/// Maps a non-2xx answer: 4xx is terminal with the server's `{reason, message}` (the TS fallback
/// copy when the body is not that); 5xx and anything else means try again, carrying `Retry-After`
/// when present.
pub fn rejection_with_retry_after(
    status: u16,
    body: &[u8],
    retry_after_ms: Option<u64>,
) -> GuestError {
    // 408, 425 and 429 are a proxy or server saying "later", not "never".
    if !(400..500).contains(&status) || matches!(status, 408 | 425 | 429) {
        let message = format!("HTTP {status}");
        return match retry_after_ms {
            Some(millis) => GuestError::RetryAfter { millis, message },
            None => GuestError::Network(message),
        };
    }
    let parsed: Option<Value> = serde_json::from_slice(body).ok();
    let field = |name: &str| {
        parsed
            .as_ref()
            .and_then(|body| body.get(name))
            .and_then(Value::as_str)
    };
    GuestError::Rejected {
        status,
        reason: field("reason").map_or(GuestFailure::Unknown, GuestFailure::from_reason),
        message: field("message")
            .unwrap_or("Something went wrong. Try again.")
            .to_owned(),
    }
}

pub fn rejection(status: u16, body: &[u8]) -> GuestError {
    rejection_with_retry_after(status, body, None)
}

/// The most one unterminated line or one event's joined `data` may hold. Guest events are bounded
/// snapshots and deltas; past this the stream is treated as broken and reconnects.
const SSE_LIMIT: usize = 1024 * 1024;

/// An incremental `text/event-stream` decoder that yields each event's JSON `data`. Comments (the
/// server's keep-alive), other fields and frames that are not JSON are skipped, as the TS page
/// skips events it cannot parse.
#[derive(Default)]
pub struct SseDecoder {
    pending: Vec<u8>,
    /// How much of `pending` is known to hold no line break, so a long line is scanned once.
    scanned: usize,
    data: Option<String>,
    started: bool,
}

/// A line or an event that never ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SseOverflow;

impl SseDecoder {
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Value>, SseOverflow> {
        self.pending.extend_from_slice(chunk);
        if !self.started {
            if self.pending.len() < 3 && b"\xef\xbb\xbf".starts_with(&self.pending) {
                return Ok(Vec::new());
            }
            self.started = true;
            if self.pending.starts_with(b"\xef\xbb\xbf") {
                self.pending.drain(..3);
            }
        }
        let mut events = Vec::new();
        let mut consumed = 0;
        let mut from = self.scanned.min(self.pending.len());
        loop {
            let rest = &self.pending[consumed..];
            let Some(offset) = rest[from..].iter().position(|b| matches!(b, b'\n' | b'\r')) else {
                // Nothing here ends a line; the next push starts after these bytes.
                from = rest.len();
                break;
            };
            let end = from + offset;
            from = 0;
            let mut skip = end + 1;
            if rest[end] == b'\r' {
                match rest.get(end + 1) {
                    // "\r" at the end of a chunk may be half of "\r\n".
                    None => {
                        from = end;
                        break;
                    }
                    Some(b'\n') => skip += 1,
                    Some(_) => {}
                }
            }
            let line = String::from_utf8_lossy(&rest[..end]).into_owned();
            consumed += skip;
            if let Some(event) = self.line(&line)? {
                events.push(event);
            }
        }
        self.pending.drain(..consumed);
        self.scanned = from;
        if self.pending.len() > SSE_LIMIT {
            return Err(SseOverflow);
        }
        Ok(events)
    }

    fn line(&mut self, line: &str) -> Result<Option<Value>, SseOverflow> {
        if line.is_empty() {
            return Ok(self
                .data
                .take()
                .and_then(|data| serde_json::from_str(&data).ok()));
        }
        if line.starts_with(':') {
            return Ok(None);
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        if field == "data" {
            let value = value.strip_prefix(' ').unwrap_or(value);
            match &mut self.data {
                Some(data) => {
                    if data.len() + value.len() + 1 > SSE_LIMIT {
                        return Err(SseOverflow);
                    }
                    data.push('\n');
                    data.push_str(value);
                }
                None => {
                    if value.len() > SSE_LIMIT {
                        return Err(SseOverflow);
                    }
                    self.data = Some(value.to_owned());
                }
            }
        }
        Ok(None)
    }
}

struct SseStream {
    chunks: Box<dyn GuestChunks>,
    decoder: SseDecoder,
    ready: std::collections::VecDeque<Value>,
}

impl GuestStreamHandle for SseStream {
    fn next(&mut self) -> GuestFuture<'_, Option<Value>> {
        Box::pin(async move {
            loop {
                if let Some(frame) = self.ready.pop_front() {
                    return Ok(Some(frame));
                }
                match self.chunks.next().await? {
                    Some(chunk) => self.ready.extend(self.decoder.push(&chunk).map_err(|_| {
                        GuestError::Network(
                            "the event stream sent an event that is too large".into(),
                        )
                    })?),
                    None => return Ok(None),
                }
            }
        })
    }
}

/// How much of a rejected response is read for its reason.
const REJECTION_BODY_LIMIT: usize = 64 * 1024;

/// The guest transport over any host's bytes.
pub struct HttpGuestTransport<H> {
    host: H,
}

impl<H: GuestHttp> HttpGuestTransport<H> {
    pub fn new(host: H) -> Self {
        Self { host }
    }

    async fn expect_ok(&self, request: GuestRequest) -> Result<(), GuestError> {
        let response = self.host.send(request).await?;
        if (200..300).contains(&response.status) {
            Ok(())
        } else {
            let retry_after_ms = response
                .retry_after
                .as_deref()
                .and_then(|header| parse_retry_after_ms(header, self.host.now_epoch_ms()));
            Err(rejection_with_retry_after(
                response.status,
                &response.body,
                retry_after_ms,
            ))
        }
    }

    fn open_sse(&self, request: GuestRequest) -> GuestFuture<'_, Box<dyn GuestStreamHandle>> {
        Box::pin(async move {
            let mut response = self.host.open(request).await?;
            if response.status != 200 {
                let mut body = Vec::new();
                while body.len() < REJECTION_BODY_LIMIT {
                    match response.chunks.next().await {
                        Ok(Some(chunk)) => body.extend(chunk),
                        _ => break,
                    }
                }
                let retry_after_ms = response
                    .retry_after
                    .as_deref()
                    .and_then(|header| parse_retry_after_ms(header, self.host.now_epoch_ms()));
                return Err(rejection_with_retry_after(
                    response.status,
                    &body,
                    retry_after_ms,
                ));
            }
            // A 200 that is not an event stream (an SPA fallback, a captive portal) never becomes one.
            let is_stream = response.content_type.as_deref().is_some_and(|kind| {
                kind.trim()
                    .to_ascii_lowercase()
                    .starts_with("text/event-stream")
            });
            if !is_stream {
                return Err(GuestError::Rejected {
                    status: 200,
                    reason: GuestFailure::Invalid,
                    message: "This guest link is not valid.".into(),
                });
            }
            Ok(Box::new(SseStream {
                chunks: response.chunks,
                decoder: SseDecoder::default(),
                ready: Default::default(),
            }) as Box<dyn GuestStreamHandle>)
        })
    }
}

impl<H: GuestHttp> GuestTransport for HttpGuestTransport<H> {
    fn redeem(&self, credential: &str, name: &str) -> GuestFuture<'_, ()> {
        let request = requests::redeem(credential, name);
        Box::pin(async move { self.expect_ok(request).await })
    }

    fn session(&self, share: &str) -> GuestFuture<'_, ()> {
        let request = requests::session(share);
        Box::pin(async move { self.expect_ok(request).await })
    }

    fn open_stream(&self, share: &str) -> GuestFuture<'_, Box<dyn GuestStreamHandle>> {
        self.open_sse(requests::stream(share))
    }

    fn history(
        &self,
        share: &str,
        cursor: &str,
    ) -> GuestFuture<'_, OrchestrationV2ThreadHistoryPage> {
        let request = requests::history(share, cursor);
        Box::pin(async move {
            let response = self.host.send(request).await?;
            if !(200..300).contains(&response.status) {
                let parsed: Option<Value> = serde_json::from_slice(&response.body).ok();
                let reason = parsed
                    .as_ref()
                    .and_then(|body| body.get("reason"))
                    .and_then(Value::as_str)
                    .map_or(GuestFailure::Unknown, GuestFailure::from_reason);
                let message = parsed
                    .as_ref()
                    .and_then(|body| body.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("Could not load earlier turns. Try again.")
                    .to_owned();
                return Err(GuestError::Rejected {
                    status: response.status,
                    reason,
                    message,
                });
            }
            let raw: Value =
                serde_json::from_slice(&response.body).map_err(|_| GuestError::Rejected {
                    status: response.status,
                    reason: GuestFailure::Unknown,
                    message: "Could not load earlier turns. Try again.".into(),
                })?;
            crate::state::decoding::history_page(raw).map_err(|_| GuestError::Rejected {
                status: response.status,
                reason: GuestFailure::Unknown,
                message: "Could not load earlier turns. Try again.".into(),
            })
        })
    }

    fn command_output(
        &self,
        share: &str,
        item: &str,
    ) -> GuestFuture<'_, Box<dyn GuestStreamHandle>> {
        self.open_sse(requests::command_output(share, item))
    }

    fn submit(&self, share: &str, text: &str) -> GuestFuture<'_, ()> {
        let request = requests::submit(share, text);
        Box::pin(async move { self.expect_ok(request).await })
    }

    fn sleep(&self, millis: u64) -> LocalBoxFuture<'_, ()> {
        self.host.sleep(millis)
    }

    fn random(&self) -> f64 {
        self.host.random()
    }
}
