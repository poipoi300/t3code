//! Voice capture policy and bounded speech streaming; hosts perform audio and inference.
use crate::platform::PlatformError;
use std::{
    cell::RefCell,
    collections::{BTreeSet, VecDeque},
    rc::Rc,
};
use t3_contracts::speech::{
    CloudSpeechKeys, CloudSpeechProvider, EnvironmentSpeechPostProcessingRequest,
    SpeechAcceleration, SpeechModelUnloadTimeout,
};
use t3_contracts::speech::{
    SpeechStreamCommand, SpeechStreamEvent, SpeechStreamText, SpeechTranscriptionOptions,
};

pub enum VoiceOperation {
    Status,
    CloudKeys,
    CloudModels,
    StreamTicket,
    Models {
        benchmarks: bool,
    },
    SetCloudKey {
        provider: CloudSpeechProvider,
        key: Option<t3_contracts::settings_secret_value::SettingsSecretValue>,
    },
    DownloadModel(String),
    SelectModel(String),
    CancelDownload(String),
    RemoveModel(String),
    LinkModels(String),
    Acceleration(SpeechAcceleration),
    UnloadTimeout(SpeechModelUnloadTimeout),
    Transcribe {
        pcm: Vec<u8>,
        options: SpeechTranscriptionOptions,
    },
    CloudTranscribe {
        provider: CloudSpeechProvider,
        model: Option<String>,
        pcm: Vec<u8>,
        options: SpeechTranscriptionOptions,
    },
    PostProcess(EnvironmentSpeechPostProcessingRequest),
}
/// Auth is attached at execution by the destination environment's authorization service.
pub struct VoiceRequestPlan {
    pub method: &'static str,
    pub path: String,
    pub body: Vec<u8>,
    pub timeout_ms: u64,
    pub requires_consent: bool,
    pub reject_redirects: bool,
}
fn provider_name(provider: &CloudSpeechProvider) -> &'static str {
    match provider {
        CloudSpeechProvider::Openai => "openai",
        CloudSpeechProvider::Xai => "xai",
        CloudSpeechProvider::Openrouter => "openrouter",
    }
}
pub fn require_cloud_speech_key(keys: &CloudSpeechKeys, provider: &CloudSpeechProvider) -> bool {
    match provider {
        CloudSpeechProvider::Openai => keys.openai,
        CloudSpeechProvider::Xai => keys.xai,
        CloudSpeechProvider::Openrouter => keys.openrouter.unwrap_or(false),
    }
}
pub fn encode_speech_pcm(
    pcm: &[u8],
    options: &SpeechTranscriptionOptions,
) -> Result<Vec<u8>, PlatformError> {
    let metadata = serde_json::to_vec(options)
        .map_err(|_| PlatformError::InvalidData("Speech options".into()))?;
    if metadata.len() > 128_000 || pcm.len() > SPEECH_STREAM_MAX_QUEUED_BYTES {
        return Err(PlatformError::TooLarge);
    }
    let mut body = Vec::with_capacity(4 + metadata.len() + pcm.len());
    body.extend((metadata.len() as u32).to_le_bytes());
    body.extend(metadata);
    body.extend(pcm);
    Ok(body)
}
impl VoiceOperation {
    pub fn request(self, consent: bool) -> Result<VoiceRequestPlan, PlatformError> {
        use serde_json::json;
        let mut plan = VoiceRequestPlan {
            method: "POST",
            path: String::new(),
            body: vec![],
            timeout_ms: 30_000,
            requires_consent: false,
            reject_redirects: true,
        };
        let mut payload = None;
        match self {
            Self::Status => {
                plan.method = "GET";
                plan.path = "/api/voice/status".into();
            }
            Self::CloudKeys => {
                plan.method = "GET";
                plan.path = "/api/voice/cloud/keys".into();
            }
            Self::CloudModels => {
                plan.method = "GET";
                plan.path = "/api/voice/cloud/openrouter/models".into();
            }
            Self::StreamTicket => {
                plan.path = "/api/auth/websocket-ticket/media".into();
            }
            Self::Models { benchmarks } => {
                plan.method = "GET";
                plan.path = if benchmarks {
                    "/api/voice/models?benchmarks=1"
                } else {
                    "/api/voice/models"
                }
                .into();
            }
            Self::SetCloudKey { provider, key } => {
                plan.path = format!("/api/voice/cloud/{}/key", provider_name(&provider));
                payload = Some(json!({"key":key}));
            }
            Self::DownloadModel(model_id) => {
                plan.path = "/api/voice/models/download".into();
                plan.timeout_ms = 30 * 60_000;
                payload = Some(json!({"modelId":model_id}));
            }
            Self::SelectModel(model_id) => {
                plan.path = "/api/voice/models/select".into();
                payload = Some(json!({"modelId":model_id}));
            }
            Self::CancelDownload(model_id) => {
                plan.path = "/api/voice/models/cancel".into();
                payload = Some(json!({"modelId":model_id}));
            }
            Self::RemoveModel(model_id) => {
                plan.path = "/api/voice/models/remove".into();
                payload = Some(json!({"modelId":model_id}));
            }
            Self::LinkModels(path) => {
                plan.path = "/api/voice/models/link".into();
                plan.timeout_ms = 30 * 60_000;
                payload = Some(json!({"path":path}));
            }
            Self::Acceleration(acceleration) => {
                plan.path = "/api/voice/acceleration".into();
                payload = Some(json!({"acceleration":acceleration}));
            }
            Self::UnloadTimeout(timeout) => {
                plan.path = "/api/voice/model-unload-timeout".into();
                payload = Some(json!({"timeout":timeout}));
            }
            Self::Transcribe { pcm, options } => {
                plan.path = "/api/voice/transcribe".into();
                plan.requires_consent = true;
                plan.timeout_ms = 30 * 60_000;
                if !consent {
                    return Err(PlatformError::Denied("Voice consent was revoked.".into()));
                }
                plan.body = encode_speech_pcm(&pcm, &options)?;
            }
            Self::CloudTranscribe {
                provider,
                model,
                pcm,
                options,
            } => {
                plan.path = format!("/api/voice/cloud/{}/transcribe", provider_name(&provider));
                if let Some(model) = model {
                    plan.path
                        .push_str(&format!("?model={}", crate::device::form_encode(&model)));
                }
                plan.requires_consent = true;
                plan.timeout_ms = 130_000;
                if !consent {
                    return Err(PlatformError::Denied("Voice consent was revoked.".into()));
                }
                plan.body = encode_speech_pcm(&pcm, &options)?;
            }
            Self::PostProcess(request) => {
                plan.path = "/api/voice/post-process".into();
                plan.requires_consent = true;
                plan.timeout_ms = 30 * 60_000;
                if !consent {
                    return Err(PlatformError::Denied("Voice consent was revoked.".into()));
                }
                payload = Some(
                    serde_json::to_value(request)
                        .map_err(|_| PlatformError::InvalidData("Speech request".into()))?,
                );
            }
        }
        if let Some(payload) = payload {
            plan.body = serde_json::to_vec(&payload)
                .map_err(|_| PlatformError::InvalidData("Speech request".into()))?;
        }
        Ok(plan)
    }
}

