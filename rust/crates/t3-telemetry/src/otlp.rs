use crate::{Error, Result, number};

pub(crate) struct ResolvedHttpClient {
    client: reqwest::blocking::Client,
    headers: http::HeaderMap,
    endpoint: http::Uri,
}
impl std::fmt::Debug for ResolvedHttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ResolvedOtlpHttpClient")
    }
}
impl ResolvedHttpClient {
    pub(crate) fn new(
        client: reqwest::blocking::Client,
        endpoint: &SignalEndpoint,
    ) -> Result<Self> {
        let mut headers = http::HeaderMap::new();
        for (key, value) in endpoint.export.headers.as_ref().into_iter().flatten() {
            headers.insert(
                http::HeaderName::from_bytes(key.as_bytes()).map_err(|_| Error::Export)?,
                value.parse().map_err(|_| Error::Export)?,
            );
        }
        Ok(Self {
            client,
            headers,
            endpoint: endpoint.url.parse().map_err(|_| Error::Export)?,
        })
    }
}
#[async_trait::async_trait]
impl opentelemetry_http::HttpClient for ResolvedHttpClient {
    async fn send_bytes(
        &self,
        mut request: opentelemetry_http::Request<opentelemetry_http::Bytes>,
    ) -> std::result::Result<
        opentelemetry_http::Response<opentelemetry_http::Bytes>,
        opentelemetry_http::HttpError,
    > {
        if request.uri() != &self.endpoint {
            return Err(Box::new(Error::Export));
        }
        // The SDK rereads OTEL headers; only t3-config's resolved signal may choose credentials.
        let content_type = request.headers().get(http::header::CONTENT_TYPE).cloned();
        *request.headers_mut() = self.headers.clone();
        if let Some(content_type) = content_type {
            request
                .headers_mut()
                .insert(http::header::CONTENT_TYPE, content_type);
        }
        opentelemetry_http::HttpClient::send_bytes(&self.client, request).await
    }
}
use std::time::Duration;
use t3_config::config::{SignalEndpoint, SignalExport, TelemetryConfig};
use t3_contracts::server::*;

pub fn endpoint(config: &TelemetryConfig, signal: OtlpSignal) -> Option<&SignalEndpoint> {
    if config.disabled {
        return None;
    }
    match signal {
        OtlpSignal::Traces => config.traces.as_ref(),
        OtlpSignal::Metrics => config.metrics.as_ref(),
        OtlpSignal::Logs => config.logs.as_ref(),
    }
}

pub fn export_options(config: &TelemetryConfig, signal: OtlpSignal) -> &SignalExport {
    endpoint(config, signal)
        .map(|e| &e.export)
        .unwrap_or(&config.default_export)
}

pub fn protocol(export: &SignalExport) -> Result<opentelemetry_otlp::Protocol> {
    match export.protocol.as_str() {
        "http/json" => Ok(opentelemetry_otlp::Protocol::HttpJson),
        "http/protobuf" => Ok(opentelemetry_otlp::Protocol::HttpBinary),
        _ => Err(Error::Configuration(
            "OTLP protocol must be http/json or http/protobuf".into(),
        )),
    }
}

pub struct OtlpEndpointCheck {
    client: reqwest::Client,
    config: TelemetryConfig,
}
impl OtlpEndpointCheck {
    pub fn new(config: TelemetryConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| Error::Export)?;
        Ok(Self { client, config })
    }
    pub async fn check(&self, input: &OtlpEndpointCheckInput) -> OtlpEndpointCheckResult {
        let running = endpoint(&self.config, input.signal);
        let options = export_options(&self.config, input.signal);
        let mut request = self.client.post(input.url.as_str());
        if let Some(running) = running.filter(|e| e.url == input.url.as_str()) {
            for (key, value) in running.export.headers.as_ref().into_iter().flatten() {
                request = request.header(key, value);
            }
        }
        if options.protocol == "http/protobuf" {
            request = request
                .header("content-type", "application/x-protobuf")
                .body(Vec::new());
        } else {
            let key = match input.signal {
                OtlpSignal::Traces => "resourceSpans",
                OtlpSignal::Metrics => "resourceMetrics",
                OtlpSignal::Logs => "resourceLogs",
            };
            request = request.json(&serde_json::json!({key:[]}));
        }
        let start = tokio::time::Instant::now();
        match request.send().await {
            Ok(response) if response.status().is_success() => {
                OtlpEndpointCheckResult::Accepted(OtlpEndpointAccepted {
                    tag: OtlpEndpointAcceptedTag::Accepted,
                    latency_ms: number(start.elapsed().as_secs_f64() * 1000.0),
                })
            }
            Ok(response) => OtlpEndpointCheckResult::Rejected(OtlpEndpointRejected {
                tag: OtlpEndpointRejectedTag::Rejected,
                status: number(f64::from(response.status().as_u16())),
            }),
            Err(error) => OtlpEndpointCheckResult::Unreachable(OtlpEndpointUnreachable {
                tag: OtlpEndpointUnreachableTag::Unreachable,
                timed_out: error.is_timeout(),
            }),
        }
    }
}
