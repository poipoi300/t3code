use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use t3_contracts::speech::{
    SpeechModelUnloadTimeout, SpeechStreamEvent, SpeechStreamText, SpeechTranscriptionOptions,
};
use t3_textgen::{
    CancellationToken,
    models::{DEFAULT_SPEECH_MODEL_ID, ModelStore, SpeechModel},
    speech::*,
};
struct Settings(Mutex<SpeechSettings>);
#[async_trait]
impl SpeechSettingsBackend for Settings {
    async fn read(&self) -> Result<SpeechSettings, SpeechError> {
        Ok(self.0.lock().expect("settings").clone())
    }
    async fn write(&self, settings: SpeechSettings) -> Result<(), SpeechError> {
        *self.0.lock().expect("settings") = settings;
        Ok(())
    }
}
#[derive(Default)]
struct Calls {
    loads: Mutex<Vec<String>>,
    options: Mutex<Vec<NativeOptions>>,
    feeds: Mutex<Vec<usize>>,
    resets: AtomicUsize,
    gpu_fail: AtomicBool,
    hang: AtomicBool,
    started: tokio::sync::Notify,
    dropped: tokio::sync::Notify,
}
struct Factory(Arc<Calls>);
struct Engine {
    calls: Arc<Calls>,
    cap: EngineCapabilities,
    revision: i64,
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.calls.dropped.notify_one();
    }
}
#[async_trait]
impl SpeechEngineFactory for Factory {
    async fn load(
        &self,
        _: PathBuf,
        acceleration: String,
        _: CancellationToken,
    ) -> Result<Box<dyn LoadedSpeechEngine>, SpeechError> {
        self.0
            .loads
            .lock()
            .expect("loads")
            .push(acceleration.clone());
        Ok(Box::new(Engine {
            calls: self.0.clone(),
            cap: EngineCapabilities {
                backend: if acceleration == "auto" {
                    "vulkan".into()
                } else {
                    acceleration
                },
                languages: vec!["en-US".into(), "fr-FR".into()],
                streaming: false,
                long_form: true,
                translation: true,
                vocabulary: true,
                max_audio_ms: 0,
            },
            revision: 0,
        }))
    }
}
#[async_trait]
impl LoadedSpeechEngine for Engine {
    fn capabilities(&self) -> &EngineCapabilities {
        &self.cap
    }
    async fn transcribe(
        &mut self,
        _: &[f32],
        options: NativeOptions,
        _: CancellationToken,
    ) -> Result<String, SpeechError> {
        self.calls.options.lock().expect("options").push(options);
        if self.calls.hang.load(Ordering::SeqCst) {
            self.calls.started.notify_one();
            return std::future::pending().await;
        }
        if self.cap.backend != "cpu" && self.calls.gpu_fail.swap(false, Ordering::SeqCst) {
            return Err(SpeechError::Inference);
        }
        Ok("git hub and Robert".into())
    }
    async fn begin(
        &mut self,
        options: NativeOptions,
        _: CancellationToken,
    ) -> Result<(), SpeechError> {
        self.calls.options.lock().expect("options").push(options);
        Ok(())
    }
    async fn feed(
        &mut self,
        pcm: &[f32],
        _: CancellationToken,
    ) -> Result<StreamUpdate, SpeechError> {
        if self.cap.backend != "cpu" && self.calls.gpu_fail.swap(false, Ordering::SeqCst) {
            return Err(SpeechError::Inference);
        }
        self.calls.feeds.lock().expect("feeds").push(pcm.len());
        self.revision += 1;
        Ok(StreamUpdate {
            revision: self.revision,
            text: Some(SpeechStreamText {
                committed: "git hub".into(),
                tentative: "Robert".into(),
            }),
        })
    }
    async fn finish(&mut self, _: CancellationToken) -> Result<String, SpeechError> {
        Ok("git hub and Robert".into())
    }
    async fn reset(&mut self) -> Result<(), SpeechError> {
        self.calls.resets.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
struct Fixture {
    _root: tempfile::TempDir,
    service: SpeechService,
    calls: Arc<Calls>,
}
fn fixture() -> Fixture {
    let root = tempfile::tempdir().expect("temp");
    let store = Arc::new(
        ModelStore::new(root.path(), root.path().join("home"), "win32".into()).expect("store"),
    );
    let bytes = b"test-model";
    let model = SpeechModel {
        id: DEFAULT_SPEECH_MODEL_ID.into(),
        revision: "pinned".into(),
        name: "Test Model".into(),
        description: String::new(),
        filename: "test.gguf".into(),
        size: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(bytes)),
        license: "MIT".into(),
        languages: vec!["en".into(), "fr".into()],
        benchmark: serde_json::Value::Null,
        recommended: true,
        supports_streaming: false,
        supports_language_detection: true,
        supports_translation: true,
    };
    std::fs::write(store.path(&model).expect("path"), bytes).expect("model");
    let calls = Arc::new(Calls::default());
    let settings = Arc::new(Settings(Mutex::new(SpeechSettings::default())));
    let service = SpeechService::with_catalog(
        store,
        Arc::new(Factory(calls.clone())),
        settings,
        true,
        vec![model],
    );
    Fixture {
        _root: root,
        service,
        calls,
    }
}
fn options() -> SpeechTranscriptionOptions {
    serde_json::from_value(serde_json::json!({"speechLanguage":"fr","speechTranslateToEnglish":false,"speechRemoveFillerWords":true,"speechSilenceFiltering":false,"speechCustomWords":[{"term":"GitHub","aliases":[]},{"term":"Bob","aliases":["Robert"]}]})).expect("options")
}

#[tokio::test]
async fn batch_and_stream_honor_native_vocabulary_aliases_language_and_translation() {
    let f = fixture();
    let pcm = 0.25f32.to_le_bytes();
    assert_eq!(
        f.service
            .transcribe(&pcm, options(), CancellationToken::new())
            .await
            .expect("transcribe"),
        "git hub and Bob"
    );
    {
        let calls = f.calls.options.lock().expect("calls");
        assert_eq!(calls[0].language.as_deref(), Some("fr-FR"));
        assert_eq!(calls[0].vocabulary, vec!["GitHub", "Bob"]);
    }
    let mut stream = f
        .service
        .start_stream(options(), CancellationToken::new())
        .await
        .expect("stream");
    assert_eq!(
        f.service.select_model(DEFAULT_SPEECH_MODEL_ID).await,
        Err(SpeechError::Busy)
    );
    let update = stream.feed(&pcm).await.expect("feed");
    assert_eq!(update.text.expect("text").tentative, "Bob");
    assert_eq!(stream.finish().await.expect("finish"), "git hub and Bob");
    stream.close().await;
    let mut translated = options();
    translated.speech_translate_to_english = true;
    assert_eq!(
        f.service
            .transcribe(&pcm, translated, CancellationToken::new())
            .await
            .expect("translate"),
        "GitHub and Bob"
    );
    let calls = f.calls.options.lock().expect("calls");
    assert!(calls.last().expect("last").translate);
    assert!(calls.last().expect("last").vocabulary.is_empty());
}

#[tokio::test]
async fn auto_gpu_failure_replays_the_stream_once_on_cpu_and_preserves_revisions() {
    let f = fixture();
    let mut stream = f
        .service
        .start_stream(options(), CancellationToken::new())
        .await
        .expect("stream");
    let pcm = 0.25f32.to_le_bytes();
    assert_eq!(stream.feed(&pcm).await.expect("first").revision, 1);
    f.calls.gpu_fail.store(true, Ordering::SeqCst);
    assert_eq!(stream.feed(&pcm).await.expect("replayed").revision, 3);
    assert_eq!(f.calls.feeds.lock().expect("feeds").as_slice(), &[1, 1, 1]);
    assert_eq!(
        f.calls.loads.lock().expect("loads").as_slice(),
        &["auto", "cpu"]
    );
    stream.finish().await.expect("finish");
    stream.close().await;
    f.service
        .transcribe(&pcm, options(), CancellationToken::new())
        .await
        .expect("sticky cpu");
    assert_eq!(f.calls.loads.lock().expect("loads").len(), 2);
}

#[tokio::test(start_paused = true)]
async fn idle_unload_uses_the_deadline_and_cancelled_inference_drops_the_worker() {
    let f = fixture();
    f.service
        .update_unload_timeout(SpeechModelUnloadTimeout::Min2)
        .await
        .expect("timeout");
    f.service
        .transcribe(&0.25f32.to_le_bytes(), options(), CancellationToken::new())
        .await
        .expect("transcribe");
    tokio::time::advance(std::time::Duration::from_secs(120)).await;
    f.calls.dropped.notified().await;
    f.service
        .transcribe(&0.25f32.to_le_bytes(), options(), CancellationToken::new())
        .await
        .expect("reload");
    assert_eq!(f.calls.loads.lock().expect("loads").len(), 2);
    f.calls.hang.store(true, Ordering::SeqCst);
    let service = f.service.clone();
    let token = CancellationToken::new();
    let child = token.clone();
    let task = tokio::spawn(async move {
        service
            .transcribe(&0.25f32.to_le_bytes(), options(), child)
            .await
    });
    f.calls.started.notified().await;
    token.cancel();
    assert_eq!(task.await.expect("join"), Err(SpeechError::Cancelled));
    f.calls.hang.store(false, Ordering::SeqCst);
    f.service
        .transcribe(&0.25f32.to_le_bytes(), options(), CancellationToken::new())
        .await
        .expect("recovered");
    assert_eq!(f.calls.loads.lock().expect("loads").len(), 3);
}

#[tokio::test]
async fn stream_close_resets_and_protocol_enforces_start_audio_finish_order() {
    let f = fixture();
    let stream = f
        .service
        .start_stream(options(), CancellationToken::new())
        .await
        .expect("stream");
    stream.close().await;
    assert_eq!(f.calls.resets.load(Ordering::SeqCst), 1);
    let mut protocol = SpeechProtocol::new(f.service.clone(), CancellationToken::new());
    let command = serde_json::json!({"type":"start","options":options()}).to_string();
    assert!(matches!(
        protocol.receive(SpeechFrame::Text(&command)).await,
        Some(SpeechStreamEvent::Ready)
    ));
    for revision in 1..=2 {
        assert!(
            matches!(protocol.receive(SpeechFrame::Binary(&0.25f32.to_le_bytes())).await,Some(SpeechStreamEvent::Update{revision:n,..})if n==revision)
        );
    }
    assert!(matches!(
        protocol
            .receive(SpeechFrame::Text("{\"type\":\"finish\"}"))
            .await,
        Some(SpeechStreamEvent::Finished { .. })
    ));
    drop(protocol);
    let mut protocol = SpeechProtocol::new(f.service.clone(), CancellationToken::new());
    assert!(matches!(
        protocol.receive(SpeechFrame::Binary(&[0; 4])).await,
        Some(SpeechStreamEvent::Error { .. })
    ));
    assert!(protocol.is_ended());
}

#[cfg(feature = "native-speech")]
#[tokio::test]
async fn native_worker_runs_on_windows_without_a_model_and_sanitizes_load_failures() {
    let factory = t3_textgen::native::NativeSpeechFactory::new(PathBuf::from(env!(
        "CARGO_BIN_EXE_t3-speech-worker"
    )));
    let devices = factory
        .gpu_devices()
        .await
        .expect("native device discovery");
    assert!(devices.is_empty());
    let root = tempfile::tempdir().expect("temp");
    let result = factory
        .load(
            root.path().join("missing.gguf"),
            "cpu".into(),
            CancellationToken::new(),
        )
        .await;
    assert!(matches!(result, Err(SpeechError::Inference)));
}