pub const VOICE_RECORDING_LIMIT_SECONDS: u64 = 300;
pub const SPEECH_STREAM_MAX_CHUNK_BYTES: usize = 16_000 * 4;
pub const SPEECH_STREAM_MAX_QUEUED_BYTES: usize = SPEECH_STREAM_MAX_CHUNK_BYTES * 300;

pub type VoiceState = VoiceInputState;

#[derive(Debug, Clone, PartialEq)]
pub enum SpeechAction {
    Send(SpeechStreamCommand),
    Audio(Vec<u8>),
    Text(SpeechStreamText),
    Finished(String),
    Failed(String),
    Close,
}
pub struct SpeechStream {
    queue: VecDeque<Vec<u8>>,
    queued_bytes: usize,
    in_flight: VecDeque<usize>,
    in_flight_bytes: usize,
    ready: bool,
    finishing: bool,
    finish_sent: bool,
    closed: bool,
    revision: i64,
    deadline: Option<u64>,
    pub result: Option<Result<String, String>>,
}
impl SpeechStream {
    pub fn new(now: u64) -> Self {
        Self {
            queue: VecDeque::new(),
            queued_bytes: 0,
            in_flight: VecDeque::new(),
            in_flight_bytes: 0,
            ready: false,
            finishing: false,
            finish_sent: false,
            closed: false,
            revision: -1,
            deadline: Some(now.saturating_add(120_000)),
            result: None,
        }
    }
    fn arm(&mut self, now: u64) {
        self.deadline = Some(now.saturating_add(120_000));
    }
    pub fn opened(
        &mut self,
        options: SpeechTranscriptionOptions,
        consent: bool,
    ) -> Vec<SpeechAction> {
        if self.closed {
            return vec![];
        }
        if !consent {
            return self.fail("Voice consent was revoked.", true);
        }
        vec![SpeechAction::Send(SpeechStreamCommand::Start { options })]
    }
    fn pump(&mut self, consent: bool, now: u64) -> Vec<SpeechAction> {
        if !self.ready || self.closed || self.finish_sent {
            return vec![];
        }
        if !consent {
            return self.fail("Voice consent was revoked.", true);
        }
        let mut actions = vec![];
        while self.in_flight.len() < 2 {
            let Some(chunk) = self.queue.pop_front() else {
                break;
            };
            self.queued_bytes -= chunk.len();
            self.in_flight_bytes += chunk.len();
            self.in_flight.push_back(chunk.len());
            actions.push(SpeechAction::Audio(chunk));
            self.arm(now);
        }
        if self.finishing && self.queue.is_empty() && self.in_flight.is_empty() {
            self.finish_sent = true;
            actions.push(SpeechAction::Send(SpeechStreamCommand::Finish));
            self.arm(now);
        }
        actions
    }
    pub fn feed(&mut self, pcm: &[f32], consent: bool, now: u64) -> Vec<SpeechAction> {
        if self.closed || self.finishing {
            return vec![];
        }
        let Some(length) = pcm.len().checked_mul(4) else {
            return self.fail("Voice recordings are limited to five minutes.", true);
        };
        if length
            > SPEECH_STREAM_MAX_QUEUED_BYTES
                .saturating_sub(self.queued_bytes + self.in_flight_bytes)
        {
            return self.fail("Voice recordings are limited to five minutes.", true);
        }
        for sample in pcm {
            if self
                .queue
                .back()
                .is_none_or(|chunk| chunk.len() == SPEECH_STREAM_MAX_CHUNK_BYTES)
            {
                self.queue.push_back(Vec::new());
            }
            if let Some(chunk) = self.queue.back_mut() {
                chunk.extend_from_slice(&sample.to_le_bytes());
                self.queued_bytes += 4;
            }
        }
        self.pump(consent, now)
    }
    pub fn finish(&mut self, consent: bool, now: u64) -> Vec<SpeechAction> {
        self.finishing = true;
        self.pump(consent, now)
    }
    pub fn receive_json(&mut self, text: &str, consent: bool, now: u64) -> Vec<SpeechAction> {
        match serde_json::from_str(text) {
            Ok(event) => self.receive(event, consent, now),
            Err(_) => self.fail("Invalid speech stream response.", true),
        }
    }
    pub fn receive(
        &mut self,
        event: SpeechStreamEvent,
        consent: bool,
        now: u64,
    ) -> Vec<SpeechAction> {
        if self.closed {
            return vec![];
        }
        if !consent {
            return self.fail("Voice consent was revoked.", true);
        }
        match event {
            SpeechStreamEvent::Ready => {
                if self.ready {
                    return self.fail("Unexpected speech stream response.", true);
                }
                self.ready = true;
                self.deadline = None;
                self.pump(consent, now)
            }
            SpeechStreamEvent::Update { revision, text } => {
                if self.in_flight.is_empty() || revision < self.revision {
                    return self.fail("Unexpected speech stream response.", true);
                }
                self.revision = revision;
                if let Some(length) = self.in_flight.pop_front() {
                    self.in_flight_bytes -= length;
                }
                self.deadline = None;
                let mut actions = text.map(SpeechAction::Text).into_iter().collect::<Vec<_>>();
                actions.extend(self.pump(consent, now));
                if !self.in_flight.is_empty() {
                    self.arm(now);
                }
                actions
            }
            SpeechStreamEvent::Finished { text } => {
                if !self.finish_sent {
                    return self.fail("Unexpected speech stream response.", true);
                }
                self.closed = true;
                self.cleanup();
                self.result = Some(Ok(text.clone()));
                vec![SpeechAction::Finished(text), SpeechAction::Close]
            }
            SpeechStreamEvent::Error { message } => self.fail(&message, true),
        }
    }
    fn cleanup(&mut self) {
        self.queue.clear();
        self.queued_bytes = 0;
        self.in_flight.clear();
        self.in_flight_bytes = 0;
        self.deadline = None;
    }
    pub fn fail(&mut self, message: &str, notify: bool) -> Vec<SpeechAction> {
        if self.closed {
            return vec![];
        }
        self.closed = true;
        self.cleanup();
        self.result = Some(Err(message.into()));
        let mut actions = vec![SpeechAction::Close];
        if notify {
            actions.push(SpeechAction::Failed(message.into()));
        }
        actions
    }
    pub fn cancel(&mut self) -> Vec<SpeechAction> {
        self.fail("Voice transcription was cancelled.", false)
    }
    pub fn disconnected(&mut self) -> Vec<SpeechAction> {
        self.fail("The voice connection closed. Please record again.", true)
    }
    pub fn tick(&mut self, now: u64) -> Vec<SpeechAction> {
        if self.deadline.is_some_and(|at| now >= at) {
            self.fail("Live transcription stopped responding.", true)
        } else {
            vec![]
        }
    }
    pub fn buffered_bytes(&self) -> usize {
        self.queued_bytes + self.in_flight_bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub start: usize,
    pub end: usize,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceDraft {
    pub owner_key: String,
    pub text: String,
    pub selection: Selection,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptCommit {
    Stale,
    Empty,
    Commit { text: String, selection: Selection },
}
fn map_position(before: &[u16], text: &[u16], position: usize, suffix_first: bool) -> usize {
    let shared = before.len().min(text.len());
    let (mut prefix, mut suffix) = (0, 0);
    let scan = |suffix: &mut usize, limit: usize| {
        while *suffix < limit
            && before[before.len() - 1 - *suffix] == text[text.len() - 1 - *suffix]
        {
            *suffix += 1;
        }
        if *suffix > 0 && (0xdc00..=0xdfff).contains(&before[before.len() - *suffix]) {
            *suffix -= 1;
        }
    };
    if suffix_first {
        scan(&mut suffix, shared);
    }
    while prefix < shared - suffix && before[prefix] == text[prefix] {
        prefix += 1;
    }
    if prefix > 0 && (0xd800..=0xdbff).contains(&before[prefix - 1]) {
        prefix -= 1;
    }
    if !suffix_first {
        scan(&mut suffix, shared - prefix);
    }
    if position < prefix {
        position
    } else if position >= before.len() - suffix {
        text.len()
            .saturating_sub(before.len() - position.min(before.len()))
    } else {
        text.len() - suffix
    }
}
fn letter_mark_number(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(c as u32,0x0300..=0x036f|0x1ab0..=0x1aff|0x1dc0..=0x1dff|0x20d0..=0x20ff|0xfe20..=0xfe2f)
}
fn latin_number(c: char) -> bool {
    c.is_numeric()
        || c.is_ascii_alphabetic()
        || matches!(c as u32,0xc0..=0x24f|0x1e00..=0x1eff|0xab30..=0xab6f)
}
pub fn resolve_transcript_commit(
    captured: &VoiceDraft,
    current: Option<&VoiceDraft>,
    transcript: &str,
    locale: &str,
) -> TranscriptCommit {
    let Some(current) = current.filter(|d| d.owner_key == captured.owner_key) else {
        return TranscriptCommit::Stale;
    };
    let replacement = transcript.trim();
    if replacement.is_empty() {
        return TranscriptCommit::Empty;
    }
    let before = captured.text.encode_utf16().collect::<Vec<_>>();
    let text = current.text.encode_utf16().collect::<Vec<_>>();
    let unchanged = captured.text == current.text;
    let end = if unchanged {
        captured.selection.end.min(text.len())
    } else {
        map_position(&before, &text, captured.selection.end, false).max(map_position(
            &before,
            &text,
            captured.selection.end,
            true,
        ))
    };
    let start = if unchanged {
        captured.selection.start.min(end)
    } else {
        end
    };
    let language = locale.split(['-', '_']).next().unwrap_or("").to_lowercase();
    let mut insertion = replacement.to_owned();
    if start == end && !["ja", "zh", "yue", "th", "lo", "km", "my"].contains(&language.as_str()) {
        let auto = language == "auto";
        let left = String::from_utf16_lossy(&text[..start]);
        let right = String::from_utf16_lossy(&text[start..]);
        let l = left.chars().next_back();
        let r = right.chars().next();
        let left_edge = l.is_some_and(|c| {
            (if auto {
                c.is_ascii_alphanumeric()
            } else {
                letter_mark_number(c)
            }) || ".!?,:;)]}'\"".contains(c)
        });
        let right_edge = r.is_some_and(|c| {
            (if auto {
                c.is_ascii_alphanumeric()
            } else {
                letter_mark_number(c)
            }) || "([{'\"".contains(c)
        });
        let prefix = (!auto || replacement.chars().next().is_some_and(latin_number))
            && left_edge
            && r.is_none_or(char::is_whitespace);
        let suffix = (!auto || replacement.chars().next_back().is_some_and(latin_number))
            && right_edge
            && l.is_none_or(char::is_whitespace);
        insertion = format!(
            "{}{}{}",
            if prefix { " " } else { "" },
            replacement,
            if suffix { " " } else { "" }
        );
    }
    let cursor = start + insertion.encode_utf16().count();
    let mut result = text[..start].to_vec();
    result.extend(insertion.encode_utf16());
    result.extend_from_slice(&text[end..]);
    TranscriptCommit::Commit {
        text: String::from_utf16_lossy(&result),
        selection: Selection {
            start: cursor,
            end: cursor,
        },
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum VoicePhase {
    #[default]
    Idle,
    Preparing,
    Recording,
    Transcribing,
    PostProcessing,
    Error,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceErrorAction {
    Retry,
    Settings,
}
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct VoiceInputState {
    pub phase: VoicePhase,
    pub error: Option<String>,
    pub error_action: Option<VoiceErrorAction>,
}
impl VoiceInputState {
    pub fn blocks_submission(&self) -> bool {
        matches!(
            self.phase,
            VoicePhase::Preparing
                | VoicePhase::Recording
                | VoicePhase::Transcribing
                | VoicePhase::PostProcessing
        )
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceEffect {
    WaitForRelease,
    RequestPermission,
    PrepareTranscription,
    ConfigureRecording,
    PrepareRecorder,
    Record { seconds: u64 },
    StartLive(String),
    AwaitLive,
    TranscriptAvailable(String),
    StopRecorder,
    ReleaseAudio,
    Transcribe(Option<String>),
    FinishStreaming,
    PostProcess(String),
    AbortPostProcessing,
    PostProcessingFailed(String),
    AbortTranscription,
    DeleteRecording(String),
    Commit { text: String, selection: Selection },
}
#[derive(Default)]
pub struct VoiceArbiter {
    next: u64,
    active: Option<(u64, bool)>,
    inference: Option<u64>,
}
impl VoiceArbiter {
    fn acquire(&mut self) -> Option<u64> {
        if self.active.is_some() {
            return None;
        }
        self.next = self.next.wrapping_add(1);
        self.active = Some((self.next, false));
        Some(self.next)
    }
    pub fn inference_started(&mut self, lease: u64) -> bool {
        if self.inference.is_some() {
            return false;
        }
        self.inference = Some(lease);
        true
    }
    pub fn inference_settled(&mut self, lease: u64) {
        if self.inference == Some(lease) {
            self.inference = None;
        }
    }
    fn release(&mut self, lease: u64) -> bool {
        if self.inference == Some(lease) {
            return false;
        }
        if self.active.is_some_and(|a| a.0 == lease) {
            self.active = None;
        }
        true
    }
}
pub struct VoiceController {
    pub state: VoiceInputState,
    arbiter: Rc<RefCell<VoiceArbiter>>,
    generation: u64,
    lease: Option<u64>,
    captured: Option<VoiceDraft>,
    initiating: Option<VoiceDraft>,
    locale: String,
    streaming: bool,
    live: bool,
    live_result: Option<Option<String>>,
    inference_generation: Option<u64>,
    uri: Option<String>,
    owned: BTreeSet<String>,
    retry: Option<(String, String)>,
    retry_failed: bool,
    configured: bool,
    finishing: bool,
    raw: Option<String>,
    cleanup_pending: bool,
}
impl VoiceController {
    pub fn new(arbiter: Rc<RefCell<VoiceArbiter>>, retry_failed: bool) -> Self {
        Self {
            state: VoiceInputState::default(),
            arbiter,
            generation: 0,
            lease: None,
            captured: None,
            initiating: None,
            locale: String::new(),
            streaming: false,
            live: false,
            live_result: None,
            inference_generation: None,
            uri: None,
            owned: BTreeSet::new(),
            retry: None,
            retry_failed,
            configured: false,
            finishing: false,
            raw: None,
            cleanup_pending: false,
        }
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn lease(&self) -> Option<u64> {
        self.lease
    }
    pub fn accepts(&self, generation: u64) -> bool {
        self.current(generation)
    }
    fn begin_inference(&mut self, effect: VoiceEffect) -> Vec<VoiceEffect> {
        let acquired = self
            .lease
            .is_some_and(|lease| self.arbiter.borrow_mut().inference_started(lease));
        if !acquired {
            self.error(
                "Voice transcription is still finishing. Try again shortly.",
                Some(VoiceErrorAction::Retry),
            );
            return self.cleanup();
        }
        self.inference_generation = Some(self.generation);
        vec![effect]
    }
    fn settle_inference(&mut self, generation: u64) {
        if self.inference_generation == Some(generation) {
            if let Some(lease) = self.lease {
                self.arbiter.borrow_mut().inference_settled(lease);
            }
            self.inference_generation = None;
        }
    }
    fn current(&self, generation: u64) -> bool {
        self.generation == generation
            && self.state.phase != VoicePhase::Idle
            && !self.cleanup_pending
    }
    fn phase(&mut self, phase: VoicePhase) {
        self.state = VoiceInputState {
            phase,
            error: None,
            error_action: None,
        };
    }
    fn error(&mut self, message: &str, action: Option<VoiceErrorAction>) {
        self.state = VoiceInputState {
            phase: VoicePhase::Error,
            error: Some(message.into()),
            error_action: action,
        };
    }
    pub fn start(&mut self, draft: Option<VoiceDraft>, supported: bool) -> Vec<VoiceEffect> {
        if !matches!(self.state.phase, VoicePhase::Idle | VoicePhase::Error) {
            return vec![];
        }
        let Some(draft) = draft else {
            self.error(
                "This draft is no longer available.",
                Some(VoiceErrorAction::Retry),
            );
            return vec![];
        };
        self.generation = self.generation.wrapping_add(1);
        self.initiating = Some(draft);
        self.phase(VoicePhase::Preparing);
        if self.cleanup_pending || self.arbiter.borrow().active.is_some_and(|a| a.1) {
            return vec![VoiceEffect::WaitForRelease];
        }
        self.acquire(supported)
    }
    pub fn resume_waiter(&mut self, generation: u64, supported: bool) -> Vec<VoiceEffect> {
        if generation != self.generation
            || self.state.phase != VoicePhase::Preparing
            || self.cleanup_pending
        {
            return vec![];
        }
        self.acquire(supported)
    }
    fn acquire(&mut self, supported: bool) -> Vec<VoiceEffect> {
        let lease = self.arbiter.borrow_mut().acquire();
        let Some(lease) = lease else {
            self.error(
                "Another voice recording is already active.",
                Some(VoiceErrorAction::Retry),
            );
            return vec![];
        };
        self.lease = Some(lease);
        let owner = self.initiating.as_ref().map(|d| d.owner_key.as_str());
        if self
            .retry
            .as_ref()
            .is_some_and(|r| Some(r.1.as_str()) != owner)
        {
            self.retry = None;
        }
        if let Some((uri, _)) = self.retry.take() {
            self.uri = Some(uri);
            self.captured = self.initiating.clone();
            self.finishing = true;
            self.phase(VoicePhase::Transcribing);
            return self.begin_inference(VoiceEffect::Transcribe(self.uri.clone()));
        }
        if !supported {
            self.error("Voice transcription is not available.", None);
            return self.cleanup();
        }
        let mut effects = self.delete_recordings();
        effects.push(VoiceEffect::RequestPermission);
        effects
    }
    pub fn permission(
        &mut self,
        generation: u64,
        granted: bool,
        can_ask_again: bool,
    ) -> Vec<VoiceEffect> {
        if !self.current(generation) {
            return vec![];
        }
        if !granted {
            self.error(
                "Microphone access is required for voice input.",
                Some(if can_ask_again {
                    VoiceErrorAction::Retry
                } else {
                    VoiceErrorAction::Settings
                }),
            );
            return self.cleanup();
        }
        self.begin_inference(VoiceEffect::PrepareTranscription)
    }
    pub fn prepared(
        &mut self,
        generation: u64,
        locale: String,
        streaming: bool,
        live: bool,
    ) -> Vec<VoiceEffect> {
        self.settle_inference(generation);
        if !self.current(generation) {
            return vec![];
        }
        self.locale = locale;
        self.streaming = streaming;
        self.live = live;
        self.live_result = None;
        vec![VoiceEffect::ConfigureRecording]
    }
    pub fn configured(&mut self, generation: u64) -> Vec<VoiceEffect> {
        if generation != self.generation || self.cleanup_pending {
            return vec![VoiceEffect::ReleaseAudio];
        }
        self.configured = true;
        vec![VoiceEffect::PrepareRecorder]
    }
    pub fn recorder_prepared(
        &mut self,
        generation: u64,
        uri: Option<String>,
        draft: Option<VoiceDraft>,
    ) -> Vec<VoiceEffect> {
        if !self.current(generation) {
            return vec![];
        }
        self.remember(uri.clone());
        self.uri = uri;
        if draft.as_ref().map(|d| &d.owner_key) != self.initiating.as_ref().map(|d| &d.owner_key)
            || draft.is_none()
        {
            self.error(
                "This draft is no longer available.",
                Some(VoiceErrorAction::Retry),
            );
            return self.cleanup();
        }
        self.captured = draft;
        self.phase(VoicePhase::Recording);
        let mut effects = vec![];
        if self.live
            && let Some(uri) = &self.uri
        {
            let uri = uri.clone();
            effects.extend(self.begin_inference(VoiceEffect::StartLive(uri)));
            if self.cleanup_pending {
                return effects;
            }
        }
        effects.push(VoiceEffect::Record {
            seconds: VOICE_RECORDING_LIMIT_SECONDS,
        });
        effects
    }
    pub fn stop(
        &mut self,
        already_stopped: bool,
        completed_uri: Option<String>,
    ) -> Vec<VoiceEffect> {
        if self.state.phase != VoicePhase::Recording || self.finishing {
            return vec![];
        }
        self.finishing = true;
        if completed_uri.is_some() {
            self.remember(completed_uri.clone());
            self.uri = completed_uri;
        }
        self.phase(VoicePhase::Transcribing);
        let mut effects = vec![];
        if !already_stopped {
            effects.push(VoiceEffect::StopRecorder);
        }
        effects.push(VoiceEffect::ReleaseAudio);
        effects
    }
    /// The host calls this only after capture is flushed and the audio session released.
    pub fn audio_released(&mut self, generation: u64) -> Vec<VoiceEffect> {
        if !self.current(generation) || self.state.phase != VoicePhase::Transcribing {
            return vec![];
        }
        self.configured = false;
        self.final_transcription()
    }
    fn final_transcription(&mut self) -> Vec<VoiceEffect> {
        if self.live && self.uri.is_some() {
            match &self.live_result {
                None => return vec![VoiceEffect::AwaitLive],
                Some(Some(text)) => return vec![VoiceEffect::TranscriptAvailable(text.clone())],
                Some(None) => {}
            }
        }
        if self.streaming {
            self.begin_inference(VoiceEffect::FinishStreaming)
        } else if self.uri.is_some() {
            self.begin_inference(VoiceEffect::Transcribe(self.uri.clone()))
        } else {
            self.error(
                "Could not finish voice recording.",
                Some(VoiceErrorAction::Retry),
            );
            self.cleanup()
        }
    }
    pub fn live_settled(
        &mut self,
        generation: u64,
        result: Result<Option<String>, String>,
    ) -> Vec<VoiceEffect> {
        self.settle_inference(generation);
        if !self.current(generation) || self.live_result.is_some() {
            return vec![];
        }
        match result {
            Err(message) => {
                self.error(&message, Some(VoiceErrorAction::Retry));
                self.cleanup()
            }
            Ok(text) => {
                self.live_result = Some(text);
                if self.state.phase == VoicePhase::Transcribing && !self.configured {
                    self.final_transcription()
                } else {
                    vec![]
                }
            }
        }
    }
    pub fn transcription_failed(&mut self, generation: u64, message: &str) -> Vec<VoiceEffect> {
        self.settle_inference(generation);
        if !self.current(generation) {
            return vec![];
        }
        if self.retry_failed
            && !self.live
            && !self.streaming
            && let (Some(uri), Some(draft)) = (&self.uri, &self.captured)
        {
            self.retry = Some((uri.clone(), draft.owner_key.clone()));
        }
        self.error(message, Some(VoiceErrorAction::Retry));
        self.cleanup()
    }
    pub fn transcribed(
        &mut self,
        generation: u64,
        text: String,
        post_process: bool,
        current: Option<&VoiceDraft>,
    ) -> Vec<VoiceEffect> {
        self.settle_inference(generation);
        if !self.current(generation) || self.state.phase != VoicePhase::Transcribing {
            return vec![];
        }
        if post_process && !text.trim().is_empty() {
            self.raw = Some(text.clone());
            self.phase(VoicePhase::PostProcessing);
            return vec![VoiceEffect::PostProcess(text)];
        }
        self.commit(&text, current)
    }
    pub fn post_processed(
        &mut self,
        generation: u64,
        result: Result<String, String>,
        current: Option<&VoiceDraft>,
    ) -> Vec<VoiceEffect> {
        if !self.current(generation) || self.state.phase != VoicePhase::PostProcessing {
            return vec![];
        }
        match result {
            Ok(text) => self.commit(&text, current),
            Err(error) => {
                let raw = self.raw.clone().unwrap_or_default();
                let mut effects = vec![VoiceEffect::PostProcessingFailed(error)];
                effects.extend(self.commit(&raw, current));
                effects
            }
        }
    }
    pub fn skip_post_processing(&mut self, current: Option<&VoiceDraft>) -> Vec<VoiceEffect> {
        if self.state.phase != VoicePhase::PostProcessing {
            return vec![];
        }
        let raw = self.raw.clone().unwrap_or_default();
        let mut effects = vec![VoiceEffect::AbortPostProcessing];
        effects.extend(self.commit(&raw, current));
        effects
    }
    fn commit(&mut self, text: &str, current: Option<&VoiceDraft>) -> Vec<VoiceEffect> {
        let result = self
            .captured
            .as_ref()
            .map_or(TranscriptCommit::Stale, |captured| {
                resolve_transcript_commit(captured, current, text, &self.locale)
            });
        let mut effects = vec![];
        match result {
            TranscriptCommit::Stale => self.error(
                "The draft changed while voice input was running. The transcript was not added.",
                Some(VoiceErrorAction::Retry),
            ),
            TranscriptCommit::Empty => {
                self.error("No speech was detected.", Some(VoiceErrorAction::Retry))
            }
            TranscriptCommit::Commit { text, selection } => {
                effects.push(VoiceEffect::Commit { text, selection });
                self.phase(VoicePhase::Idle);
            }
        }
        effects.extend(self.cleanup());
        effects
    }
    pub fn cancel(&mut self) -> Vec<VoiceEffect> {
        if self.state.phase == VoicePhase::Idle && !self.cleanup_pending {
            return vec![];
        }
        self.generation = self.generation.wrapping_add(1);
        self.retry = None;
        self.phase(VoicePhase::Idle);
        self.cleanup()
    }
    pub fn interrupt(&mut self, message: &str) -> Vec<VoiceEffect> {
        if !matches!(
            self.state.phase,
            VoicePhase::Preparing | VoicePhase::Recording
        ) {
            return vec![];
        }
        self.generation = self.generation.wrapping_add(1);
        self.error(message, Some(VoiceErrorAction::Retry));
        self.cleanup()
    }
    pub fn backgrounded(&mut self) -> Vec<VoiceEffect> {
        let message = if self.state.phase == VoicePhase::Preparing {
            "Voice input stopped when the app moved to the background."
        } else {
            "Voice recording was interrupted."
        };
        self.interrupt(message)
    }
    pub fn recorder_status(
        &mut self,
        uri: Option<String>,
        finished: bool,
        error: Option<&str>,
    ) -> Vec<VoiceEffect> {
        if self.state.phase != VoicePhase::Recording
            || (uri.is_some() && self.uri.is_some() && uri != self.uri)
        {
            return vec![];
        }
        if let Some(error) = error {
            return self.interrupt(error);
        }
        if finished {
            if uri.is_none() {
                return self.interrupt("Voice recording was interrupted.");
            }
            return self.stop(true, uri);
        }
        vec![]
    }
    fn remember(&mut self, uri: Option<String>) {
        if let Some(uri) = uri {
            self.owned.insert(uri);
        }
    }
    fn delete_recordings(&mut self) -> Vec<VoiceEffect> {
        let keep = self.retry.as_ref().map(|r| &r.0);
        let mut effects = vec![];
        self.owned.retain(|uri| {
            if Some(uri) == keep {
                true
            } else {
                effects.push(VoiceEffect::DeleteRecording(uri.clone()));
                false
            }
        });
        effects
    }
    fn cleanup(&mut self) -> Vec<VoiceEffect> {
        if self.lease.is_none() {
            self.cleanup_pending = false;
            return self.delete_recordings();
        }
        if self.cleanup_pending {
            return vec![];
        }
        self.cleanup_pending = true;
        if let Some(lease) = self.lease {
            let mut arbiter = self.arbiter.borrow_mut();
            if let Some(active) = arbiter.active.as_mut().filter(|a| a.0 == lease) {
                active.1 = true;
            }
        }
        let mut effects = vec![
            VoiceEffect::AbortTranscription,
            VoiceEffect::AbortPostProcessing,
        ];
        if self.configured {
            effects.push(VoiceEffect::StopRecorder);
            effects.push(VoiceEffect::ReleaseAudio);
        }
        effects
    }
    /// Inference must settle before file deletion or releasing the global capture lease.
    pub fn resources_released(&mut self, lease: u64) -> Vec<VoiceEffect> {
        if self.lease != Some(lease)
            || !self.cleanup_pending
            || !self.arbiter.borrow_mut().release(lease)
        {
            return vec![];
        }
        self.lease = None;
        self.cleanup_pending = false;
        self.configured = false;
        self.finishing = false;
        self.uri = None;
        self.captured = None;
        self.raw = None;
        self.live_result = None;
        self.inference_generation = None;
        self.delete_recordings()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn draft(text: &str, start: usize, end: usize) -> VoiceDraft {
        VoiceDraft {
            owner_key: "owner".into(),
            text: text.into(),
            selection: Selection { start, end },
        }
    }
    fn controller() -> VoiceController {
        VoiceController::new(Rc::new(RefCell::new(VoiceArbiter::default())), true)
    }
    fn record(c: &mut VoiceController, streaming: bool, uri: Option<String>) -> u64 {
        assert_eq!(
            c.start(Some(draft("hello", 5, 5)), true),
            vec![VoiceEffect::RequestPermission]
        );
        let g = c.generation();
        assert_eq!(
            c.permission(g, true, true),
            vec![VoiceEffect::PrepareTranscription]
        );
        assert_eq!(
            c.prepared(g, "en-US".into(), streaming, false),
            vec![VoiceEffect::ConfigureRecording]
        );
        assert_eq!(c.configured(g), vec![VoiceEffect::PrepareRecorder]);
        assert_eq!(
            c.recorder_prepared(g, uri, Some(draft("hello", 5, 5))),
            vec![VoiceEffect::Record { seconds: 300 }]
        );
        g
    }
    fn stop(c: &mut VoiceController, g: u64, streaming: bool) {
        assert_eq!(
            c.stop(false, None),
            vec![VoiceEffect::StopRecorder, VoiceEffect::ReleaseAudio]
        );
        assert_eq!(
            c.audio_released(g),
            vec![if streaming {
                VoiceEffect::FinishStreaming
            } else {
                VoiceEffect::Transcribe(Some("audio".into()))
            }]
        );
    }
    #[test]
    fn stream_drains_two_acknowledged_chunks_before_finish_and_replaces_text() {
        let mut s = SpeechStream::new(0);
        assert!(s.feed(&[0.1], true, 0).is_empty());
        let first = s.receive(SpeechStreamEvent::Ready, true, 1);
        assert!(matches!(first.as_slice(), [SpeechAction::Audio(_)]));
        assert!(matches!(
            s.feed(&[0.2], true, 2).as_slice(),
            [SpeechAction::Audio(_)]
        ));
        assert!(s.feed(&[0.3], true, 3).is_empty());
        assert!(s.finish(true, 4).is_empty());
        let text = SpeechStreamText {
            committed: "hello ".into(),
            tentative: "were".into(),
        };
        let actions = s.receive(
            SpeechStreamEvent::Update {
                revision: 1,
                text: Some(text.clone()),
            },
            true,
            5,
        );
        assert_eq!(actions[0], SpeechAction::Text(text));
        assert!(matches!(actions[1], SpeechAction::Audio(_)));
        assert!(
            s.receive(
                SpeechStreamEvent::Update {
                    revision: 2,
                    text: None
                },
                true,
                6
            )
            .is_empty()
        );
        assert_eq!(
            s.receive(
                SpeechStreamEvent::Update {
                    revision: 3,
                    text: None
                },
                true,
                7
            ),
            vec![SpeechAction::Send(SpeechStreamCommand::Finish)]
        );
        assert_eq!(
            s.receive(
                SpeechStreamEvent::Finished {
                    text: "hello world".into()
                },
                true,
                8
            ),
            vec![
                SpeechAction::Finished("hello world".into()),
                SpeechAction::Close
            ]
        );
        assert_eq!(s.result, Some(Ok("hello world".into())));
    }
    #[test]
    fn voice_request_plans_preserve_endpoints_timeouts_and_consent() {
        let model = VoiceOperation::DownloadModel("model".into())
            .request(false)
            .expect("plan");
        assert_eq!(model.path, "/api/voice/models/download");
        assert_eq!(model.timeout_ms, 1_800_000);
        assert!(model.reject_redirects);
        assert_eq!(
            VoiceOperation::Models { benchmarks: true }
                .request(false)
                .expect("plan")
                .path,
            "/api/voice/models?benchmarks=1"
        );
        assert_eq!(
            VoiceOperation::StreamTicket
                .request(false)
                .expect("plan")
                .path,
            "/api/auth/websocket-ticket/media"
        );
        let options: SpeechTranscriptionOptions =
            serde_json::from_value(serde_json::json!({"speechLanguage":"auto","speechTranslateToEnglish":false,"speechCustomWords":[],"speechRemoveFillerWords":true,"speechSilenceFiltering":true}))
                .expect("options");
        assert!(matches!(
            VoiceOperation::Transcribe {
                pcm: vec![0; 4],
                options: options.clone()
            }
            .request(false),
            Err(PlatformError::Denied(_))
        ));
        let request = VoiceOperation::CloudTranscribe {
            provider: CloudSpeechProvider::Openrouter,
            model: Some("a/b".into()),
            pcm: vec![0; 4],
            options,
        }
        .request(true)
        .expect("consented");
        assert_eq!(request.timeout_ms, 130_000);
        assert!(request.path.ends_with("?model=a%2Fb"));
        assert_eq!(&request.body[request.body.len() - 4..], &[0; 4]);
    }
    #[test]
    fn stream_bounds_duration_cancellation_revocation_and_deadline() {
        let mut s = SpeechStream::new(0);
        let actions = s.feed(&vec![0.; SPEECH_STREAM_MAX_QUEUED_BYTES / 4 + 1], true, 0);
        assert!(matches!(
            actions.as_slice(),
            [SpeechAction::Close, SpeechAction::Failed(_)]
        ));
        assert_eq!(s.buffered_bytes(), 0);
        for revoke in [true, false] {
            let mut s = SpeechStream::new(0);
            s.receive(SpeechStreamEvent::Ready, true, 0);
            s.feed(&[0.; 2], true, 1);
            s.feed(&[0.; 2], true, 2);
            s.feed(&[0.; 2], true, 3);
            if !revoke {
                s.cancel();
            }
            let actions = s.receive(
                SpeechStreamEvent::Update {
                    revision: 1,
                    text: None,
                },
                !revoke,
                4,
            );
            assert!(
                !actions
                    .iter()
                    .any(|a| matches!(a, SpeechAction::Audio(_) | SpeechAction::Text(_)))
            );
            assert!(s.result.is_some_and(|r| r.is_err()));
        }
        let mut s = SpeechStream::new(0);
        assert!(s.tick(119_999).is_empty());
        assert!(matches!(
            s.tick(120_000).as_slice(),
            [SpeechAction::Close, SpeechAction::Failed(_)]
        ));
    }
    #[test]
    fn stream_rejects_regressing_revision_duplicate_ready_and_early_finished() {
        for event in [
            SpeechStreamEvent::Finished {
                text: "early".into(),
            },
            SpeechStreamEvent::Update {
                revision: 1,
                text: None,
            },
        ] {
            let mut s = SpeechStream::new(0);
            assert!(matches!(
                s.receive(event, true, 0).as_slice(),
                [SpeechAction::Close, SpeechAction::Failed(_)]
            ));
        }
        let mut s = SpeechStream::new(0);
        s.receive(SpeechStreamEvent::Ready, true, 0);
        assert!(matches!(
            s.receive(SpeechStreamEvent::Ready, true, 0).as_slice(),
            [SpeechAction::Close, SpeechAction::Failed(_)]
        ));
    }
    #[test]
    fn transcript_spacing_respects_recorded_language_and_unicode() {
        for (text, spoken, locale, expected) in [
            ("Hello", "world", "auto", "Hello world"),
            ("Hello", "世界", "auto", "Hello世界"),
            ("Hello", "𝟚", "auto", "Hello 𝟚"),
            ("你好", "世界", "auto", "你好世界"),
            ("코드를", "검토해 주세요", "ko_KR", "코드를 검토해 주세요"),
            ("𞤀", "𞤦", "ff-Adlm", "𞤀 𞤦"),
            ("नमस्ते", "दुनिया", "hi-IN", "नमस्ते दुनिया"),
            (
                "修正キャッシュ",
                "テストも",
                "ja-JP",
                "修正キャッシュテストも",
            ),
        ] {
            let n = text.encode_utf16().count();
            let d = draft(text, n, n);
            assert!(
                matches!(resolve_transcript_commit(&d,Some(&d),spoken,locale),TranscriptCommit::Commit{text,..} if text==expected),
                "{text} + {spoken}"
            );
        }
        let d = draft("Änderungen", 0, 0);
        assert!(
            matches!(resolve_transcript_commit(&d,Some(&d),"Weitere","de_DE"),TranscriptCommit::Commit{text,..} if text=="Weitere Änderungen")
        );
    }
    #[test]
    fn transcript_preserves_citations_repeated_edits_and_emoji_boundaries() {
        for (selection, current, expected) in [
            (
                Selection { start: 6, end: 11 },
                "hello [cite]world",
                "hello [cite]world new text",
            ),
            (
                Selection { start: 6, end: 11 },
                "hello wo[cite]rld",
                "hello wo[cite]rld new text",
            ),
            (
                Selection { start: 6, end: 6 },
                "hello [cite] world",
                "hello [cite] new text world",
            ),
            (
                Selection { start: 0, end: 5 },
                "hello hello world",
                "hello hello new text world",
            ),
        ] {
            let mut captured = draft("hello world", 0, 0);
            captured.selection = selection;
            assert!(
                matches!(resolve_transcript_commit(&captured,Some(&draft(current,0,0)),"new text","en-US"),TranscriptCommit::Commit{text,..} if text==expected)
            );
        }
        let captured = draft("🔔", 0, 0);
        assert!(
            matches!(resolve_transcript_commit(&captured,Some(&draft("🤔",0,0)),"hello","en-US"),TranscriptCommit::Commit{text,..} if text=="🤔hello")
        );
        let captured = draft("Fix 🧪 then $review please", 12, 19);
        assert!(
            matches!(resolve_transcript_commit(&captured,Some(&captured),"use the mobile skill","en-US"),TranscriptCommit::Commit{text,..} if text=="Fix 🧪 then use the mobile skill please")
        );
        let captured = draft("hello", 5, 5);
        let mut other = captured.clone();
        other.owner_key = "other".into();
        assert_eq!(
            resolve_transcript_commit(&captured, Some(&other), "text", "en"),
            TranscriptCommit::Stale
        );
    }
    #[test]
    fn capture_flushes_and_releases_audio_before_stream_finish_and_commits_once() {
        let mut c = controller();
        let g = record(&mut c, true, None);
        stop(&mut c, g, true);
        let effects = c.transcribed(g, "new text".into(), false, Some(&draft("hello", 5, 5)));
        assert!(effects.contains(&VoiceEffect::Commit {
            text: "hello new text".into(),
            selection: Selection { start: 14, end: 14 }
        }));
        assert!(
            c.transcribed(g, "late".into(), false, Some(&draft("hello", 5, 5)))
                .is_empty()
        );
        assert_eq!(c.state.phase, VoicePhase::Idle);
    }
    #[test]
    fn live_transcription_is_retained_until_capture_stops() {
        for complete_early in [true, false] {
            let mut c = controller();
            c.start(Some(draft("hello", 5, 5)), true);
            let g = c.generation();
            c.permission(g, true, true);
            c.prepared(g, "en-US".into(), false, true);
            c.configured(g);
            assert_eq!(
                c.recorder_prepared(g, Some("audio".into()), Some(draft("hello", 5, 5))),
                vec![
                    VoiceEffect::StartLive("audio".into()),
                    VoiceEffect::Record { seconds: 300 }
                ]
            );
            if complete_early {
                assert!(c.live_settled(g, Ok(Some("live".into()))).is_empty());
                assert_eq!(c.state.phase, VoicePhase::Recording);
            }
            c.stop(false, None);
            if complete_early {
                assert_eq!(
                    c.audio_released(g),
                    vec![VoiceEffect::TranscriptAvailable("live".into())]
                );
            } else {
                assert_eq!(c.audio_released(g), vec![VoiceEffect::AwaitLive]);
                assert_eq!(
                    c.live_settled(g, Ok(Some("live".into()))),
                    vec![VoiceEffect::TranscriptAvailable("live".into())]
                );
            }
            assert!(
                c.transcribed(g, "live".into(), false, Some(&draft("hello", 5, 5)))
                    .iter()
                    .any(|e| matches!(e, VoiceEffect::Commit { .. }))
            );
        }
    }
    #[test]
    fn preparation_and_transcription_serialize_inference_and_ignore_stale_completion() {
        let mut c = controller();
        c.start(Some(draft("", 0, 0)), true);
        let generation = c.generation();
        c.permission(generation, true, true);
        let lease = c.lease().expect("lease");
        c.cancel();
        assert!(c.resources_released(lease).is_empty());
        assert_eq!(c.lease(), Some(lease));
        assert!(c.prepared(generation, "en".into(), false, false).is_empty());
        assert!(c.resources_released(lease).is_empty());
        assert_eq!(c.lease(), None);
        let mut c = controller();
        c.arbiter.borrow_mut().inference = Some(999);
        c.start(Some(draft("", 0, 0)), true);
        let g = c.generation();
        assert!(
            !c.permission(g, true, true)
                .contains(&VoiceEffect::PrepareTranscription)
        );
        assert_eq!(
            c.state.error.as_deref(),
            Some("Voice transcription is still finishing. Try again shortly.")
        );
    }
    #[test]
    fn skips_postprocessing_immediately_and_ignores_late_cleanup_result() {
        let mut c = controller();
        let g = record(&mut c, false, Some("audio".into()));
        stop(&mut c, g, false);
        assert_eq!(
            c.transcribed(g, "raw".into(), true, None),
            vec![VoiceEffect::PostProcess("raw".into())]
        );
        let effects = c.skip_post_processing(Some(&draft("hello", 5, 5)));
        assert!(effects.contains(&VoiceEffect::Commit {
            text: "hello raw".into(),
            selection: Selection { start: 9, end: 9 }
        }));
        assert!(
            c.post_processed(g, Ok("late".into()), Some(&draft("hello", 5, 5)))
                .is_empty()
        );
    }
    #[test]
    fn cancellation_waits_for_inference_before_deleting_audio_or_releasing_lease() {
        let mut c = controller();
        let g = record(&mut c, false, Some("audio".into()));
        let lease = c.lease().expect("lease");
        assert!(c.arbiter.borrow_mut().inference_started(lease));
        c.cancel();
        assert!(
            c.transcribed(g, "late".into(), false, Some(&draft("hello", 5, 5)))
                .is_empty()
        );
        assert!(c.resources_released(lease).is_empty());
        assert_eq!(c.lease(), Some(lease));
        assert_eq!(
            c.start(Some(draft("next", 4, 4)), true),
            vec![VoiceEffect::WaitForRelease]
        );
        let next = c.generation();
        c.arbiter.borrow_mut().inference_settled(lease);
        assert_eq!(
            c.resources_released(lease),
            vec![VoiceEffect::DeleteRecording("audio".into())]
        );
        assert_eq!(
            c.resume_waiter(next, true),
            vec![VoiceEffect::RequestPermission]
        );
        assert!(c.resources_released(lease).is_empty());
    }
    #[test]
    fn batch_retry_retains_audio_until_success_or_cancel() {
        let mut c = controller();
        let g = record(&mut c, false, Some("audio".into()));
        stop(&mut c, g, false);
        c.transcription_failed(g, "failed");
        let lease = c.lease().expect("lease");
        assert!(c.resources_released(lease).is_empty());
        assert_eq!(
            c.start(Some(draft("hello", 5, 5)), true),
            vec![VoiceEffect::Transcribe(Some("audio".into()))]
        );
        let g = c.generation();
        c.transcribed(g, "retried".into(), false, Some(&draft("hello", 5, 5)));
        let lease = c.lease().expect("retry lease");
        assert_eq!(
            c.resources_released(lease),
            vec![VoiceEffect::DeleteRecording("audio".into())]
        );
    }
    #[test]
    fn permissions_support_and_owner_changes_never_start_capture() {
        let mut c = controller();
        c.start(Some(draft("", 0, 0)), false);
        assert_eq!(c.state.phase, VoicePhase::Error);
        let mut c = controller();
        c.start(Some(draft("", 0, 0)), true);
        let g = c.generation();
        c.permission(g, false, false);
        assert_eq!(c.state.error_action, Some(VoiceErrorAction::Settings));
        let mut c = controller();
        c.start(Some(draft("", 0, 0)), true);
        let g = c.generation();
        c.prepared(g, "en".into(), false, false);
        c.configured(g);
        let mut other = draft("", 0, 0);
        other.owner_key = "other".into();
        let effects = c.recorder_prepared(g, None, Some(other));
        assert!(
            !effects
                .iter()
                .any(|a| matches!(a, VoiceEffect::Record { .. }))
        );
        assert_eq!(c.state.phase, VoicePhase::Error);
    }
    #[test]
    fn arbiter_rejects_concurrent_capture_and_background_abandons_preparation() {
        let arbiter = Rc::new(RefCell::new(VoiceArbiter::default()));
        let mut a = VoiceController::new(arbiter.clone(), false);
        let mut b = VoiceController::new(arbiter, false);
        a.start(Some(draft("", 0, 0)), true);
        b.start(Some(draft("", 0, 0)), true);
        assert_eq!(
            b.state.error.as_deref(),
            Some("Another voice recording is already active.")
        );
        a.backgrounded();
        assert_eq!(
            a.state.error.as_deref(),
            Some("Voice input stopped when the app moved to the background.")
        );
        b.cancel();
        assert!(
            b.start(Some(draft("", 0, 0)), true)
                .contains(&VoiceEffect::WaitForRelease)
        );
    }
}
