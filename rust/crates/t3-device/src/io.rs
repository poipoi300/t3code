use crate::{Error, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use std::{collections::BTreeMap, fmt, pin::Pin, sync::Arc, time::Duration};
use t3_process::{ProcessOptions, SpawnedProcess};

pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>;

#[derive(Clone)]
pub struct Secret(String);
impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

pub struct HttpRequest {
    pub method: http::Method,
    pub url: url::Url,
    pub headers: BTreeMap<String, Secret>,
    pub body: Option<Bytes>,
    pub body_stream: Option<ByteStream>,
    pub timeout: Option<Duration>,
    pub max_bytes: Option<usize>,
}
impl HttpRequest {
    pub fn new(method: http::Method, url: &str, timeout: Duration) -> Result<Self> {
        Ok(Self {
            method,
            url: validated_url(url)?,
            headers: BTreeMap::new(),
            body: None,
            body_stream: None,
            timeout: Some(timeout),
            max_bytes: None,
        })
    }
    pub fn bearer(mut self, token: &Secret) -> Self {
        self.headers.insert(
            "authorization".into(),
            Secret::new(format!("Bearer {}", token.expose())),
        );
        self
    }
    pub fn json(mut self, body: &serde_json::Value) -> Result<Self> {
        self.headers
            .insert("content-type".into(), Secret::new("application/json"));
        self.body = Some(
            serde_json::to_vec(body)
                .map_err(|_| crate::error::invalid("encode request"))?
                .into(),
        );
        Ok(self)
    }
}

pub struct HttpResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: ByteStream,
}
impl HttpResponse {
    pub fn bytes(status: u16, body: impl Into<Bytes>) -> Self {
        Self {
            status,
            headers: BTreeMap::new(),
            body: Box::pin(futures_util::stream::once(std::future::ready(Ok(
                body.into()
            )))),
        }
    }
    pub async fn collect(mut self, max: Option<usize>) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        while let Some(chunk) = self.body.next().await {
            let chunk = chunk?;
            if max.is_some_and(|max| bytes.len().saturating_add(chunk.len()) > max) {
                return Err(crate::error::invalid("response size"));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}

#[async_trait]
pub trait Http: Send + Sync {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse>;
}

pub struct NativeHttp(reqwest::Client);
impl NativeHttp {
    pub fn new() -> Result<Self> {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map(Self)
            .map_err(|_| Error::Unavailable("Could not create device HTTP client.".into()))
    }
}
#[async_trait]
impl Http for NativeHttp {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse> {
        let mut builder = self.0.request(request.method, request.url);
        for (name, value) in request.headers {
            builder = builder.header(name, value.expose());
        }
        if let Some(timeout) = request.timeout {
            builder = builder.timeout(timeout);
        }
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
        if let Some(body) = request.body_stream {
            builder = builder.body(reqwest::Body::wrap_stream(body));
        }
        let response = builder.send().await.map_err(|error| {
            Error::Unavailable(
                if error.is_timeout() {
                    "Device HTTP request timed out."
                } else {
                    "Device HTTP request failed."
                }
                .into(),
            )
        })?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_string())))
            .collect();
        let max = request.max_bytes;
        let body = async_stream::try_stream! {
            let mut stream = response.bytes_stream();
            let mut received = 0usize;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|error| Error::Unavailable(if error.is_timeout() { "Device HTTP request timed out." } else { "Device HTTP body failed." }.into()))?;
                received = received.saturating_add(chunk.len());
                if max.is_some_and(|max| received > max) { Err(crate::error::invalid("response size"))?; }
                yield chunk;
            }
        };
        Ok(HttpResponse {
            status,
            headers,
            body: Box::pin(body),
        })
    }
}

