use crate::base_schemas::TrimmedNonEmptyString;
use crate::wire::JsNumber;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type CloudSpeechApiKey = TrimmedNonEmptyString;
pub type CloudSpeechModelId = String;
pub type SpeechModelId = String;
pub type SpeechLanguage = String;
pub type SpeechAcceleration = String;
pub type SpeechCorrectionCue = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloudSpeechProvider {
    Openai,
    Xai,
    Openrouter,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CloudSpeechKeys {
    pub openai: bool,
    pub xai: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub openrouter: Option<bool>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CloudSpeechModel {
    pub id: CloudSpeechModelId,
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CloudSpeechModels {
    pub models: Vec<CloudSpeechModel>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CloudSpeechErrorReason {
    MissingKey,
    KeyChanged,
    UnreadableKey,
    RejectedKey,
    RateLimit,
    ModelUnavailable,
    Unavailable,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CloudSpeechErrorTag {
    CloudSpeechError,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CloudSpeechError {
    #[serde(rename = "_tag")]
    pub tag: CloudSpeechErrorTag,
    pub provider: CloudSpeechProvider,
    pub reason: CloudSpeechErrorReason,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeechCustomWord {
    pub term: String,
    pub aliases: Vec<String>,
}
pub type SpeechCustomWords = Vec<SpeechCustomWord>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SpeechModelUnloadTimeout {
    #[serde(rename = "never")]
    Never,
    #[serde(rename = "immediately")]
    Immediately,
    #[serde(rename = "min_2")]
    Min2,
    #[serde(rename = "min_5")]
    Min5,
    #[serde(rename = "min_10")]
    Min10,
    #[serde(rename = "min_15")]
    Min15,
    #[serde(rename = "hour_1")]
    Hour1,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeechGpuDevice {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeechBenchmarkTier {
    Cpu,
    Accelerated,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentSpeechModelState {
    Downloadable,
    Downloading,
    Verifying,
    Installed,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechBenchmarkAccuracy {
    pub grade: String,
    pub error_percent: JsNumber,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "SpeechBenchmarkWire")]
pub struct SpeechBenchmark {
    pub accuracy: BTreeMap<String, SpeechBenchmarkAccuracy>,
    #[serde(with = "crate::wire::nullable")]
    pub seconds_per30s: Option<JsNumber>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cpu_seconds_per30s: Option<Option<JsNumber>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentSpeechModel {
    pub id: SpeechModelId,
    pub name: String,
    pub description: String,
    pub size: JsNumber,
    pub languages: Vec<String>,
    pub accuracy: JsNumber,
    pub speed: JsNumber,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub benchmark: Option<SpeechBenchmark>,
    pub recommended: bool,
    pub supports_streaming: bool,
    pub supports_language_detection: bool,
    pub supports_translation: bool,
    pub active: bool,
    pub state: EnvironmentSpeechModelState,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub downloaded: Option<JsNumber>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub linked_from: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentSpeechModels {
    pub models: Vec<EnvironmentSpeechModel>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentSpeechModelRequest {
    pub model_id: SpeechModelId,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentSpeechModelLinkRequest {
    pub path: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnvironmentSpeechState {
    MissingModel,
    Ready,
    Transcribing,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "SupportedSpeechStatusWire")]
pub struct SupportedSpeechStatus {
    pub supported: True,
    pub state: EnvironmentSpeechState,
    pub model_id: SpeechModelId,
    pub model: String,
    pub size: JsNumber,
    pub supports_streaming: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub supports_buffered_streaming: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub supports_model_linking: Option<bool>,
    pub supports_translation: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub languages: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub supports_language_detection: Option<bool>,
    pub acceleration: SpeechAcceleration,
    pub benchmark_tier: SpeechBenchmarkTier,
    pub model_unload_timeout: SpeechModelUnloadTimeout,
    pub gpu_devices: Vec<SpeechGpuDevice>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnsupportedSpeechStatus {
    pub supported: False,
    pub reason: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EnvironmentSpeechStatus {
    Supported(Box<SupportedSpeechStatus>),
    Unsupported(UnsupportedSpeechStatus),
}
#[derive(Debug, Clone, PartialEq)]
pub struct True;
#[derive(Debug, Clone, PartialEq)]
pub struct False;
macro_rules! literal_bool {
    ($name:ident, $value:expr) => {
        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_bool($value)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                if bool::deserialize(d)? == $value {
                    Ok(Self)
                } else {
                    Err(serde::de::Error::custom("incorrect boolean literal"))
                }
            }
        }
    };
}
literal_bool!(True, true);
literal_bool!(False, false);
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentSpeechTranscriptionResult {
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentSpeechAccelerationRequest {
    pub acceleration: SpeechAcceleration,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentSpeechModelUnloadTimeoutRequest {
    pub timeout: SpeechModelUnloadTimeout,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeechPostProcessingPromptMode {
    Default,
    Custom,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechPostProcessingPromptSettings {
    pub mode: SpeechPostProcessingPromptMode,
    pub custom_instructions: String,
}
fn default_silence_filtering() -> bool {
    true
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechTranscriptionOptions {
    pub speech_language: SpeechLanguage,
    pub speech_translate_to_english: bool,
    pub speech_custom_words: SpeechCustomWords,
    pub speech_remove_filler_words: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub speech_correction_word: Option<SpeechCorrectionCue>,
    #[serde(default = "default_silence_filtering")]
    pub speech_silence_filtering: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechPostProcessingOptions {
    pub speech_custom_words: SpeechCustomWords,
    pub speech_post_processing_prompt: SpeechPostProcessingPromptSettings,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub speech_correction_word: Option<SpeechCorrectionCue>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeechDraftSelection {
    pub start: SpeechNumber,
    pub end: SpeechNumber,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeechDraft {
    pub text: String,
    pub selection: SpeechDraftSelection,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentSpeechPostProcessingRequest {
    pub options: SpeechPostProcessingOptions,
    pub transcript: String,
    pub draft: SpeechDraft,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentSpeechPostProcessingResult {
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SpeechStreamCommand {
    Start { options: SpeechTranscriptionOptions },
    Finish,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeechStreamText {
    pub committed: String,
    pub tentative: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SpeechStreamEvent {
    Ready,
    Update {
        revision: i64,
        #[serde(with = "crate::wire::nullable")]
        text: Option<SpeechStreamText>,
    },
    Finished {
        text: String,
    },
    Error {
        message: String,
    },
}

// Golden JSON represents JavaScript's non-finite Number values with string sentinels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SpeechNumber {
    Finite(JsNumber),
    NonFinite(NonFiniteSpeechNumber),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NonFiniteSpeechNumber {
    #[serde(rename = "NaN")]
    NaN,
    #[serde(rename = "Infinity")]
    Infinity,
    #[serde(rename = "-Infinity")]
    NegativeInfinity,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpeechBenchmarkTimes {
    #[serde(default, with = "crate::wire::double_option")]
    cpu: Option<Option<JsNumber>>,
    #[serde(with = "crate::wire::nullable")]
    accelerated: Option<JsNumber>,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum SpeechBenchmarkSeconds {
    Times(SpeechBenchmarkTimes),
    Scalar(Option<JsNumber>),
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpeechBenchmarkWire {
    accuracy: BTreeMap<String, SpeechBenchmarkAccuracy>,
    seconds_per30s: SpeechBenchmarkSeconds,
    #[serde(default, with = "crate::wire::double_option")]
    cpu_seconds_per30s: Option<Option<JsNumber>>,
}
impl From<SpeechBenchmarkWire> for SpeechBenchmark {
    fn from(value: SpeechBenchmarkWire) -> Self {
        let (seconds_per30s, cpu_seconds_per30s) = match value.seconds_per30s {
            SpeechBenchmarkSeconds::Times(times) => (times.accelerated, times.cpu),
            SpeechBenchmarkSeconds::Scalar(seconds) => (seconds, value.cpu_seconds_per30s),
        };
        Self {
            accuracy: value.accuracy,
            seconds_per30s,
            cpu_seconds_per30s,
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SupportedSpeechStatusWire {
    pub supported: True,
    pub state: EnvironmentSpeechState,
    pub model_id: SpeechModelId,
    pub model: String,
    pub size: JsNumber,
    pub supports_streaming: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub supports_buffered_streaming: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub supports_model_linking: Option<bool>,
    pub supports_translation: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub languages: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub supports_language_detection: Option<bool>,
    pub acceleration: SpeechAcceleration,
    #[serde(default, with = "crate::wire::optional")]
    pub benchmark_tier: Option<SpeechBenchmarkTier>,
    pub model_unload_timeout: SpeechModelUnloadTimeout,
    pub gpu_devices: Vec<SpeechGpuDevice>,
}
impl From<SupportedSpeechStatusWire> for SupportedSpeechStatus {
    fn from(value: SupportedSpeechStatusWire) -> Self {
        let benchmark_tier = value.benchmark_tier.unwrap_or_else(|| {
            if value.acceleration == "cpu"
                || (value.acceleration == "auto" && value.gpu_devices.is_empty())
            {
                SpeechBenchmarkTier::Cpu
            } else {
                SpeechBenchmarkTier::Accelerated
            }
        });
        Self {
            benchmark_tier,
            supported: value.supported,
            state: value.state,
            model_id: value.model_id,
            model: value.model,
            size: value.size,
            supports_streaming: value.supports_streaming,
            supports_buffered_streaming: value.supports_buffered_streaming,
            supports_model_linking: value.supports_model_linking,
            supports_translation: value.supports_translation,
            languages: value.languages,
            supports_language_detection: value.supports_language_detection,
            acceleration: value.acceleration,
            model_unload_timeout: value.model_unload_timeout,
            gpu_devices: value.gpu_devices,
        }
    }
}
