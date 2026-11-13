use crate::{
    CancellationToken,
    api::{http_client, response_json},
    dictionary::{apply_aliases, remove_filler_words},
    speech::{MAX_SPEECH_BYTES, SpeechError, decode_pcm, effective_language, validate_options},
};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use t3_contracts::{
    settings_secret_value::SettingsSecretValue,
    speech::{
        CloudSpeechErrorReason, CloudSpeechModel, CloudSpeechProvider, SpeechTranscriptionOptions,
    },
};
use tokio::{sync::Mutex, time::Instant};

#[derive(Debug, Clone, thiserror::Error)]
#[error("Cloud speech failed ({provider:?}, {reason:?}).")]
pub struct CloudError {
    pub provider: CloudSpeechProvider,
    pub reason: CloudSpeechErrorReason,
}
#[derive(Debug, thiserror::Error)]
pub enum CloudTranscriptionError {
    #[error(transparent)]
    Cloud(#[from] CloudError),
    #[error(transparent)]
    Audio(#[from] SpeechError),
}
#[async_trait]
pub trait CloudCredentials: Send + Sync {
    async fn current(
        &self,
        provider: CloudSpeechProvider,
    ) -> Result<Option<SettingsSecretValue>, ()>;
    /// Resolve afresh, invalidating any cached vault credential before authorizing an upload.
    async fn resolve_fresh(&self, value: &SettingsSecretValue) -> Result<String, ()>;
    async fn save(
        &self,
        provider: CloudSpeechProvider,
        value: Option<SettingsSecretValue>,
    ) -> Result<(), ()>;
}
pub fn valid_model_id(model: &str) -> bool {
    model.len() <= 160
        && !model.to_lowercase().starts_with("openrouter/")
        && regex::Regex::new(r"(?i)^[a-z\d][a-z\d.-]*/[a-z\d][a-z\d._-]*$")
            .expect("cloud model pattern")
            .is_match(model)
}
fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 4096
        && key.bytes().all(|b| (b'!'..=b'~').contains(&b))
        && !regex::Regex::new(r"(?i)^[a-z][a-z\d+.-]*://")
            .expect("key pattern")
            .is_match(key)
}
pub fn wave(samples: &[f32]) -> Result<Vec<u8>, SpeechError> {
    if samples.len() > MAX_SPEECH_BYTES / 4
        || samples
            .iter()
            .any(|s| !s.is_finite() || !(-1.0..=1.0).contains(s))
    {
        return Err(SpeechError::InvalidAudio);
    }
    let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
    let length = (samples.len() * 2) as u32;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + length).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&16000u32.to_le_bytes());
    bytes.extend_from_slice(&32000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&length.to_le_bytes());
    for &sample in samples {
        let value =
            (sample as f64 * if sample < 0.0 { 32768.0 } else { 32767.0 } + 0.5).floor() as i16;
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    Ok(bytes)
}
pub struct CloudSpeech {
    credentials: Arc<dyn CloudCredentials>,
    client: reqwest::Client,
    models: Mutex<Option<(Instant, Vec<CloudSpeechModel>)>>,
    endpoints: [String; 3],
    catalog_url: String,
}
#[cfg(test)]
#[path = "cloud_tests.rs"]
mod tests;
impl CloudSpeech {
    pub async fn configured(&self) -> Result<t3_contracts::speech::CloudSpeechKeys, CloudError> {
        let mut states = Vec::new();
        for provider in [
            CloudSpeechProvider::Openai,
            CloudSpeechProvider::Xai,
            CloudSpeechProvider::Openrouter,
        ] {
            states.push(
                self.credentials
                    .current(provider.clone())
                    .await
                    .map_err(|_| CloudError {
                        provider,
                        reason: CloudSpeechErrorReason::Unavailable,
                    })?
                    .is_some(),
            );
        }
        Ok(t3_contracts::speech::CloudSpeechKeys {
            openai: states[0],
            xai: states[1],
            openrouter: Some(states[2]),
        })
    }
    pub async fn set_key(
        &self,
        provider: CloudSpeechProvider,
        value: Option<SettingsSecretValue>,
    ) -> Result<(), CloudError> {
        self.credentials
            .save(provider.clone(), value)
            .await
            .map_err(|_| CloudError {
                provider,
                reason: CloudSpeechErrorReason::Unavailable,
            })
    }
    pub fn new(credentials: Arc<dyn CloudCredentials>) -> Result<Self, CloudError> {
        Ok(Self {
            credentials,
            client: http_client().map_err(|_| CloudError {
                provider: CloudSpeechProvider::Openai,
                reason: CloudSpeechErrorReason::Unavailable,
            })?,
            models: Mutex::new(None),
            endpoints: [
                "https://api.openai.com/v1/audio/transcriptions".into(),
                "https://api.x.ai/v1/stt".into(),
                "https://openrouter.ai/api/v1/audio/transcriptions".into(),
            ],
            catalog_url:
                "https://openrouter.ai/api/v1/models?output_modalities=transcription&zdr=true"
                    .into(),
        })
    }
    pub async fn models(&self) -> Result<Vec<CloudSpeechModel>, CloudError> {
        let error = || CloudError {
            provider: CloudSpeechProvider::Openrouter,
            reason: CloudSpeechErrorReason::Unavailable,
        };
        let mut cached = self.models.lock().await;
        if let Some((at, models)) = &*cached
            && Instant::now().duration_since(*at) < Duration::from_secs(600)
        {
            return Ok(models.clone());
        }
        let work = async {
            let response = self
                .client
                .get(&self.catalog_url)
                .send()
                .await
                .map_err(|_| error())?;
            if !response.status().is_success() {
                return Err(error());
            }
            let reply = response_json(response).await.map_err(|_| error())?;
            let data = reply
                .get("data")
                .and_then(Value::as_array)
                .ok_or_else(error)?;
            let mut models = Vec::new();
            for entry in data {
                let id = entry.get("id").and_then(Value::as_str).ok_or_else(error)?;
                if valid_model_id(id) {
                    models.push(CloudSpeechModel {
                        id: id.into(),
                        name: entry
                            .get("name")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .unwrap_or(id)
                            .into(),
                    });
                }
            }
            Ok(models)
        };
        let models = tokio::time::timeout(Duration::from_secs(10), work)
            .await
            .map_err(|_| error())??;
        *cached = Some((Instant::now(), models.clone()));
        Ok(models)
    }
    pub async fn transcribe(
        &self,
        provider: CloudSpeechProvider,
        pcm: &[u8],
        options: &SpeechTranscriptionOptions,
        model: Option<&str>,
        cancellation: CancellationToken,
    ) -> Result<String, CloudTranscriptionError> {
        let work = self.transcribe_inner(provider.clone(), pcm, options, model);
        tokio::select! {biased;_=cancellation.cancelled()=>Err(SpeechError::Cancelled.into()),result=tokio::time::timeout(Duration::from_secs(120),work)=>result.unwrap_or_else(|_|Err(CloudError{provider,reason:CloudSpeechErrorReason::Unavailable}.into()))}
    }
    async fn transcribe_inner(
        &self,
        provider: CloudSpeechProvider,
        pcm: &[u8],
        options: &SpeechTranscriptionOptions,
        model: Option<&str>,
    ) -> Result<String, CloudTranscriptionError> {
        let error = |reason| CloudError {
            provider: provider.clone(),
            reason,
        };
        let source = self
            .credentials
            .current(provider.clone())
            .await
            .map_err(|_| error(CloudSpeechErrorReason::Unavailable))?
            .ok_or_else(|| error(CloudSpeechErrorReason::MissingKey))?;
        if provider == CloudSpeechProvider::Openrouter && !model.is_some_and(valid_model_id) {
            return Err(error(CloudSpeechErrorReason::ModelUnavailable).into());
        }
        validate_options(options)?;
        let samples = decode_pcm(pcm)?;
        let languages="ar bg ca cs da de el en es fa fi fr hi hr hu id it ja ko mk ms nl pl pt ro ru sk sv th tr uk ur vi zh".split(' ').map(str::to_owned).collect::<Vec<_>>();
        let language = effective_language(&languages, true, &options.speech_language);
        if provider == CloudSpeechProvider::Openrouter
            && !self
                .models()
                .await?
                .iter()
                .any(|m| Some(m.id.as_str()) == model)
        {
            return Err(error(CloudSpeechErrorReason::ModelUnavailable).into());
        }
        let key = self
            .credentials
            .resolve_fresh(&source)
            .await
            .map_err(|_| error(CloudSpeechErrorReason::UnreadableKey))?;
        if !valid_key(&key) {
            return Err(error(CloudSpeechErrorReason::UnreadableKey).into());
        }
        let audio = wave(&samples)?;
        let index = match provider {
            CloudSpeechProvider::Openai => 0,
            CloudSpeechProvider::Xai => 1,
            CloudSpeechProvider::Openrouter => 2,
        };
        let mut request = self.client.post(&self.endpoints[index]).bearer_auth(key);
        if provider == CloudSpeechProvider::Openrouter {
            let mut body = json!({"model":model,"input_audio":{"data":STANDARD.encode(audio),"format":"wav"},"provider":{"zdr":true}});
            if language != "auto" {
                body["language"] = json!(language);
            }
            request = request.json(&body);
        } else {
            let openai = provider == CloudSpeechProvider::Openai;
            let mut form = reqwest::multipart::Form::new().text(
                "model",
                if openai {
                    "gpt-transcribe"
                } else {
                    "grok-voice-transcribe-2.0"
                },
            );
            if language != "auto" {
                form = form.text(
                    if openai { "languages[]" } else { "language" },
                    language.clone(),
                );
            }
            for word in &options.speech_custom_words {
                if openai && word.term.contains(['<', '>', '\r', '\n']) {
                    continue;
                }
                form = form.text(
                    if openai { "keywords[]" } else { "keyterm" },
                    word.term.clone(),
                );
            }
            if !openai {
                if language != "auto" {
                    form = form.text("format", "true");
                }
                form = form.text(
                    "filler_words",
                    (!options.speech_remove_filler_words
                        || options
                            .speech_correction_word
                            .as_ref()
                            .is_some_and(|s| !s.is_empty()))
                    .to_string(),
                );
                if !options.speech_silence_filtering {
                    form = form.text("vad_threshold", "0");
                }
            }
            // xAI ignores options after the file; it must be the final multipart field.
            form = form.part(
                "file",
                reqwest::multipart::Part::bytes(audio)
                    .file_name("dictation.wav")
                    .mime_str("audio/wav")
                    .map_err(|_| error(CloudSpeechErrorReason::Unavailable))?,
            );
            request = request.multipart(form);
        }
        let current = self
            .credentials
            .current(provider.clone())
            .await
            .map_err(|_| error(CloudSpeechErrorReason::Unavailable))?;
        match current {
            None => return Err(error(CloudSpeechErrorReason::MissingKey).into()),
            Some(current) if current != source => {
                return Err(error(CloudSpeechErrorReason::KeyChanged).into());
            }
            _ => (),
        }
        let response = request
            .send()
            .await
            .map_err(|_| error(CloudSpeechErrorReason::Unavailable))?;
        if !response.status().is_success() {
            return Err(error(match response.status().as_u16() {
                429 => CloudSpeechErrorReason::RateLimit,
                401..=403 => CloudSpeechErrorReason::RejectedKey,
                404 if provider == CloudSpeechProvider::Openrouter => {
                    CloudSpeechErrorReason::ModelUnavailable
                }
                _ => CloudSpeechErrorReason::Unavailable,
            })
            .into());
        }
        let reply = response_json(response)
            .await
            .map_err(|_| error(CloudSpeechErrorReason::Unavailable))?;
        let text = reply
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| error(CloudSpeechErrorReason::Unavailable))?;
        let text = apply_aliases(text, &options.speech_custom_words);
        Ok(if options.speech_remove_filler_words {
            remove_filler_words(
                &text,
                Some(&language),
                options.speech_correction_word.as_deref(),
            )
        } else {
            text
        }
        .trim()
        .into())
    }
}