pub fn validated_url(raw: &str) -> Result<url::Url> {
    let url = url::Url::parse(raw).map_err(|_| crate::error::invalid("URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || raw.chars().any(|c| c.is_control())
    {
        return Err(crate::error::invalid("URL"));
    }
    Ok(url)
}

pub async fn request_bytes(http: &dyn Http, request: HttpRequest) -> Result<(u16, Vec<u8>)> {
    let timeout = request.timeout;
    let max = request.max_bytes;
    let run = async {
        let response = http.execute(request).await?;
        let status = response.status;
        Ok((status, response.collect(max).await?))
    };
    match timeout {
        Some(limit) => tokio::time::timeout(limit, run)
            .await
            .map_err(|_| Error::Unavailable("Device HTTP request timed out.".into()))?,
        None => run.await,
    }
}

pub async fn json_request(
    http: &dyn Http,
    request: HttpRequest,
    operation: &str,
) -> Result<serde_json::Value> {
    let (status, bytes) = request_bytes(http, request).await?;
    if !(200..300).contains(&status) {
        return Err(Error::operation(
            operation,
            t3_contracts::device::DeviceOperationErrorReason::HubRejected,
        ));
    }
    serde_json::from_slice(&bytes).map_err(|_| crate::error::invalid(operation))
}

#[derive(Clone)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<std::path::PathBuf>,
    pub env: BTreeMap<String, Option<Secret>>,
    pub stdin: Option<Vec<u8>>,
    pub timeout: Duration,
    pub max_bytes: usize,
}
impl Command {
    pub fn new(
        program: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            cwd: None,
            env: BTreeMap::new(),
            stdin: None,
            timeout: Duration::from_secs(20),
            max_bytes: 8 * 1024 * 1024,
        }
    }
    pub fn options(&self) -> ProcessOptions {
        let mut options = ProcessOptions::new(&self.program).args(&self.args);
        options.cwd = self.cwd.clone();
        options.stdin = self.stdin.clone();
        options.timeout = Some(self.timeout);
        options.max_output_bytes = self.max_bytes;
        options.check_exit = false;
        options.timed_out_result = true;
        options.env = self
            .env
            .iter()
            .map(|(k, v)| (k.into(), v.as_ref().map(|v| v.expose().into())))
            .collect();
        options
    }
}

#[derive(Clone, Default)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

#[async_trait]
pub trait Commands: Send + Sync {
    async fn run(&self, command: Command) -> Result<CommandOutput>;
}

pub struct NativeCommands;
#[async_trait]
impl Commands for NativeCommands {
    async fn run(&self, command: Command) -> Result<CommandOutput> {
        let result = t3_process::run(command.options()).await;
        match result {
            Ok(out) => Ok(CommandOutput {
                stdout: out.stdout_text(),
                stderr: out.stderr_text(),
                code: if out.timed_out {
                    124
                } else {
                    out.code.unwrap_or(127)
                },
            }),
            Err(t3_process::ProcessError::NotFound { .. }) => Ok(CommandOutput {
                code: 127,
                ..Default::default()
            }),
            Err(_) => Err(Error::operation(
                "command",
                t3_contracts::device::DeviceOperationErrorReason::CommandFailed,
            )),
        }
    }
}

#[async_trait]
pub trait ManagedChild: Send + Sync {
    async fn exited(&mut self) -> Result<()>;
    async fn stop(&mut self) -> Result<()>;
}
#[async_trait]
pub trait Spawner: Send + Sync {
    async fn spawn(&self, command: Command) -> Result<Box<dyn ManagedChild>>;
}
pub struct NativeSpawner;
struct NativeChild {
    child: SpawnedProcess,
    readers: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for NativeChild {
    fn drop(&mut self) {
        for reader in &self.readers {
            reader.abort();
        }
    }
}
#[async_trait]
impl ManagedChild for NativeChild {
    async fn exited(&mut self) -> Result<()> {
        self.child
            .wait()
            .await
            .map(|_| ())
            .map_err(|_| Error::Unavailable("Device helper exited.".into()))
    }
    async fn stop(&mut self) -> Result<()> {
        self.child
            .kill()
            .await
            .map_err(|_| Error::Unavailable("Could not stop owned device helper.".into()))
    }
}
#[async_trait]
impl Spawner for NativeSpawner {
    async fn spawn(&self, command: Command) -> Result<Box<dyn ManagedChild>> {
        let mut child = t3_process::spawn(&command.options())
            .map_err(|_| Error::Unavailable("Could not spawn device helper.".into()))?;
        let mut readers = vec![];
        if let Some(mut out) = child.stdout.take() {
            readers.push(tokio::spawn(async move {
                let _ = tokio::io::copy(&mut out, &mut tokio::io::sink()).await;
            }));
        }
        if let Some(mut out) = child.stderr.take() {
            readers.push(tokio::spawn(async move {
                let _ = tokio::io::copy(&mut out, &mut tokio::io::sink()).await;
            }));
        }
        Ok(Box::new(NativeChild { child, readers }))
    }
}

pub type SharedHttp = Arc<dyn Http>;
pub type SharedCommands = Arc<dyn Commands>;
