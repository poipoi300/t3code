use crate::{
    CancellationToken,
    speech::{
        EngineCapabilities, LoadedSpeechEngine, MAX_SPEECH_BYTES, NativeOptions,
        SpeechEngineFactory, SpeechError, StreamUpdate,
    },
};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{mpsc, oneshot},
};

pub const MAX_WORKER_FRAME: usize = 32 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WorkerCommand {
    Discover,
    Load { path: PathBuf, acceleration: String },
    Begin { options: NativeOptions },
    Transcribe { pcm: String, options: NativeOptions },
    Feed { pcm: String },
    Finish,
    Reset,
}
#[derive(Serialize, Deserialize)]
pub struct WorkerReply {
    pub error: Option<SpeechError>,
    pub capabilities: Option<EngineCapabilities>,
    pub update: Option<StreamUpdate>,
    pub text: Option<String>,
    pub devices: Option<Vec<t3_contracts::speech::SpeechGpuDevice>>,
}
impl WorkerReply {
    pub fn empty() -> Self {
        Self {
            error: None,
            capabilities: None,
            update: None,
            text: None,
            devices: None,
        }
    }
}
struct Pending {
    command: WorkerCommand,
    reply: oneshot::Sender<Result<WorkerReply, SpeechError>>,
    cancellation: CancellationToken,
}
struct Actor {
    send: mpsc::Sender<Pending>,
    lifetime: CancellationToken,
}
impl Drop for Actor {
    fn drop(&mut self) {
        self.lifetime.cancel();
    }
}
impl Actor {
    async fn spawn(executable: PathBuf, cpu: bool) -> Result<Arc<Self>, SpeechError> {
        let mut options = t3_process::ProcessOptions::new(executable);
        if cpu {
            options
                .env
                .insert("TRANSCRIBE_BACKENDS".into(), Some("cpu".into()));
        }
        let mut child = tokio::task::spawn_blocking(move || t3_process::spawn(&options))
            .await
            .map_err(|_| SpeechError::Inference)?
            .map_err(|_| SpeechError::Inference)?;
        let mut stdin = child.stdin.take().ok_or(SpeechError::Inference)?;
        let mut stdout = child.stdout.take().ok_or(SpeechError::Inference)?;
        let stderr = child.stderr.take();
        let (send, mut receive) = mpsc::channel::<Pending>(1);
        let lifetime = CancellationToken::new();
        let stop = lifetime.clone();
        tokio::spawn(async move {
            if let Some(mut stderr) = stderr {
                let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
            }
        });
        tokio::spawn(async move {
            loop {
                let pending = tokio::select! {biased;_=stop.cancelled()=>break,pending=receive.recv()=>match pending{Some(pending)=>pending,None=>break}};
                let work = async {
                    let frame =
                        serde_json::to_vec(&pending.command).map_err(|_| SpeechError::Inference)?;
                    if frame.len() > MAX_WORKER_FRAME {
                        return Err(SpeechError::InvalidAudio);
                    }
                    stdin
                        .write_all(&(frame.len() as u32).to_le_bytes())
                        .await
                        .map_err(|_| SpeechError::Inference)?;
                    stdin
                        .write_all(&frame)
                        .await
                        .map_err(|_| SpeechError::Inference)?;
                    stdin.flush().await.map_err(|_| SpeechError::Inference)?;
                    let length = stdout
                        .read_u32_le()
                        .await
                        .map_err(|_| SpeechError::Inference)?
                        as usize;
                    if length > MAX_WORKER_FRAME {
                        return Err(SpeechError::Inference);
                    }
                    let mut bytes = vec![0; length];
                    stdout
                        .read_exact(&mut bytes)
                        .await
                        .map_err(|_| SpeechError::Inference)?;
                    let reply: WorkerReply =
                        serde_json::from_slice(&bytes).map_err(|_| SpeechError::Inference)?;
                    if let Some(error) = reply.error {
                        Err(error)
                    } else {
                        Ok(reply)
                    }
                };
                let result = tokio::select! {biased;_=stop.cancelled()=>Err(SpeechError::Cancelled),_=pending.cancellation.cancelled()=>Err(SpeechError::Cancelled),result=work=>result};
                let failed = result.is_err();
                let _ = pending.reply.send(result);
                if failed {
                    break;
                }
            }
            let _ = child.kill().await;
        });
        Ok(Arc::new(Self { send, lifetime }))
    }
    async fn call(
        &self,
        command: WorkerCommand,
        cancellation: CancellationToken,
    ) -> Result<WorkerReply, SpeechError> {
        let token = cancellation.child_token();
        struct Guard(CancellationToken);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.cancel();
            }
        }
        let _guard = Guard(token.clone());
        let (send, receive) = oneshot::channel();
        self.send
            .send(Pending {
                command,
                reply: send,
                cancellation: token,
            })
            .await
            .map_err(|_| SpeechError::Inference)?;
        tokio::select! {biased;_=cancellation.cancelled()=>Err(SpeechError::Cancelled),reply=receive=>reply.map_err(|_|SpeechError::Inference)?}
    }
}
pub struct NativeSpeechFactory {
    worker: PathBuf,
}
impl NativeSpeechFactory {
    pub fn new(worker: PathBuf) -> Self {
        Self { worker }
    }
}
struct NativeEngine {
    actor: Arc<Actor>,
    capabilities: EngineCapabilities,
}
fn encode_pcm(pcm: &[f32]) -> Result<String, SpeechError> {
    if pcm.len() > MAX_SPEECH_BYTES / 4
        || pcm
            .iter()
            .any(|s| !s.is_finite() || !(-1.0..=1.0).contains(s))
    {
        return Err(SpeechError::InvalidAudio);
    }
    let mut bytes = Vec::with_capacity(pcm.len() * 4);
    for sample in pcm {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(STANDARD.encode(bytes))
}
#[async_trait]
impl SpeechEngineFactory for NativeSpeechFactory {
    async fn load(
        &self,
        path: PathBuf,
        acceleration: String,
        cancellation: CancellationToken,
    ) -> Result<Box<dyn LoadedSpeechEngine>, SpeechError> {
        let actor = Actor::spawn(self.worker.clone(), acceleration == "cpu").await?;
        let reply = actor
            .call(WorkerCommand::Load { path, acceleration }, cancellation)
            .await?;
        Ok(Box::new(NativeEngine {
            actor,
            capabilities: reply.capabilities.ok_or(SpeechError::Inference)?,
        }))
    }
    async fn gpu_devices(&self) -> Result<Vec<t3_contracts::speech::SpeechGpuDevice>, SpeechError> {
        let actor = Actor::spawn(self.worker.clone(), false).await?;
        tokio::time::timeout(
            Duration::from_secs(10),
            actor.call(WorkerCommand::Discover, CancellationToken::new()),
        )
        .await
        .map_err(|_| SpeechError::Timeout)?
        .map(|r| r.devices.unwrap_or_default())
    }
}
#[async_trait]
impl LoadedSpeechEngine for NativeEngine {
    fn capabilities(&self) -> &EngineCapabilities {
        &self.capabilities
    }
    async fn transcribe(
        &mut self,
        pcm: &[f32],
        options: NativeOptions,
        cancellation: CancellationToken,
    ) -> Result<String, SpeechError> {
        self.actor
            .call(
                WorkerCommand::Transcribe {
                    pcm: encode_pcm(pcm)?,
                    options,
                },
                cancellation,
            )
            .await?
            .text
            .ok_or(SpeechError::Inference)
    }
    async fn begin(
        &mut self,
        options: NativeOptions,
        cancellation: CancellationToken,
    ) -> Result<(), SpeechError> {
        self.actor
            .call(WorkerCommand::Begin { options }, cancellation)
            .await?;
        Ok(())
    }
    async fn feed(
        &mut self,
        pcm: &[f32],
        cancellation: CancellationToken,
    ) -> Result<StreamUpdate, SpeechError> {
        self.actor
            .call(
                WorkerCommand::Feed {
                    pcm: encode_pcm(pcm)?,
                },
                cancellation,
            )
            .await?
            .update
            .ok_or(SpeechError::Inference)
    }
    async fn finish(&mut self, cancellation: CancellationToken) -> Result<String, SpeechError> {
        self.actor
            .call(WorkerCommand::Finish, cancellation)
            .await?
            .text
            .ok_or(SpeechError::Inference)
    }
    async fn reset(&mut self) -> Result<(), SpeechError> {
        self.actor
            .call(WorkerCommand::Reset, CancellationToken::new())
            .await?;
        Ok(())
    }
}
