use crate::{
    FailureReason, ForgeError, Result,
    rate_limit::{RateLimitKey, RateLimits, retry_at_from_header},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::StreamExt;
use reqwest::{
    Client, Method,
    header::{AUTHORIZATION, HeaderValue},
    redirect::Policy,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Semaphore;
use url::Url;

#[derive(Clone)]
pub enum ApiCredential {
    Anonymous,
    Bearer(String),
    Basic { username: String, password: String },
}
impl ApiCredential {
    fn header(&self) -> Option<String> {
        match self {
            Self::Anonymous => None,
            Self::Bearer(token) => Some(format!("Bearer {token}")),
            Self::Basic { username, password } => Some(format!(
                "Basic {}",
                STANDARD.encode(format!("{username}:{password}"))
            )),
        }
    }
}
#[derive(Clone)]
pub struct ApiHttp {
    provider: &'static str,
    base: Url,
    auth: Arc<RwLock<(Option<String>, u64)>>,
    client: Client,
    slots: Arc<Semaphore>,
    limits: Arc<RateLimits>,
    cancellation: t3_process::CancellationToken,
}
pub struct ApiResponse {
    pub bytes: Vec<u8>,
    pub truncated: bool,
}
impl ApiResponse {
    pub fn json(&self, provider: &str, operation: &str) -> Result<Value> {
        if self.truncated {
            return Err(failed(
                provider,
                operation,
                "The response exceeded the byte limit.",
            ));
        }
        if self.bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&self.bytes).map_err(|_| {
            failed(
                provider,
                operation,
                "The source control host returned invalid JSON.",
            )
        })
    }
}
fn failed(provider: &str, operation: &str, detail: &str) -> ForgeError {
    ForgeError::provider(provider, operation, FailureReason::Failed, detail)
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
impl ApiHttp {
    pub fn new(
        provider: &'static str,
        base: Url,
        credential: ApiCredential,
        limits: Arc<RateLimits>,
    ) -> Result<Self> {
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(failed(
                provider,
                "configure",
                "Invalid source control API origin.",
            ));
        }
        let header = credential.header();
        if header
            .as_ref()
            .is_some_and(|v| HeaderValue::from_str(v).is_err())
        {
            return Err(failed(
                provider,
                "configure",
                "Invalid source control credential.",
            ));
        }
        Ok(Self {
            provider,
            base,
            auth: Arc::new(RwLock::new((header, 0))),
            client: Client::builder()
                .redirect(Policy::none())
                .build()
                .map_err(|_| {
                    failed(
                        provider,
                        "configure",
                        "Could not create the source control HTTP client.",
                    )
                })?,
            slots: Arc::new(Semaphore::new(4)),
            limits,
            cancellation: t3_process::CancellationToken::new(),
        })
    }
    pub fn base_url(&self) -> &Url {
        &self.base
    }
    pub fn credential_configured(&self) -> bool {
        self.auth
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .0
            .is_some()
    }
    pub fn settings_revision(&self) -> u64 {
        self.auth.read().unwrap_or_else(|e| e.into_inner()).1
    }
    pub fn update_credential(&self, credential: ApiCredential) -> Result<()> {
        let header = credential.header();
        if header
            .as_ref()
            .is_some_and(|v| HeaderValue::from_str(v).is_err())
        {
            return Err(failed(
                self.provider,
                "configure",
                "Invalid source control credential.",
            ));
        }
        let mut auth = self.auth.write().unwrap_or_else(|e| e.into_inner());
        auth.0 = header;
        auth.1 = auth.1.wrapping_add(1);
        Ok(())
    }
    pub fn shutdown(&self) {
        self.cancellation.cancel();
    }
    pub async fn request(
        &self,
        operation: &str,
        method: Method,
        path: &str,
        body: Option<Value>,
        limit: usize,
    ) -> Result<ApiResponse> {
        tokio::select! {biased; _=self.cancellation.cancelled()=>Err(failed(self.provider,operation,"The source control request was cancelled.")),result=async {
            let _permit=self.slots.acquire().await.map_err(|_|failed(self.provider,operation,"The source control client is closed."))?;
            let header=self.auth.read().unwrap_or_else(|e|e.into_inner()).0.clone();
            let key=RateLimitKey {provider:self.provider.into(),host:self.base.authority().into(),credential_scope:format!("{:x}",Sha256::digest(header.as_deref().unwrap_or("").as_bytes()))};
            let lease=self.limits.check(&key,now(),method!=Method::GET||crate::github::transport::is_interactive())?;
            let result=tokio::time::timeout(Duration::from_secs(30),self.send(operation,method,path,body,limit,header.as_deref())).await.map_err(|_|failed(self.provider,operation,"The source control request timed out."))?;
            match &result {Ok(_)=>self.limits.record_success(&key,lease,now()),Err(ForgeError::Provider {reason:FailureReason::RateLimited,retry_at,..})=>self.limits.record_limit(&key,lease,*retry_at,now()),_=>{}}
            result
        }=>result}
    }
    fn url(&self, path: &str) -> Result<Url> {
        let parsed = Url::parse(path);
        let url = if let Ok(url) = parsed {
            url
        } else {
            let mut prefix = self.base.as_str().trim_end_matches('/').to_string();
            prefix.push('/');
            prefix.push_str(path.trim_start_matches('/'));
            Url::parse(&prefix)
                .map_err(|_| failed(self.provider, "request", "Invalid API path."))?
        };
        if url.origin() != self.base.origin()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(failed(
                self.provider,
                "request",
                "The source control API refused a foreign origin.",
            ));
        }
        Ok(url)
    }
    async fn send(
        &self,
        operation: &str,
        method: Method,
        path: &str,
        body: Option<Value>,
        limit: usize,
        auth: Option<&str>,
    ) -> Result<ApiResponse> {
        let mut url = self.url(path)?;
        for hop in 0..=3 {
            let mut request = self.client.request(method.clone(), url.clone());
            if let Some(auth) = auth {
                request = request.header(AUTHORIZATION, auth);
            }
            if let Some(body) = &body {
                request = request.json(body);
            }
            let response = request.send().await.map_err(|_| {
                failed(
                    self.provider,
                    operation,
                    "Could not send the source control request.",
                )
            })?;
            let status = response.status().as_u16();
            if (300..400).contains(&status)
                && hop < 3
                && let Some(location) = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
            {
                let redirected = url.join(location).map_err(|_| {
                    failed(self.provider, operation, "Invalid source control redirect.")
                })?;
                url = self.url(redirected.as_str())?;
                continue;
            }
            if !(200..300).contains(&status) {
                let reason = match status {
                    401 => FailureReason::Unauthenticated,
                    403 => FailureReason::Forbidden,
                    404 | 410 => FailureReason::NotFound,
                    429 => FailureReason::RateLimited,
                    _ => FailureReason::Failed,
                };
                let mut error = ForgeError::provider(
                    self.provider,
                    operation,
                    reason,
                    format!("The source control host returned HTTP {status}."),
                );
                if let ForgeError::Provider {
                    status: target,
                    retry_at,
                    ..
                } = &mut error
                {
                    *target = Some(status);
                    *retry_at = response
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| retry_at_from_header(v, now()));
                }
                return Err(error);
            }
            let mut stream = response.bytes_stream();
            let mut bytes = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| {
                    failed(
                        self.provider,
                        operation,
                        "Could not read the source control response.",
                    )
                })?;
                let remaining = limit.saturating_sub(bytes.len());
                bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                if chunk.len() > remaining {
                    return Ok(ApiResponse {
                        bytes,
                        truncated: true,
                    });
                }
            }
            return Ok(ApiResponse {
                bytes,
                truncated: false,
            });
        }
        Err(failed(
            self.provider,
            operation,
            "Too many source control redirects.",
        ))
    }
    pub async fn json(
        &self,
        operation: &str,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        self.request(operation, method, path, body, 8 * 1024 * 1024)
            .await?
            .json(self.provider, operation)
    }
}
