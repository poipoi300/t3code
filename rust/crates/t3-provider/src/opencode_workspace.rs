use crate::{
    environment::Environment,
    opencode_http::{OpenCodeError, OpenCodeHttp},
    sensitive::{ServiceError, ServiceResult},
    transport::HttpRequest,
};
use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;
use tokio::sync::mpsc;

pub struct ScanEvent {
    pub directory: String,
    pub kind: ScanKind,
}
#[derive(Clone, Copy)]
pub enum ScanKind {
    Commands,
    Skills,
}
pub struct ScanSubscription {
    pub events: mpsc::Receiver<ScanEvent>,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl ScanSubscription {
    pub fn new(events: mpsc::Receiver<ScanEvent>) -> Self {
        Self { events, task: None }
    }
}
impl Drop for ScanSubscription {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
#[async_trait]
pub trait WorkspaceEvents: Send + Sync {
    async fn subscribe(&self, request: HttpRequest) -> ServiceResult<ScanSubscription>;
}
pub struct HttpWorkspaceEvents {
    client: reqwest::Client,
}
impl HttpWorkspaceEvents {
    pub fn new() -> ServiceResult<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Network,
                ))?,
        })
    }
}
#[async_trait]
impl WorkspaceEvents for HttpWorkspaceEvents {
    async fn subscribe(&self, request: HttpRequest) -> ServiceResult<ScanSubscription> {
        let mut builder = self.client.get(request.url);
        for (key, value) in request.headers {
            builder = builder.header(key, value.into_inner());
        }
        let mut response = builder.send().await.map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Network,
        ))?;
        if !response.status().is_success()
            || !response
                .headers()
                .get("content-type")
                .and_then(|header| header.to_str().ok())
                .is_some_and(|header| header.split(';').next() == Some("text/event-stream"))
        {
            return Err(ServiceError::Response);
        }
        let (send, events) = mpsc::channel(32);
        let task = tokio::spawn(async move {
            let mut pending = Vec::new();
            let mut data = Vec::new();
            let mut skip_lf = false;
            while let Ok(Some(chunk)) = response.chunk().await {
                for byte in chunk {
                    if skip_lf && byte == b'\n' {
                        skip_lf = false;
                        continue;
                    }
                    skip_lf = false;
                    if byte == b'\r' || byte == b'\n' {
                        skip_lf = byte == b'\r';
                        if pending.is_empty() {
                            if let Ok(value) = serde_json::from_slice::<Value>(&data) {
                                let kind = match value["type"].as_str() {
                                    Some("command.updated") => Some(ScanKind::Commands),
                                    Some("skill.updated") => Some(ScanKind::Skills),
                                    _ => None,
                                };
                                if let Some(kind) = kind
                                    && let Some(directory) = value["location"]["directory"].as_str()
                                    && send
                                        .send(ScanEvent {
                                            directory: directory.into(),
                                            kind,
                                        })
                                        .await
                                        .is_err()
                                {
                                    return;
                                }
                            }
                            data.clear();
                        } else if let Some(line) = pending.strip_prefix(b"data:") {
                            if !data.is_empty() {
                                data.push(b'\n');
                            }
                            data.extend_from_slice(line.strip_prefix(b" ").unwrap_or(line));
                        }
                        pending.clear();
                    } else {
                        pending.push(byte);
                    }
                    if pending.len().saturating_add(data.len()) > 1024 * 1024 {
                        return;
                    }
                }
            }
        });
        Ok(ScanSubscription {
            events,
            task: Some(task),
        })
    }
}
pub async fn load(
    connection: &OpenCodeHttp,
    events: &dyn WorkspaceEvents,
    directory: &str,
    environment: &Environment,
) -> Result<(Vec<Value>, Vec<Value>), OpenCodeError> {
    let timeout = Duration::from_millis(crate::versions::probe_timeout_ms(
        "opencode",
        10_000,
        environment,
    ));
    let work = async {
        let mut stream =
            tokio::time::timeout(timeout, events.subscribe(connection.event_request()))
                .await
                .ok()
                .and_then(Result::ok);
        let loaded = if stream.is_none() {
            true
        } else {
            connection
                .get("/api/debug/location", None, 2, Duration::from_secs(2))
                .await
                .ok()
                .and_then(|value| {
                    value
                        .as_array()
                        .map(|entries| entries.iter().any(|entry| entry["directory"] == directory))
                })
                .unwrap_or(true)
        };
        let _ = connection
            .get("/api/provider", Some(directory), 2, Duration::from_secs(10))
            .await;
        let first = connection.workspace_v2(directory, timeout).await?;
        if loaded && !first.1.is_empty() {
            return Ok(first);
        }
        let scanned = async {
            let mut commands = false;
            let mut skills = false;
            if let Some(stream) = stream.as_mut() {
                while let Some(event) = stream.events.recv().await {
                    if event.directory != directory {
                        continue;
                    }
                    match event.kind {
                        ScanKind::Commands => commands = true,
                        ScanKind::Skills => skills = true,
                    }
                    if commands && skills {
                        return;
                    }
                }
            }
            std::future::pending::<()>().await;
        };
        let _ = tokio::time::timeout(timeout, scanned).await;
        connection.workspace_v2(directory, timeout).await
    };
    tokio::time::timeout(timeout.saturating_mul(2), work)
        .await
        .map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            OpenCodeError::Timeout,
        ))?
}
