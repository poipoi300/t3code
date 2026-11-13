use crate::{Error, Rejection, Result, Secret, SharedClock};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::BTreeMap,
    fmt,
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};
use url::Url;

pub struct Request {
    pub method: reqwest::Method,
    pub url: Url,
    pub headers: BTreeMap<String, Secret>,
    pub body: Vec<u8>,
    pub timeout: Duration,
}
impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Request([REDACTED])")
    }
}
pub struct Response {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}
impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}
#[async_trait]
pub trait Transport: Send + Sync {
    /// Must not follow redirects, log requests, or include bodies/URLs in errors.
    async fn send(&self, request: Request) -> Result<Response>;
}
pub type Http = Arc<dyn Transport>;
pub struct ReqwestTransport(reqwest::Client);
impl ReqwestTransport {
    pub fn new() -> Result<Self> {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map(Self)
            .map_err(|_| unavailable())
    }
}
#[async_trait]
impl Transport for ReqwestTransport {
    async fn send(&self, request: Request) -> Result<Response> {
        let mut builder = self
            .0
            .request(request.method, request.url)
            .timeout(request.timeout);
        for (k, v) in request.headers {
            builder = builder.header(k, v.expose());
        }
        let response = builder
            .body(request.body)
            .send()
            .await
            .map_err(|_| unavailable())?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_owned())))
            .collect();
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| unavailable())?;
            if body.len().saturating_add(chunk.len()) > 4 * 1024 * 1024 {
                return Err(Error::Protocol);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Response {
            status,
            headers,
            body,
        })
    }
}
pub fn unavailable() -> Error {
    Error::Remote {
        rejection: Rejection::Unavailable,
        retry_after_ms: None,
    }
}
pub fn secure_origin(value: &str) -> Result<Url> {
    let url = Url::parse(value).map_err(|_| Error::Configuration)?;
    if value != value.trim()
        || url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || value.contains(['?', '#'])
    {
        return Err(Error::Configuration);
    }
    Ok(url)
}
pub fn retry_after(value: Option<&String>, now_ms: i64) -> Option<u64> {
    let value = value?.trim();
    let delay = if value.bytes().all(|b| b.is_ascii_digit()) && !value.is_empty() {
        value.parse::<u64>().ok()?.saturating_mul(1000)
    } else {
        let date = httpdate::parse_http_date(value)
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_millis();
        let date = i64::try_from(date).ok()?;
        u64::try_from(date.saturating_sub(now_ms)).ok()?
    };
    (delay > 0).then_some(delay.min(900_000))
}
pub fn check(response: &Response, now: i64) -> Result<()> {
    if (200..300).contains(&response.status) {
        return Ok(());
    }
    let expired = serde_json::from_slice::<
        t3_contracts::relay::RelayEnvironmentLinkProofExpiredError,
    >(&response.body)
    .is_ok();
    let rejection = match response.status {
        401 if expired => Rejection::Unavailable,
        401 => Rejection::Unauthorized,
        403 => Rejection::Forbidden,
        400..=499 if !matches!(response.status, 408 | 429) => Rejection::Rejected,
        300..=399 => Rejection::Rejected,
        _ => Rejection::Unavailable,
    };
    Err(Error::Remote {
        rejection,
        retry_after_ms: (rejection == Rejection::Unavailable)
            .then(|| retry_after(response.headers.get("retry-after"), now))
            .flatten(),
    })
}
#[derive(Clone)]
pub struct Client {
    pub(crate) http: Http,
    pub(crate) base: Url,
    pub(crate) clock: SharedClock,
}
impl Client {
    pub fn relay(http: Http, base: &str, clock: SharedClock) -> Result<Self> {
        Ok(Self {
            http,
            base: secure_origin(base)?,
            clock,
        })
    }
    pub(crate) fn at(http: Http, base: Url, clock: SharedClock) -> Self {
        Self { http, base, clock }
    }
    pub async fn call<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &[&str],
        bearer: Option<&Secret>,
        body: Option<&impl Serialize>,
        timeout: Duration,
    ) -> Result<T> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| Error::Configuration)?
            .clear()
            .extend(path);
        let mut headers = BTreeMap::new();
        headers.insert("content-type".to_owned(), Secret::new("application/json"));
        if let Some(token) = bearer {
            headers.insert(
                "authorization".to_owned(),
                Secret::new(format!("Bearer {}", token.expose())),
            );
        }
        let body = body
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|_| Error::Protocol)?
            .unwrap_or_default();
        let response = self
            .http
            .send(Request {
                method,
                url,
                headers,
                body,
                timeout,
            })
            .await?;
        check(&response, self.clock.now_ms())?;
        serde_json::from_slice(&response.body).map_err(|_| Error::Protocol)
    }
}

/// Contracts derive Debug on credentials; service boundaries wrap those values.
pub struct Sensitive<T>(T);
impl<T> Sensitive<T> {
    pub fn new(value: T) -> Self {
        Self(value)
    }
    pub fn expose(&self) -> &T {
        &self.0
    }
    pub fn expose_mut(&mut self) -> &mut T {
        &mut self.0
    }
    pub fn into_inner(self) -> T {
        self.0
    }
}
impl<T> fmt::Debug for Sensitive<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Sensitive([REDACTED])")
    }
}
