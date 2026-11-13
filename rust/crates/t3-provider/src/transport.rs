use crate::sensitive::{Sensitive, ServiceError, ServiceResult};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone)]
pub struct HttpRequest {
    pub method: &'static str,
    pub url: String,
    pub headers: BTreeMap<String, Sensitive<String>>,
    pub body: Option<Sensitive<String>>,
}
impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HttpRequest([REDACTED])")
    }
}
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Sensitive<Value>,
}
#[derive(Debug, Clone)]
pub struct HttpTextResponse {
    pub status: u16,
    pub body: Sensitive<String>,
}
#[async_trait]
pub trait HttpTransport: Send + Sync {
    async fn send(&self, request: HttpRequest) -> ServiceResult<HttpResponse>;
    async fn text(&self, request: HttpRequest) -> ServiceResult<HttpTextResponse> {
        let response = self.send(request).await?;
        let body = response
            .body
            .expose()
            .as_str()
            .ok_or(ServiceError::Response)?
            .to_owned();
        Ok(HttpTextResponse {
            status: response.status,
            body: Sensitive::new(body),
        })
    }
}
pub struct ReqwestTransport {
    client: reqwest::Client,
}
impl ReqwestTransport {
    pub fn new() -> ServiceResult<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| ServiceError::Network)?,
        })
    }
    async fn bytes(&self, request: HttpRequest) -> ServiceResult<(u16, Vec<u8>)> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|_| ServiceError::Response)?;
        let mut builder = self.client.request(method, &request.url);
        for (key, value) in request.headers {
            builder = builder.header(key, value.into_inner());
        }
        if let Some(body) = request.body {
            builder = builder.body(body.into_inner());
        }
        let mut response = builder.send().await.map_err(|_| ServiceError::Network)?;
        let status = response.status().as_u16();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| ServiceError::Network)? {
            if bytes.len().saturating_add(chunk.len()) > 8 * 1024 * 1024 {
                return Err(ServiceError::Response);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((status, bytes))
    }
}
#[async_trait]
impl HttpTransport for ReqwestTransport {
    async fn text(&self, request: HttpRequest) -> ServiceResult<HttpTextResponse> {
        let (status, bytes) = self.bytes(request).await?;
        Ok(HttpTextResponse {
            status,
            body: Sensitive::new(String::from_utf8_lossy(&bytes).into_owned()),
        })
    }
    async fn send(&self, request: HttpRequest) -> ServiceResult<HttpResponse> {
        let (status, bytes) = self.bytes(request).await?;
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).map_err(|_| ServiceError::Response)?
        };
        Ok(HttpResponse {
            status,
            body: Sensitive::new(body),
        })
    }
}
pub trait ProviderClock: Send + Sync {
    fn now_ms(&self) -> i64;
}
pub struct SystemClock;
impl ProviderClock for SystemClock {
    fn now_ms(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
            .unwrap_or(0)
    }
}
