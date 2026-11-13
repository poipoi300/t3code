use crate::{
    CancellationToken,
    dictionary::{TextCorrector, normalize_custom_words, remove_filler_words},
    models::{DEFAULT_SPEECH_MODEL_ID, ModelStore, SpeechModel, catalog, silero},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use t3_contracts::speech::{
    EnvironmentSpeechModel, EnvironmentSpeechModelState, EnvironmentSpeechState,
    EnvironmentSpeechStatus, SpeechBenchmark, SpeechBenchmarkTier, SpeechGpuDevice,
    SpeechModelUnloadTimeout, SpeechStreamCommand, SpeechStreamEvent, SpeechStreamText,
    SpeechTranscriptionOptions, SupportedSpeechStatus, UnsupportedSpeechStatus,
};
use tokio::{
    sync::{Mutex, OwnedMutexGuard},
    time::Instant,
};

pub const SAMPLE_RATE: usize = 16000;
pub const MAX_SPEECH_BYTES: usize = SAMPLE_RATE * 4 * 5 * 60;
pub const MAX_OPTIONS_BYTES: usize = 128000;
pub const MAX_CHUNK_BYTES: usize = SAMPLE_RATE * 4;
pub const MAX_BODY_BYTES: usize = MAX_SPEECH_BYTES + MAX_OPTIONS_BYTES + 4;
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
pub enum SpeechError {
    #[error(
        "Audio must contain at most five minutes of 16 kHz mono Float32 PCM with finite samples in [-1, 1]."
    )]
    InvalidAudio,
    #[error("Invalid speech options.")]
    InvalidOptions,
    #[error("Environment speech is busy.")]
    Busy,
    #[error("Voice transcription is unavailable on this platform.")]
    UnsupportedPlatform,
    #[error("The speech model is not available.")]
    ModelNotFound,
    #[error("Speech transcription was cancelled.")]
    Cancelled,
    #[error("Environment speech inference failed.")]
    Inference,
    #[error("Environment speech storage failed.")]
    Storage,
    #[error("Speech model download failed.")]
    Download,
    #[error("Speech model transfer failed.")]
    Transport,
    #[error("Speech model download returned an invalid range.")]
    InvalidRange,
    #[error("Speech model download returned an invalid size.")]
    InvalidSize,
    #[error("Speech model verification failed.")]
    Verification,
    #[error("Invalid Silero frame sequence.")]
    InvalidVad,
    #[error("The speech stream is not ready.")]
    StreamNotReady,
    #[error("Speech operation took too long.")]
    Timeout,
}
pub fn decode_pcm(bytes: &[u8]) -> Result<Vec<f32>, SpeechError> {
    if bytes.is_empty() || bytes.len() > MAX_SPEECH_BYTES || !bytes.len().is_multiple_of(4) {
        return Err(SpeechError::InvalidAudio);
    }
    let valid = |sample: f32| sample.is_finite() && (-1.0..=1.0).contains(&sample);
    if bytes
        .as_chunks::<4>()
        .0
        .iter()
        .any(|v| !valid(f32::from_le_bytes([v[0], v[1], v[2], v[3]])))
    {
        return Err(SpeechError::InvalidAudio);
    }
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| f32::from_le_bytes([v[0], v[1], v[2], v[3]]))
        .collect())
}
pub fn decode_request(body: &[u8]) -> Result<(SpeechTranscriptionOptions, &[u8]), SpeechError> {
    if body.len() < 4 || body.len() > MAX_BODY_BYTES {
        return Err(SpeechError::InvalidAudio);
    }
    let length = u32::from_le_bytes([body[0], body[1], body[2], body[3]]) as usize;
    if length > MAX_OPTIONS_BYTES || 4 + length >= body.len() {
        return Err(SpeechError::InvalidOptions);
    }
    let options = decode_options(&body[4..4 + length])?;
    Ok((options, &body[4 + length..]))
}
pub fn decode_options(bytes: &[u8]) -> Result<SpeechTranscriptionOptions, SpeechError> {
    if bytes.len() > MAX_OPTIONS_BYTES {
        return Err(SpeechError::InvalidOptions);
    }
    let options: SpeechTranscriptionOptions =
        serde_json::from_slice(bytes).map_err(|_| SpeechError::InvalidOptions)?;
    validate_options(&options)?;
    Ok(options)
}
pub fn validate_options(options: &SpeechTranscriptionOptions) -> Result<(), SpeechError> {
    use unicode_normalization::UnicodeNormalization;
    let mut seen = std::collections::HashSet::new();
    if !(2..=16).contains(&crate::text::utf16_len(&options.speech_language))
        || options
            .speech_correction_word
            .as_ref()
            .is_some_and(|s| crate::text::utf16_len(s) > 50)
        || options.speech_custom_words.len() > 100
    {
        return Err(SpeechError::InvalidOptions);
    }
    for word in &options.speech_custom_words {
        if word.aliases.len() > 8 {
            return Err(SpeechError::InvalidOptions);
        }
        for value in std::iter::once(&word.term).chain(&word.aliases) {
            if value.trim().is_empty()
                || crate::text::utf16_len(value) > 50
                || !seen.insert(value.trim().nfc().collect::<String>().to_lowercase())
            {
                return Err(SpeechError::InvalidOptions);
            }
        }
    }
    Ok(())
}
pub fn effective_language(languages: &[String], detect: bool, intent: &str) -> String {
    if languages.len() == 1 {
        return languages[0].clone();
    }
    if intent != "auto" && languages.iter().any(|l| l == intent) {
        return intent.into();
    }
    if detect {
        "auto".into()
    } else {
        languages
            .iter()
            .find(|l| l.as_str() == "en")
            .or(languages.first())
            .cloned()
            .unwrap_or_else(|| "auto".into())
    }
}
pub fn native_language(advertised: &[String], language: &str) -> Option<String> {
    if language == "auto" {
        return None;
    }
    let tag = |s: &str| s.to_lowercase().replace('_', "-");
    let wanted = tag(language);
    advertised
        .iter()
        .find(|s| s.as_str() == language)
        .or_else(|| advertised.iter().find(|s| tag(s) == wanted))
        .or_else(|| {
            if wanted.contains('-') {
                None
            } else {
                advertised
                    .iter()
                    .find(|s| tag(s).split('-').next() == Some(&wanted))
            }
        })
        .cloned()
        .or_else(|| Some(language.into()))
}
pub fn platform_supported(platform: &str, arch: &str, libc: Option<&str>) -> bool {
    (platform != "linux" || libc == Some("gnu"))
        && matches!(
            (platform, arch),
            ("darwin", "arm64" | "x64") | ("win32", "x64") | ("linux", "x64" | "arm64")
        )
}
#[derive(Clone, Serialize, Deserialize)]
pub struct SpeechSettings {
    pub model_id: String,
    pub acceleration: String,
    pub unload_timeout: SpeechModelUnloadTimeout,
}
impl Default for SpeechSettings {
    fn default() -> Self {
        Self {
            model_id: DEFAULT_SPEECH_MODEL_ID.into(),
            acceleration: "auto".into(),
            unload_timeout: SpeechModelUnloadTimeout::Min15,
        }
    }
}
#[async_trait]
pub trait SpeechSettingsBackend: Send + Sync {
    async fn read(&self) -> Result<SpeechSettings, SpeechError>;
    async fn write(&self, settings: SpeechSettings) -> Result<(), SpeechError>;
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct EngineCapabilities {
    pub backend: String,
    pub languages: Vec<String>,
    pub streaming: bool,
    pub long_form: bool,
    pub translation: bool,
    pub vocabulary: bool,
    pub max_audio_ms: u64,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct NativeOptions {
    pub language: Option<String>,
    pub translate: bool,
    pub vocabulary: Vec<String>,
    pub filter_silence: bool,
    pub vad_path: Option<PathBuf>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct StreamUpdate {
    pub revision: i64,
    pub text: Option<SpeechStreamText>,
}
#[async_trait]
pub trait LoadedSpeechEngine: Send + Sync {
    fn capabilities(&self) -> &EngineCapabilities;
    async fn transcribe(
        &mut self,
        pcm: &[f32],
        options: NativeOptions,
        cancellation: CancellationToken,
    ) -> Result<String, SpeechError>;
    async fn begin(
        &mut self,
        options: NativeOptions,
        cancellation: CancellationToken,
    ) -> Result<(), SpeechError>;
    async fn feed(
        &mut self,
        pcm: &[f32],
        cancellation: CancellationToken,
    ) -> Result<StreamUpdate, SpeechError>;
    async fn finish(&mut self, cancellation: CancellationToken) -> Result<String, SpeechError>;
    async fn reset(&mut self) -> Result<(), SpeechError>;
}
#[async_trait]
pub trait SpeechEngineFactory: Send + Sync {
    async fn load(
        &self,
        path: PathBuf,
        acceleration: String,
        cancellation: CancellationToken,
    ) -> Result<Box<dyn LoadedSpeechEngine>, SpeechError>;
    async fn gpu_devices(&self) -> Result<Vec<SpeechGpuDevice>, SpeechError> {
        Ok(Vec::new())
    }
}
struct State {
    engine: Option<Box<dyn LoadedSpeechEngine>>,
    model_id: String,
    acceleration: String,
    auto_cpu: Option<String>,
}
struct Download {
    id: String,
    bytes: Arc<AtomicU64>,
    verifying: Arc<AtomicBool>,
    cancel: CancellationToken,
    result: tokio::sync::watch::Receiver<Option<Result<(), SpeechError>>>,
}
struct Progress {
    bytes: Arc<AtomicU64>,
    verifying: Arc<AtomicBool>,
}
impl crate::models::DownloadProgress for Progress {
    fn progress(&self, bytes: u64) {
        self.bytes.store(bytes, Ordering::SeqCst);
        self.verifying.store(false, Ordering::SeqCst);
    }
    fn verifying(&self) {
        self.verifying.store(true, Ordering::SeqCst);
    }
}
struct Inner {
    active: AtomicUsize,
    gpu: Mutex<Option<(Option<Instant>, Vec<SpeechGpuDevice>)>>,
    downloads: std::sync::Mutex<Option<Download>>,
    catalog: Vec<SpeechModel>,
    state: Arc<Mutex<State>>,
    store: Arc<ModelStore>,
    factory: Arc<dyn SpeechEngineFactory>,
    settings: Arc<dyn SpeechSettingsBackend>,
    supported: bool,
    epoch: AtomicU64,
    silence_retry: Mutex<Option<Instant>>,
}
struct Activity(Arc<Inner>);
impl Activity {
    fn new(inner: Arc<Inner>) -> Self {
        inner.active.fetch_add(1, Ordering::SeqCst);
        Self(inner)
    }
}
impl Drop for Activity {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}
#[derive(Clone)]
pub struct SpeechService {
    inner: Arc<Inner>,
}
fn unload_duration(timeout: &SpeechModelUnloadTimeout) -> Option<Duration> {
    use SpeechModelUnloadTimeout::*;
    Some(Duration::from_secs(match timeout {
        Never => return None,
        Immediately => 0,
        Min2 => 120,
        Min5 => 300,
        Min10 => 600,
        Min15 => 900,
        Hour1 => 3600,
    }))
}
impl Inner {
    async fn devices(&self) -> Vec<SpeechGpuDevice> {
        let mut cached = self.gpu.lock().await;
        if let Some((expires, devices)) = &*cached
            && expires.is_none_or(|at| Instant::now() < at)
        {
            return devices.clone();
        }
        let (expires, devices) = match self.factory.gpu_devices().await {
            Ok(devices) => (None, devices),
            Err(_) => (Some(Instant::now() + Duration::from_secs(60)), Vec::new()),
        };
        *cached = Some((expires, devices.clone()));
        devices
    }
    fn schedule_unload(self: &Arc<Self>) {
        let start = Instant::now();
        let epoch = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let Ok(settings) = inner.settings.read().await else {
                return;
            };
            let Some(delay) = unload_duration(&settings.unload_timeout) else {
                return;
            };
            drop(inner);
            tokio::time::sleep_until(start + delay).await;
            if let Some(inner) = weak.upgrade() {
                let mut state = inner.state.lock().await;
                if inner.epoch.load(Ordering::SeqCst) == epoch {
                    state.engine = None;
                }
            }
        });
    }
    async fn silence_path(
        &self,
        cap: &EngineCapabilities,
        options: &SpeechTranscriptionOptions,
    ) -> Option<PathBuf> {
        if !options.speech_silence_filtering && (cap.streaming || cap.long_form) {
            return None;
        }
        let mut retry = self.silence_retry.lock().await;
        if retry.is_some_and(|at| Instant::now() < at) {
            return None;
        }
        let token = CancellationToken::new();
        let result = tokio::time::timeout(
            Duration::from_secs(15),
            self.store.download(&silero(), &(), token.clone()),
        )
        .await;
        token.cancel();
        if let Ok(Ok(path)) = result {
            *retry = None;
            Some(path)
        } else {
            *retry = Some(Instant::now() + Duration::from_secs(600));
            None
        }
    }
}
impl SpeechService {
    pub fn new(
        store: Arc<ModelStore>,
        factory: Arc<dyn SpeechEngineFactory>,
        settings: Arc<dyn SpeechSettingsBackend>,
        supported: bool,
    ) -> Self {
        Self::with_catalog(store, factory, settings, supported, catalog())
    }
    pub fn with_catalog(
        store: Arc<ModelStore>,
        factory: Arc<dyn SpeechEngineFactory>,
        settings: Arc<dyn SpeechSettingsBackend>,
        supported: bool,
        models: Vec<SpeechModel>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                active: AtomicUsize::new(0),
                gpu: Mutex::new(None),
                downloads: std::sync::Mutex::new(None),
                catalog: models,
                state: Arc::new(Mutex::new(State {
                    engine: None,
                    model_id: String::new(),
                    acceleration: String::new(),
                    auto_cpu: None,
                })),
                store,
                factory,
                settings,
                supported,
                epoch: AtomicU64::new(0),
                silence_retry: Mutex::new(None),
            }),
        }
    }
    fn definition(&self, id: &str) -> SpeechModel {
        let models = &self.inner.catalog;
        models
            .iter()
            .find(|m| m.id == id)
            .or_else(|| models.iter().find(|m| m.id == DEFAULT_SPEECH_MODEL_ID))
            .expect("default catalog model")
            .clone()
    }
    fn reserve(&self) -> Result<OwnedMutexGuard<State>, SpeechError> {
        if !self.inner.supported {
            return Err(SpeechError::UnsupportedPlatform);
        }
        let state = self
            .inner
            .state
            .clone()
            .try_lock_owned()
            .map_err(|_| SpeechError::Busy)?;
        self.inner.epoch.fetch_add(1, Ordering::SeqCst);
        Ok(state)
    }
    async fn load(
        &self,
        state: &mut State,
        definition: &SpeechModel,
        acceleration: &str,
        token: CancellationToken,
    ) -> Result<(), SpeechError> {
        let accel = if acceleration == "auto" && state.auto_cpu.as_deref() == Some(&definition.id) {
            "cpu"
        } else {
            acceleration
        };
        if state.engine.is_some() && state.model_id == definition.id && state.acceleration == accel
        {
            return Ok(());
        }
        state.engine = None;
        let path = self
            .inner
            .store
            .verified_installed_path(definition)
            .await
            .ok_or(SpeechError::ModelNotFound)?;
        let mut loaded = self
            .inner
            .factory
            .load(path.clone(), accel.into(), token.clone())
            .await;
        if loaded.is_err() && accel == "auto" && !token.is_cancelled() {
            loaded = self
                .inner
                .factory
                .load(path, "cpu".into(), token.clone())
                .await;
        }
        state.engine = Some(loaded?);
        state.model_id = definition.id.clone();
        state.acceleration = accel.into();
        Ok(())
    }
    pub async fn status(&self) -> Result<EnvironmentSpeechStatus, SpeechError> {
        if !self.inner.supported {
            return Ok(EnvironmentSpeechStatus::Unsupported(
                UnsupportedSpeechStatus {
                    supported: t3_contracts::speech::False,
                    reason: "voice transcription is not available on this platform".into(),
                },
            ));
        }
        let settings = self.inner.settings.read().await?;
        let model = self.definition(&settings.model_id);
        let cap = {
            let state = self.inner.state.try_lock();
            state
                .as_ref()
                .ok()
                .and_then(|s| s.engine.as_ref())
                .filter(|_| state.as_ref().is_ok_and(|s| s.model_id == model.id))
                .map(|e| e.capabilities().clone())
        };
        let devices = self.inner.devices().await;
        let cpu =
            settings.acceleration == "cpu" || settings.acceleration == "auto" && devices.is_empty();
        let status = if self.inner.active.load(Ordering::SeqCst) > 0 {
            EnvironmentSpeechState::Transcribing
        } else if self.inner.store.is_ready(&model) {
            EnvironmentSpeechState::Ready
        } else {
            EnvironmentSpeechState::MissingModel
        };
        let streaming = cap
            .as_ref()
            .map_or(model.supports_streaming, |c| c.streaming);
        Ok(EnvironmentSpeechStatus::Supported(Box::new(
            SupportedSpeechStatus {
                supported: t3_contracts::speech::True,
                state: status,
                model_id: model.id,
                model: model.name,
                size: model.size.into(),
                supports_streaming: streaming,
                supports_buffered_streaming: Some(true),
                supports_model_linking: Some(true),
                supports_translation: cap
                    .as_ref()
                    .map_or(model.supports_translation, |c| c.translation),
                languages: Some(model.languages),
                supports_language_detection: Some(model.supports_language_detection),
                acceleration: settings.acceleration,
                benchmark_tier: if cpu {
                    SpeechBenchmarkTier::Cpu
                } else {
                    SpeechBenchmarkTier::Accelerated
                },
                model_unload_timeout: settings.unload_timeout,
                gpu_devices: devices,
            },
        )))
    }
    pub async fn models(
        &self,
        benchmarks: bool,
    ) -> Result<Vec<EnvironmentSpeechModel>, SpeechError> {
        let settings = self.inner.settings.read().await?;
        Ok(self
            .inner
            .catalog
            .clone()
            .into_iter()
            .map(|model| EnvironmentSpeechModel {
                id: model.id.clone(),
                name: model.name.clone(),
                description: model.description.clone(),
                size: model.size.into(),
                languages: model.languages.clone(),
                accuracy: 0.into(),
                speed: 0.into(),
                benchmark: if benchmarks {
                    serde_json::from_value::<SpeechBenchmark>(model.benchmark.clone()).ok()
                } else {
                    None
                },
                recommended: model.recommended,
                supports_streaming: model.supports_streaming,
                supports_language_detection: model.supports_language_detection,
                supports_translation: model.supports_translation,
                active: model.id == settings.model_id,
                state: self
                    .inner
                    .downloads
                    .lock()
                    .ok()
                    .and_then(|d| {
                        d.as_ref().filter(|d| d.id == model.id).map(|d| {
                            if d.verifying.load(Ordering::SeqCst) {
                                EnvironmentSpeechModelState::Verifying
                            } else {
                                EnvironmentSpeechModelState::Downloading
                            }
                        })
                    })
                    .unwrap_or_else(|| {
                        if self.inner.store.is_ready(&model) {
                            EnvironmentSpeechModelState::Installed
                        } else {
                            EnvironmentSpeechModelState::Downloadable
                        }
                    }),
                downloaded: self.inner.downloads.lock().ok().and_then(|d| {
                    d.as_ref()
                        .filter(|d| d.id == model.id)
                        .map(|d| d.bytes.load(Ordering::SeqCst).into())
                }),
                linked_from: self
                    .inner
                    .store
                    .linked_from(&model)
                    .map(|p| p.to_string_lossy().into_owned()),
            })
            .collect())
    }
    pub async fn prepare_model(&self, cancellation: CancellationToken) -> Result<(), SpeechError> {
        let mut state = self.reserve()?;
        let settings = self.inner.settings.read().await?;
        let model = self.definition(&settings.model_id);
        let result = tokio::time::timeout(
            Duration::from_secs(120),
            self.load(&mut state, &model, &settings.acceleration, cancellation),
        )
        .await
        .map_err(|_| SpeechError::Timeout)?;
        drop(state);
        self.inner.schedule_unload();
        result
    }
    pub async fn download_model(
        &self,
        id: &str,
        cancellation: CancellationToken,
    ) -> Result<Vec<EnvironmentSpeechModel>, SpeechError> {
        self.download_model_inner(id, cancellation).await?;
        self.models(false).await
    }
    async fn download_model_inner(
        &self,
        id: &str,
        cancellation: CancellationToken,
    ) -> Result<(), SpeechError> {
        let mut result = {
            let mut downloading = self
                .inner
                .downloads
                .lock()
                .map_err(|_| SpeechError::Storage)?;
            if let Some(active) = downloading.as_ref() {
                if active.id != id {
                    return Err(SpeechError::Busy);
                }
                active.result.clone()
            } else {
                let state = self.reserve()?;
                let model = self
                    .inner
                    .catalog
                    .iter()
                    .find(|m| m.id == id)
                    .cloned()
                    .ok_or(SpeechError::ModelNotFound)?;
                let (send, receive) = tokio::sync::watch::channel(None);
                let cancel = CancellationToken::new();
                let bytes = Arc::new(AtomicU64::new(0));
                let verifying = Arc::new(AtomicBool::new(false));
                *downloading = Some(Download {
                    id: id.into(),
                    bytes: bytes.clone(),
                    verifying: verifying.clone(),
                    cancel: cancel.clone(),
                    result: receive.clone(),
                });
                let inner = self.inner.clone();
                tokio::spawn(async move {
                    let progress = Progress { bytes, verifying };
                    let helper = silero();
                    let helper_token = cancel.child_token();
                    let main = inner.store.download(&model, &progress, cancel.clone());
                    let optional = tokio::time::timeout(
                        Duration::from_secs(120),
                        inner.store.download(&helper, &(), helper_token.clone()),
                    );
                    let (result, _) = tokio::join!(main, optional);
                    helper_token.cancel();
                    if let Ok(mut download) = inner.downloads.lock() {
                        *download = None;
                    }
                    drop(state);
                    inner.schedule_unload();
                    let _ = send.send(Some(result.map(|_| ())));
                });
                receive
            }
        };
        loop {
            if let Some(result) = *result.borrow_and_update() {
                return result;
            }
            tokio::select! {biased;_=cancellation.cancelled()=>return Err(SpeechError::Cancelled),changed=result.changed()=>changed.map_err(|_|SpeechError::Download)?}
        }
    }
    pub async fn cancel_download(
        &self,
        id: &str,
    ) -> Result<Vec<EnvironmentSpeechModel>, SpeechError> {
        self.cancel_download_inner(id).await?;
        self.models(false).await
    }
    async fn cancel_download_inner(&self, id: &str) -> Result<(), SpeechError> {
        let receiver = {
            let downloading = self
                .inner
                .downloads
                .lock()
                .map_err(|_| SpeechError::Storage)?;
            downloading.as_ref().filter(|d| d.id == id).map(|d| {
                d.cancel.cancel();
                d.result.clone()
            })
        };
        if let Some(mut receiver) = receiver {
            while receiver.borrow().is_none() {
                receiver
                    .changed()
                    .await
                    .map_err(|_| SpeechError::Download)?;
            }
        }
        Ok(())
    }
    pub async fn select_model(&self, id: &str) -> Result<Vec<EnvironmentSpeechModel>, SpeechError> {
        self.select_model_inner(id).await?;
        self.models(false).await
    }
    async fn select_model_inner(&self, id: &str) -> Result<(), SpeechError> {
        let mut state = self.reserve()?;
        self.inner
            .catalog
            .clone()
            .into_iter()
            .find(|m| m.id == id)
            .ok_or(SpeechError::ModelNotFound)?;
        let mut settings = self.inner.settings.read().await?;
        settings.model_id = id.into();
        self.inner.settings.write(settings).await?;
        state.engine = None;
        Ok(())
    }
    pub async fn remove_model(&self, id: &str) -> Result<Vec<EnvironmentSpeechModel>, SpeechError> {
        self.remove_model_inner(id).await?;
        self.models(false).await
    }
    async fn remove_model_inner(&self, id: &str) -> Result<(), SpeechError> {
        let mut state = self.reserve()?;
        let model = self
            .inner
            .catalog
            .iter()
            .find(|m| m.id == id)
            .ok_or(SpeechError::ModelNotFound)?;
        if state.model_id == id {
            state.engine = None;
        }
        self.inner.store.remove(model)?;
        let mut settings = self.inner.settings.read().await?;
        if self.definition(&settings.model_id).id == id
            && let Some(replacement) = self
                .inner
                .catalog
                .iter()
                .find(|m| m.id != id && self.inner.store.is_ready(m))
        {
            settings.model_id = replacement.id.clone();
            self.inner.settings.write(settings).await?;
        }
        Ok(())
    }
    pub async fn link_models(
        &self,
        source: &std::path::Path,
    ) -> Result<Vec<EnvironmentSpeechModel>, SpeechError> {
        self.link_models_inner(source).await?;
        self.models(false).await
    }
    async fn link_models_inner(
        &self,
        source: &std::path::Path,
    ) -> Result<Vec<String>, SpeechError> {
        let _state = self.reserve()?;
        let linked = self.inner.store.link(&self.inner.catalog, source)?;
        let first = linked.first().ok_or(SpeechError::ModelNotFound)?;
        let mut settings = self.inner.settings.read().await?;
        if !self
            .inner
            .store
            .is_ready(&self.definition(&settings.model_id))
        {
            settings.model_id = first.clone();
            self.inner.settings.write(settings).await?;
        }
        Ok(linked)
    }
    pub async fn update_acceleration(
        &self,
        acceleration: &str,
    ) -> Result<EnvironmentSpeechStatus, SpeechError> {
        self.update_acceleration_inner(acceleration).await?;
        self.status().await
    }
    async fn update_acceleration_inner(&self, acceleration: &str) -> Result<(), SpeechError> {
        let mut state = self.reserve()?;
        if !matches!(acceleration, "auto" | "cpu") && !acceleration.starts_with("gpu:") {
            return Err(SpeechError::InvalidOptions);
        }
        if let Some(id) = acceleration.strip_prefix("gpu:")
            && !self.inner.devices().await.iter().any(|d| d.id == id)
        {
            return Err(SpeechError::InvalidOptions);
        }
        let mut settings = self.inner.settings.read().await?;
        settings.acceleration = acceleration.into();
        self.inner.settings.write(settings).await?;
        state.engine = None;
        state.auto_cpu = None;
        Ok(())
    }
    pub async fn update_unload_timeout(
        &self,
        timeout: SpeechModelUnloadTimeout,
    ) -> Result<EnvironmentSpeechStatus, SpeechError> {
        self.update_unload_timeout_inner(timeout).await?;
        self.status().await
    }
    async fn update_unload_timeout_inner(
        &self,
        timeout: SpeechModelUnloadTimeout,
    ) -> Result<(), SpeechError> {
        let mut settings = self.inner.settings.read().await?;
        settings.unload_timeout = timeout;
        self.inner.settings.write(settings).await?;
        self.inner.schedule_unload();
        Ok(())
    }
    pub async fn transcribe(
        &self,
        bytes: &[u8],
        options: SpeechTranscriptionOptions,
        cancellation: CancellationToken,
    ) -> Result<String, SpeechError> {
        tokio::select! {biased;_=cancellation.cancelled()=>Err(SpeechError::Cancelled),result=self.transcribe_inner(bytes,options,cancellation.clone())=>result}
    }
    async fn transcribe_inner(
        &self,
        bytes: &[u8],
        options: SpeechTranscriptionOptions,
        cancellation: CancellationToken,
    ) -> Result<String, SpeechError> {
        validate_options(&options)?;
        let pcm = decode_pcm(bytes)?;
        let mut state = self.reserve()?;
        let _activity = Activity::new(self.inner.clone());
        let settings = self.inner.settings.read().await?;
        let definition = self.definition(&settings.model_id);
        self.load(
            &mut state,
            &definition,
            &settings.acceleration,
            cancellation.clone(),
        )
        .await?;
        let mut inference = state.engine.take().ok_or(SpeechError::Inference)?;
        let cap = inference.capabilities().clone();
        let vad = self.inner.silence_path(&cap, &options).await;
        let (native, correct, language) =
            native_options(&definition, &cap, &options, vad.clone(), false);
        let mut result = tokio::select! {biased;_=cancellation.cancelled()=>Err(SpeechError::Cancelled),result=inference.transcribe(&pcm,native,cancellation.clone())=>result};
        let mut correct = correct;
        let mut language = language;
        if result.is_err()
            && settings.acceleration == "auto"
            && cap.backend != "cpu"
            && !cancellation.is_cancelled()
        {
            drop(inference);
            self.load(&mut state, &definition, "cpu", cancellation.clone())
                .await?;
            state.auto_cpu = Some(definition.id.clone());
            inference = state.engine.take().ok_or(SpeechError::Inference)?;
            let engine = &mut inference;
            let (native, new_correct, new_language) =
                native_options(&definition, engine.capabilities(), &options, vad, false);
            correct = new_correct;
            language = new_language;
            result = tokio::select! {biased;_=cancellation.cancelled()=>Err(SpeechError::Cancelled),result=engine.transcribe(&pcm,native,cancellation.clone())=>result};
        }
        if result.is_ok() {
            state.engine = Some(inference);
        }
        let output = result.map(|text| finalize(&correct.apply(&text), &options, &language));
        drop(state);
        self.inner.schedule_unload();
        output
    }
    pub async fn start_stream(
        &self,
        options: SpeechTranscriptionOptions,
        cancellation: CancellationToken,
    ) -> Result<SpeechStream, SpeechError> {
        tokio::select! {biased;_=cancellation.cancelled()=>Err(SpeechError::Cancelled),result=self.start_stream_inner(options,cancellation.clone())=>result}
    }
    async fn start_stream_inner(
        &self,
        options: SpeechTranscriptionOptions,
        cancellation: CancellationToken,
    ) -> Result<SpeechStream, SpeechError> {
        validate_options(&options)?;
        let mut state = self.reserve()?;
        let activity = Activity::new(self.inner.clone());
        let settings = self.inner.settings.read().await?;
        let definition = self.definition(&settings.model_id);
        self.load(
            &mut state,
            &definition,
            &settings.acceleration,
            cancellation.clone(),
        )
        .await?;
        let mut engine = state.engine.take().ok_or(SpeechError::Inference)?;
        let vad = self
            .inner
            .silence_path(engine.capabilities(), &options)
            .await;
        let (native, correct, language) = native_options(
            &definition,
            engine.capabilities(),
            &options,
            vad.clone(),
            true,
        );
        tokio::select! {biased;_=cancellation.cancelled()=>return Err(SpeechError::Cancelled),result=engine.begin(native,cancellation.clone())=>result?};
        state.engine = Some(engine);
        Ok(SpeechStream {
            _activity: activity,
            state: Some(state),
            service: self.clone(),
            options,
            settings,
            definition,
            vad,
            correct,
            language,
            received: Vec::new(),
            bytes: 0,
            revision_base: 0,
            last_revision: 0,
            retried: false,
            finished: false,
            cancellation,
        })
    }
}
fn native_options(
    model: &SpeechModel,
    cap: &EngineCapabilities,
    options: &SpeechTranscriptionOptions,
    vad: Option<PathBuf>,
    stream: bool,
) -> (NativeOptions, TextCorrector, String) {
    let language = effective_language(
        &model.languages,
        model.supports_language_detection,
        &options.speech_language,
    );
    let translate = options.speech_translate_to_english
        && (if stream {
            cap.translation && !cap.streaming
        } else {
            model.supports_translation
        })
        && language != "en";
    let dictionary = normalize_custom_words(&options.speech_custom_words);
    let words = dictionary
        .iter()
        .map(|w| w.term.clone())
        .collect::<Vec<_>>();
    let vocabulary = cap.vocabulary && !translate;
    let correct = TextCorrector::new(if vocabulary { &[] } else { &words }, &dictionary);
    (
        NativeOptions {
            language: native_language(&cap.languages, &language),
            translate,
            vocabulary: if vocabulary { words } else { Vec::new() },
            filter_silence: options.speech_silence_filtering,
            vad_path: vad,
        },
        correct,
        if translate { "en".into() } else { language },
    )
}
fn finalize(text: &str, options: &SpeechTranscriptionOptions, language: &str) -> String {
    if options.speech_remove_filler_words {
        remove_filler_words(
            text,
            if language == "auto" {
                None
            } else {
                Some(language)
            },
            options.speech_correction_word.as_deref(),
        )
    } else {
        text.trim().into()
    }
}
pub struct SpeechStream {
    _activity: Activity,
    state: Option<OwnedMutexGuard<State>>,
    service: SpeechService,
    options: SpeechTranscriptionOptions,
    settings: SpeechSettings,
    definition: SpeechModel,
    vad: Option<PathBuf>,
    correct: TextCorrector,
    language: String,
    received: Vec<Vec<f32>>,
    bytes: usize,
    revision_base: i64,
    last_revision: i64,
    retried: bool,
    finished: bool,
    cancellation: CancellationToken,
}
impl SpeechStream {
    pub async fn close(mut self) {
        if let Some(mut state) = self.state.take() {
            if !self.finished
                && let Some(engine) = state.engine.as_mut()
                && !matches!(
                    tokio::time::timeout(Duration::from_millis(2500), engine.reset()).await,
                    Ok(Ok(()))
                )
            {
                state.engine = None;
            }
            drop(state);
            self.service.inner.schedule_unload();
        }
    }
    async fn recover(&mut self) -> Result<Option<StreamUpdate>, SpeechError> {
        let state = self.state.as_mut().ok_or(SpeechError::StreamNotReady)?;
        if self.retried
            || self.cancellation.is_cancelled()
            || self.settings.acceleration != "auto"
            || state
                .engine
                .as_ref()
                .is_none_or(|e| e.capabilities().backend == "cpu")
        {
            state.engine = None;
            return Err(SpeechError::Inference);
        }
        self.retried = true;
        state.engine = None;
        self.service
            .load(state, &self.definition, "cpu", self.cancellation.clone())
            .await?;
        state.auto_cpu = Some(self.definition.id.clone());
        let engine = state.engine.as_mut().ok_or(SpeechError::Inference)?;
        let (native, correct, language) = native_options(
            &self.definition,
            engine.capabilities(),
            &self.options,
            self.vad.clone(),
            true,
        );
        self.correct = correct;
        self.language = language;
        engine.begin(native, self.cancellation.clone()).await?;
        let mut update = None;
        for chunk in &self.received {
            update = Some(engine.feed(chunk, self.cancellation.clone()).await?);
        }
        Ok(update)
    }
    pub async fn feed(&mut self, bytes: &[u8]) -> Result<StreamUpdate, SpeechError> {
        if self.finished || self.cancellation.is_cancelled() {
            return Err(SpeechError::StreamNotReady);
        }
        if bytes.len() > MAX_CHUNK_BYTES
            || self.bytes.saturating_add(bytes.len()) > MAX_SPEECH_BYTES
        {
            return Err(SpeechError::InvalidAudio);
        }
        let pcm = decode_pcm(bytes)?;
        self.bytes += bytes.len();
        self.received.push(pcm);
        let engine = self
            .state
            .as_mut()
            .and_then(|s| s.engine.as_mut())
            .ok_or(SpeechError::StreamNotReady)?;
        let result = engine
            .feed(
                self.received.last().ok_or(SpeechError::InvalidAudio)?,
                self.cancellation.clone(),
            )
            .await;
        let update = match result {
            Ok(update) => Some(update),
            Err(_) => {
                let update = self.recover().await?;
                self.revision_base += self.last_revision;
                self.last_revision = 0;
                update
            }
        };
        if let Some(update) = &update {
            self.last_revision = update.revision;
        }
        Ok(StreamUpdate {
            revision: self.revision_base + self.last_revision,
            text: update.and_then(|u| u.text).map(|text| SpeechStreamText {
                committed: self.correct.apply(&text.committed),
                tentative: self.correct.apply(&text.tentative),
            }),
        })
    }
    pub async fn finish(&mut self) -> Result<String, SpeechError> {
        if self.finished || self.cancellation.is_cancelled() {
            return Err(SpeechError::StreamNotReady);
        }
        let result = self
            .state
            .as_mut()
            .and_then(|s| s.engine.as_mut())
            .ok_or(SpeechError::StreamNotReady)?
            .finish(self.cancellation.clone())
            .await;
        let result = match result {
            Ok(text) => Ok(text),
            Err(_) => {
                self.recover().await?;
                self.state
                    .as_mut()
                    .and_then(|s| s.engine.as_mut())
                    .ok_or(SpeechError::StreamNotReady)?
                    .finish(self.cancellation.clone())
                    .await
            }
        };
        self.finished = true;
        result.map(|text| finalize(&self.correct.apply(&text), &self.options, &self.language))
    }
}
impl Drop for SpeechStream {
    fn drop(&mut self) {
        let Some(mut state) = self.state.take() else {
            return;
        };
        let inner = self.service.inner.clone();
        let finished = self.finished;
        tokio::spawn(async move {
            if !finished
                && let Some(engine) = state.engine.as_mut()
                && !matches!(
                    tokio::time::timeout(Duration::from_millis(2500), engine.reset()).await,
                    Ok(Ok(()))
                )
            {
                state.engine = None;
            }
            drop(state);
            inner.schedule_unload();
        });
    }
}

