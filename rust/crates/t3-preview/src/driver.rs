use crate::{Error, Result, lock, manager::Events};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use t3_contracts::preview::{PreviewReportStatusInput, PreviewSessionSnapshot};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::{Mutex as AsyncMutex, mpsc, oneshot},
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub struct TabKey {
    pub thread_id: String,
    pub tab_id: String,
}
impl TabKey {
    pub fn new(thread: impl Into<String>, tab: impl Into<String>) -> Self {
        Self {
            thread_id: thread.into(),
            tab_id: tab.into(),
        }
    }
    pub fn of(snapshot: &PreviewSessionSnapshot) -> Self {
        Self::new(snapshot.thread_id.as_str(), snapshot.tab_id.as_str())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "command",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BrowserCommand {
    Executable {
        path: PathBuf,
    },
    Open {
        snapshot: Box<PreviewSessionSnapshot>,
        desktop_endpoint: Option<String>,
        isolated: bool,
        popup_id: Option<String>,
        load_error: Option<t3_contracts::preview_automation::PreviewAutomationLoadError>,
        crashed_url: Option<String>,
    },
    DiscardPopup {
        popup_id: String,
    },
    Operate {
        key: TabKey,
        operation: String,
        input: Value,
        timeout_ms: u64,
        generation: u64,
    },
    Adjust {
        snapshot: Box<PreviewSessionSnapshot>,
        request: Value,
    },
    Close {
        key: TabKey,
        desktop: bool,
        unload: bool,
    },
    ClearProfile {
        profile_id: String,
    },
    ViewerAttach {
        key: TabKey,
        viewer_id: String,
        max_width: u32,
        max_height: u32,
        quality: u8,
    },
    ViewerDetach {
        key: TabKey,
        viewer_id: String,
    },
    ViewerInput {
        key: TabKey,
        viewer_id: String,
        input: Value,
    },
    ViewerAck {
        key: TabKey,
        viewer_id: String,
        frame_id: u64,
    },
    AnswerFileChooser {
        key: TabKey,
        chooser_id: String,
        files: Vec<UploadFile>,
    },
    HtmlCapture {
        html: String,
        width: u32,
        appearance: String,
        fragment: String,
    },
    HtmlMeasure {
        html: String,
        widths: Vec<u32>,
        fragment: String,
    },
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadFile {
    pub name: String,
    pub mime_type: String,
    pub data: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(
    tag = "event",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BrowserEvent {
    Worker {
        generation: u64,
        message: Box<BrowserEvent>,
    },
    Disconnected,
    Status {
        input: PreviewReportStatusInput,
    },
    Closed {
        key: TabKey,
    },
    Crashed {
        key: TabKey,
        url: String,
    },
    Popup {
        opener: TabKey,
        popup_id: String,
        url: Option<String>,
    },
    Viewer {
        key: TabKey,
        viewer_id: Option<String>,
        output: Value,
    },
    Diagnostic {
        message: String,
    },
}

/// Browser ownership/automation boundary. Desktop shells supply CDP through DesktopBrowser.
#[async_trait]
pub trait BrowserDriver: Send + Sync {
    fn generation(&self) -> u64 {
        0
    }
    async fn prepare(&self) -> Result<u64> {
        Ok(self.generation())
    }
    async fn call_on(
        &self,
        generation: u64,
        command: BrowserCommand,
        timeout: Duration,
    ) -> Result<Value> {
        if generation != self.generation() {
            return Err(Error::Disconnected);
        }
        self.call(command, timeout).await
    }
    async fn call(&self, command: BrowserCommand, timeout: Duration) -> Result<Value>;
    fn subscribe(&self) -> mpsc::UnboundedReceiver<BrowserEvent>;
}

#[derive(Debug, Clone)]
pub struct PlaywrightOptions {
    pub node: PathBuf,
    pub cwd: PathBuf,
    pub executable: PathBuf,
    pub profiles_dir: PathBuf,
    pub downloads_dir: PathBuf,
    pub attachments_dir: PathBuf,
    /// Absolute package entry point, or None to resolve playwright-core from cwd.
    pub playwright_module: Option<PathBuf>,
    pub sandbox: bool,
}

type Pending = Arc<Mutex<BTreeMap<u64, oneshot::Sender<Result<Value>>>>>;
pub struct PlaywrightDriver {
    generation: u64,
    stdin: AsyncMutex<tokio::process::ChildStdin>,
    pending: Pending,
    sequence: AtomicU64,
    events: Events<BrowserEvent>,
    stop: CancellationToken,
}

impl PlaywrightDriver {
    pub async fn start(options: PlaywrightOptions) -> Result<Self> {
        Self::start_with_environment(options, t3_process::Environment::new()).await
    }
    pub async fn start_with_environment(
        options: PlaywrightOptions,
        environment: t3_process::Environment,
    ) -> Result<Self> {
        static GENERATION: AtomicU64 = AtomicU64::new(1);
        let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
        let runtime = tempfile::Builder::new()
            .prefix("t3-preview-driver-")
            .tempdir()?;
        for (name, source) in [
            ("worker.mjs", include_str!("../driver/worker.mjs")),
            ("page.mjs", include_str!("../driver/page.mjs")),
            ("contexts.mjs", include_str!("../driver/contexts.mjs")),
            ("pdf.mjs", include_str!("../driver/pdf.mjs")),
            ("proxy.mjs", include_str!("../driver/proxy.mjs")),
            ("scripts.mjs", include_str!("../driver/scripts.mjs")),
            ("resources.mjs", include_str!("../driver/resources.mjs")),
            (
                "desktop-viewport.mjs",
                include_str!("../driver/desktop-viewport.mjs"),
            ),
            ("screencast.mjs", include_str!("../driver/screencast.mjs")),
            (
                "recording-frames.mjs",
                include_str!("../driver/recording-frames.mjs"),
            ),
        ] {
            std::fs::write(runtime.path().join(name), source)?;
        }
        let mut process = t3_process::ProcessOptions::new(options.node);
        process.args = vec![runtime.path().join("worker.mjs").into_os_string()];
        process.cwd = Some(options.cwd);
        process.env = environment;
        process.timeout = None;
        process.env.insert(
            "T3_PREVIEW_CONFIG".into(),
            Some(
                serde_json::json!({
                    "executable":options.executable,"profilesDir":options.profiles_dir,
                    "downloadsDir":options.downloads_dir,"attachmentsDir":options.attachments_dir,
                    "sandbox":options.sandbox,
                })
                .to_string()
                .into(),
            ),
        );
        if let Some(module) = options.playwright_module {
            process.env.insert(
                "T3_PREVIEW_PLAYWRIGHT_MODULE".into(),
                Some(module.into_os_string()),
            );
        }
        let mut child = tokio::task::spawn_blocking(move || t3_process::spawn(&process))
            .await
            .map_err(|e| Error::Io(e.to_string()))??;
        let stdin = child.stdin.take().ok_or(Error::Disconnected)?;
        let stdout = child.stdout.take().ok_or(Error::Disconnected)?;
        let stderr = child.stderr.take().ok_or(Error::Disconnected)?;
        let pending: Pending = Arc::default();
        let events = Events::default();
        let stop = CancellationToken::new();
        let reader_pending = pending.clone();
        let reader_events = events.clone();
        let reader_stop = stop.clone();
        tokio::spawn(async move {
            let mut stdout = BufReader::new(stdout);
            while let Ok(Some(line)) = read_bounded_line(&mut stdout, 150 * 1024 * 1024).await {
                let Ok(message) = serde_json::from_slice::<Value>(&line) else {
                    tracing::warn!("Ignoring malformed preview driver JSON event");
                    continue;
                };
                if let Some(id) = message["id"].as_u64() {
                    let result = if message["ok"] == true {
                        Ok(message["result"].clone())
                    } else {
                        Err(Error::Operation {
                            tag: message["error"]["tag"]
                                .as_str()
                                .unwrap_or("PreviewAutomationExecutionError")
                                .into(),
                            message: message["error"]["message"]
                                .as_str()
                                .unwrap_or("Browser operation failed")
                                .into(),
                            detail: message["error"].get("detail").cloned(),
                        })
                    };
                    if let Some(reply) = lock(&reader_pending).remove(&id) {
                        let _ = reply.send(result);
                    }
                } else if let Ok(event) = serde_json::from_value(message) {
                    reader_events.publish(BrowserEvent::Worker {
                        generation,
                        message: Box::new(event),
                    });
                } else {
                    tracing::warn!("Ignoring unknown or malformed preview driver event");
                }
            }
            let pending = std::mem::take(&mut *lock(&reader_pending));
            for reply in pending.into_values() {
                let _ = reply.send(Err(Error::Disconnected));
            }
            reader_stop.cancel();
            reader_events.publish(BrowserEvent::Worker {
                generation,
                message: Box::new(BrowserEvent::Disconnected),
            });
        });
        let monitor_stop = stop.clone();
        tokio::spawn(async move {
            let _runtime = runtime;
            tokio::select! { _ = monitor_stop.cancelled() => { let _ = child.terminate(Duration::from_secs(2)).await; }, _ = child.wait() => {} }
            monitor_stop.cancel();
        });
        tokio::spawn(async move {
            let mut stderr = stderr;
            let mut bytes = [0u8; 2048];
            while let Ok(count) = stderr.read(&mut bytes).await {
                if count == 0 {
                    break;
                }
                tracing::debug!(output=%String::from_utf8_lossy(&bytes[..count]),"preview driver stderr");
            }
        });
        Ok(Self {
            generation,
            stdin: AsyncMutex::new(stdin),
            pending,
            sequence: AtomicU64::new(0),
            events,
            stop,
        })
    }
}

struct Waiting {
    pending: Pending,
    id: u64,
}

struct Writing {
    stop: CancellationToken,
    complete: bool,
}
impl Drop for Writing {
    fn drop(&mut self) {
        if !self.complete {
            self.stop.cancel();
        }
    }
}
impl Drop for Waiting {
    fn drop(&mut self) {
        lock(&self.pending).remove(&self.id);
    }
}

#[async_trait]
impl BrowserDriver for PlaywrightDriver {
    fn generation(&self) -> u64 {
        self.generation
    }
    async fn call(&self, command: BrowserCommand, timeout: Duration) -> Result<Value> {
        let opening = matches!(command, BrowserCommand::Open { .. });
        if self.stop.is_cancelled() {
            return Err(Error::Disconnected);
        }
        let id = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id, tx);
        let _waiting = Waiting {
            pending: self.pending.clone(),
            id,
        };
        let mut payload = serde_json::to_value(command)?;
        payload["id"] = id.into();
        let mut bytes = serde_json::to_vec(&payload)?;
        if bytes.len() > 150 * 1024 * 1024 {
            return Err(Error::InvalidInput(
                "Browser command exceeds protocol limit".into(),
            ));
        }
        bytes.push(b'\n');
        let mut value=tokio::time::timeout(timeout,async {
            {
                let mut stdin = self.stdin.lock().await;
                let mut writing=Writing {stop:self.stop.clone(),complete:false};
                stdin.write_all(&bytes).await?;
                stdin.flush().await?;
                writing.complete=true;
            }
            tokio::select! { reply = rx => reply.unwrap_or(Err(Error::Disconnected)), _ = self.stop.cancelled() => Err(Error::Disconnected) }
        }).await.map_err(|_| Error::Timeout("browser driver".into()))??;
        if opening {
            value
                .as_object_mut()
                .ok_or_else(|| Error::InvalidInput("Invalid browser open reply".into()))?
                .insert("_workerGeneration".into(), self.generation.into());
        }
        Ok(value)
    }
    fn subscribe(&self) -> mpsc::UnboundedReceiver<BrowserEvent> {
        self.events.subscribe()
    }
}
impl Drop for PlaywrightDriver {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
impl PlaywrightDriver {
    pub fn is_connected(&self) -> bool {
        !self.stop.is_cancelled()
    }
}

pub(crate) async fn read_bounded_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max: usize,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "partial driver frame",
                ))
            };
        }
        let end = chunk.iter().position(|b| *b == b'\n').map(|n| n + 1);
        let count = end.unwrap_or(chunk.len());
        if line.len() + count > max {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "driver frame limit",
            ));
        }
        line.extend_from_slice(&chunk[..count]);
        reader.consume(count);
        if end.is_some() {
            return Ok(Some(line));
        }
    }
}
