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
#[derive(Debug, Clone)]
pub struct RawHttpResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Sensitive<Vec<u8>>,
}
#[async_trait]
pub trait HttpTransport: Send + Sync {
    async fn send(&self, request: HttpRequest) -> ServiceResult<HttpResponse>;
    async fn raw(&self, request: HttpRequest) -> ServiceResult<RawHttpResponse> {
        let response = self.send(request).await?;
        Ok(RawHttpResponse {
            status: response.status,
            content_type: Some("application/json".into()),
            body: Sensitive::new(serde_json::to_vec(response.body.expose()).map_err(
                crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Response,
                ),
            )?),
        })
    }
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
    /// Node fetch's behaviour, as TypeScript's provider and usage HTTP has it: ambient
    /// `HTTP(S)_PROXY`/`ALL_PROXY` are ignored, and the bundled roots are trusted plus every
    /// certificate in the PEM file `NODE_EXTRA_CA_CERTS` names (an unreadable file adds none).
    pub fn new() -> ServiceResult<Self> {
        let extra = std::env::var_os("NODE_EXTRA_CA_CERTS")
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|pem| reqwest::Certificate::from_pem_bundle(&pem).ok())
            .unwrap_or_default();
        let builder = extra
            .into_iter()
            .fold(reqwest::Client::builder().no_proxy(), |builder, root| {
                builder.add_root_certificate(root)
            });
        Ok(Self {
            client: builder
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Network,
                ))?,
        })
    }
    async fn bytes(&self, request: HttpRequest) -> ServiceResult<RawHttpResponse> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes()).map_err(
            crate::diagnostics::mapped(concat!(module_path!(), "::error"), ServiceError::Response),
        )?;
        let mut builder = self.client.request(method, &request.url);
        for (key, value) in request.headers {
            builder = builder.header(key, value.into_inner());
        }
        if let Some(body) = request.body {
            builder = builder.body(body.into_inner());
        }
        let mut response = builder.send().await.map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Network,
        ))?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Network,
        ))? {
            if bytes.len().saturating_add(chunk.len()) > 8 * 1024 * 1024 {
                return Err(ServiceError::Response);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(RawHttpResponse {
            status,
            content_type,
            body: Sensitive::new(bytes),
        })
    }
}
#[async_trait]
impl HttpTransport for ReqwestTransport {
    async fn raw(&self, request: HttpRequest) -> ServiceResult<RawHttpResponse> {
        self.bytes(request).await
    }
    async fn text(&self, request: HttpRequest) -> ServiceResult<HttpTextResponse> {
        let response = self.bytes(request).await?;
        Ok(HttpTextResponse {
            status: response.status,
            body: Sensitive::new(String::from_utf8_lossy(response.body.expose()).into_owned()),
        })
    }
    async fn send(&self, request: HttpRequest) -> ServiceResult<HttpResponse> {
        let response = self.bytes(request).await?;
        let status = response.status;
        let bytes = response.body.into_inner();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ))?
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