pub struct SpeechProtocol {
    service: SpeechService,
    stream: Option<SpeechStream>,
    ended: bool,
    deadline: Instant,
    cancellation: CancellationToken,
}
impl SpeechProtocol {
    pub fn new(service: SpeechService, cancellation: CancellationToken) -> Self {
        Self {
            service,
            stream: None,
            ended: false,
            deadline: Instant::now() + Duration::from_secs(600),
            cancellation,
        }
    }
    pub fn is_ended(&self) -> bool {
        self.ended
    }
    pub fn deadline(&self) -> Instant {
        self.deadline
    }
    pub async fn receive(&mut self, frame: SpeechFrame<'_>) -> Option<SpeechStreamEvent> {
        if self.ended {
            return None;
        }
        if Instant::now() >= self.deadline {
            self.ended = true;
            self.stream = None;
            self.cancellation.cancel();
            return None;
        }
        let result = match frame {
            SpeechFrame::Text(command) => {
                if command.len() > MAX_OPTIONS_BYTES {
                    Err("Invalid speech stream command.")
                } else {
                    match serde_json::from_str::<SpeechStreamCommand>(command) {
                        Ok(SpeechStreamCommand::Start { options }) => {
                            if self.stream.is_some() {
                                Err("Speech stream already started.")
                            } else {
                                match self
                                    .service
                                    .start_stream(options, self.cancellation.child_token())
                                    .await
                                {
                                    Ok(stream) => {
                                        self.stream = Some(stream);
                                        Ok(SpeechStreamEvent::Ready)
                                    }
                                    Err(_) => Err(
                                        "Live transcription failed. Please try recording again.",
                                    ),
                                }
                            }
                        }
                        Ok(SpeechStreamCommand::Finish) => {
                            if let Some(stream) = self.stream.as_mut() {
                                match stream.finish().await {
                                    Ok(text) => {
                                        self.ended = true;
                                        Ok(SpeechStreamEvent::Finished { text })
                                    }
                                    Err(_) => Err(
                                        "Live transcription failed. Please try recording again.",
                                    ),
                                }
                            } else {
                                Err("Start speech transcription before finishing.")
                            }
                        }
                        Err(_) => Err("Live transcription failed. Please try recording again."),
                    }
                }
            }
            SpeechFrame::Binary(bytes) => {
                if bytes.len() > MAX_CHUNK_BYTES {
                    Err("Speech audio chunk is too large.")
                } else if let Some(stream) = self.stream.as_mut() {
                    match stream.feed(bytes).await {
                        Ok(update) => Ok(SpeechStreamEvent::Update {
                            revision: update.revision,
                            text: update.text,
                        }),
                        Err(_) => Err("Live transcription failed. Please try recording again."),
                    }
                } else {
                    Err("Start speech transcription before sending audio.")
                }
            }
        };
        Some(match result {
            Ok(event) => event,
            Err(message) => {
                self.ended = true;
                self.stream = None;
                SpeechStreamEvent::Error {
                    message: message.into(),
                }
            }
        })
    }
}
impl Drop for SpeechProtocol {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}
pub enum SpeechFrame<'a> {
    Text(&'a str),
    Binary(&'a [u8]),
}
