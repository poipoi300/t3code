use crate::{Error, service::RateFetcher, sources::ProbeFuture};
use serde_json::Value;
use std::{sync::Arc, time::Duration};

pub const RATE_FETCH_TIMEOUT: Duration = Duration::from_secs(10);
pub const MAX_RATE_BODY_BYTES: usize = 8 * 1024 * 1024;

pub trait RateHttpBody: Send {
    fn next_chunk(&mut self) -> ProbeFuture<'_, Option<Vec<u8>>>;
}
pub struct RateHttpResponse {
    pub status: u16,
    pub content_length: Option<u64>,
    pub body: Box<dyn RateHttpBody>,
}
pub trait RateHttpClient: Send + Sync {
    fn get<'a>(
        &'a self,
        url: &'a str,
        accept: &'a str,
        timeout: Duration,
    ) -> ProbeFuture<'a, RateHttpResponse>;
}
pub struct BoundedRateFetcher {
    http: Arc<dyn RateHttpClient>,
}
impl BoundedRateFetcher {
    pub fn new(http: Arc<dyn RateHttpClient>) -> Self {
        Self { http }
    }
}
impl RateFetcher for BoundedRateFetcher {
    fn fetch<'a>(&'a self, url: &'a str, accept: &'a str) -> ProbeFuture<'a, Value> {
        Box::pin(async move {
            tokio::time::timeout(RATE_FETCH_TIMEOUT, async {
                let mut response = self
                    .http
                    .get(url, accept, RATE_FETCH_TIMEOUT)
                    .await
                    .map_err(|_| Error::Probe("Rate HTTP request failed".into()))?;
                if !(200..300).contains(&response.status) {
                    return Err(Error::Probe("Rate HTTP status was not successful".into()));
                }
                if response
                    .content_length
                    .is_some_and(|length| length > MAX_RATE_BODY_BYTES as u64)
                {
                    return Err(Error::Probe("Rate HTTP body exceeds its size limit".into()));
                }
                let mut bytes = Vec::new();
                while let Some(chunk) = response
                    .body
                    .next_chunk()
                    .await
                    .map_err(|_| Error::Probe("Rate HTTP body read failed".into()))?
                {
                    if chunk.len() > MAX_RATE_BODY_BYTES - bytes.len() {
                        return Err(Error::Probe("Rate HTTP body exceeds its size limit".into()));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                tokio::task::spawn_blocking(move || {
                    if !crate::safety::json_shape_ok(&bytes, 2_000_000) {
                        return Err(Error::Probe(
                            "Rate HTTP JSON exceeds its shape limit".into(),
                        ));
                    }
                    serde_json::from_slice(&bytes)
                        .map_err(|_| Error::Probe("Invalid rate HTTP JSON".into()))
                })
                .await
                .map_err(|_| Error::Probe("Rate HTTP JSON worker failed".into()))?
            })
            .await
            .map_err(|_| Error::Probe("Rate HTTP request timed out".into()))?
        })
    }
}
