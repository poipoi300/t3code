use crate::{
    CancellationToken,
    generation::{GenerationError, GenerationRequest, TextRunner},
};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString,
    settings::{TextGeneratorConfig, TextGeneratorKind, TextGeneratorModel},
    settings_secret_value::SettingsSecretValue,
};
use tokio::sync::Mutex;

pub(crate) fn http_client() -> Result<reqwest::Client, GenerationError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| GenerationError::Provider)
}
pub(crate) async fn response_json(response: reqwest::Response) -> Result<Value, GenerationError> {
    const MAX_REPLY: usize = 8 * 1024 * 1024;
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| GenerationError::Provider)?;
        if bytes.len().saturating_add(chunk.len()) > MAX_REPLY {
            return Err(GenerationError::InvalidOutput);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| GenerationError::InvalidOutput)
}
fn status_error(status: reqwest::StatusCode) -> GenerationError {
    match status.as_u16() {
        401 | 403 => GenerationError::RejectedKey,
        404 => GenerationError::Unavailable,
        429 => GenerationError::RateLimit,
        _ => GenerationError::Provider,
    }
}

#[async_trait]
pub trait GeneratorSecrets: Send + Sync {
    async fn resolve(&self, value: &SettingsSecretValue) -> Result<String, GenerationError>;
}

pub struct ApiTextRunner {
    config: TextGeneratorConfig,
    secrets: Arc<dyn GeneratorSecrets>,
    client: reqwest::Client,
}
impl ApiTextRunner {
    pub fn new(
        config: TextGeneratorConfig,
        secrets: Arc<dyn GeneratorSecrets>,
    ) -> Result<Self, GenerationError> {
        Ok(Self {
            config,
            secrets,
            client: http_client()?,
        })
    }
    async fn send(&self, request: reqwest::RequestBuilder) -> Result<Value, GenerationError> {
        let response = request
            .send()
            .await
            .map_err(|_| GenerationError::Provider)?;
        if !response.status().is_success() {
            return Err(status_error(response.status()));
        }
        response_json(response).await
    }
    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.config.base_url.0.trim_end_matches('/'))
    }
    fn authenticate(&self, request: reqwest::RequestBuilder, key: &str) -> reqwest::RequestBuilder {
        match self.config.kind {
            TextGeneratorKind::Anthropic => request
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01"),
            TextGeneratorKind::OpenaiCompatible => {
                if key.is_empty() {
                    request
                } else {
                    request.bearer_auth(key)
                }
            }
        }
    }
    pub async fn generate_text(
        &self,
        model: &str,
        prompt: &str,
    ) -> Result<String, GenerationError> {
        let work = async {
            let key = self.secrets.resolve(&self.config.api_key).await?;
            let (path, body) = match self.config.kind {
                TextGeneratorKind::OpenaiCompatible => (
                    "/chat/completions",
                    json!({"model":model,"messages":[{"role":"user","content":prompt}]}),
                ),
                TextGeneratorKind::Anthropic => (
                    "/messages",
                    json!({"model":model,"max_tokens":8192,"messages":[{"role":"user","content":prompt}]}),
                ),
            };
            let reply = self
                .send(self.authenticate(self.client.post(self.endpoint(path)).json(&body), &key))
                .await?;
            let text = match self.config.kind {
                TextGeneratorKind::OpenaiCompatible => {
                    let choice = reply
                        .get("choices")
                        .and_then(Value::as_array)
                        .and_then(|c| c.first())
                        .ok_or(GenerationError::InvalidOutput)?;
                    if choice.get("finish_reason").and_then(Value::as_str) == Some("length") {
                        return Err(GenerationError::OutputLimit);
                    }
                    choice
                        .pointer("/message/content")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned()
                }
                TextGeneratorKind::Anthropic => {
                    if reply.get("stop_reason").and_then(Value::as_str) == Some("max_tokens") {
                        return Err(GenerationError::OutputLimit);
                    }
                    let content = reply
                        .get("content")
                        .and_then(Value::as_array)
                        .ok_or(GenerationError::InvalidOutput)?;
                    content
                        .iter()
                        .filter(|c| c.get("type").and_then(Value::as_str) == Some("text"))
                        .filter_map(|c| c.get("text").and_then(Value::as_str))
                        .collect::<String>()
                }
            };
            if text.trim().is_empty() {
                Err(GenerationError::EmptyOutput)
            } else {
                Ok(text.trim().to_owned())
            }
        };
        tokio::time::timeout(Duration::from_secs(120), work)
            .await
            .map_err(|_| GenerationError::Timeout)?
    }
    pub async fn list_models(&self) -> Result<Vec<TextGeneratorModel>, GenerationError> {
        let work = async {
            let key = self.secrets.resolve(&self.config.api_key).await?;
            let mut models = BTreeMap::new();
            let mut openai_models = Vec::new();
            let mut after = None::<String>;
            for _ in 0..10 {
                let mut request = self.client.get(self.endpoint("/models"));
                if matches!(self.config.kind, TextGeneratorKind::Anthropic) {
                    request = request.query(&[("limit", "1000")]);
                    if let Some(after) = &after {
                        request = request.query(&[("after_id", after)]);
                    }
                }
                let reply = self.send(self.authenticate(request, &key)).await?;
                for entry in reply
                    .get("data")
                    .and_then(Value::as_array)
                    .ok_or(GenerationError::InvalidOutput)?
                {
                    let id = entry
                        .get("id")
                        .and_then(Value::as_str)
                        .ok_or(GenerationError::InvalidOutput)?
                        .trim();
                    if id.is_empty() {
                        continue;
                    }
                    let name = entry
                        .get(
                            if matches!(self.config.kind, TextGeneratorKind::Anthropic) {
                                "display_name"
                            } else {
                                "name"
                            },
                        )
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .unwrap_or(id);
                    let model = TextGeneratorModel {
                        id: TrimmedNonEmptyString(id.into()),
                        name: TrimmedNonEmptyString(name.into()),
                    };
                    if matches!(self.config.kind, TextGeneratorKind::Anthropic) {
                        models.insert(id.to_owned(), model);
                    } else {
                        openai_models.push(model);
                    }
                }
                if !matches!(self.config.kind, TextGeneratorKind::Anthropic) {
                    break;
                }
                let last = reply
                    .get("last_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty());
                if reply.get("has_more").and_then(Value::as_bool) != Some(true)
                    || last.is_none()
                    || last == after.as_deref()
                {
                    break;
                }
                after = last.map(str::to_owned);
            }
            let mut output = if matches!(self.config.kind, TextGeneratorKind::Anthropic) {
                models.into_values().collect()
            } else {
                openai_models
            };
            let collator = icu_collator::Collator::try_new(Default::default(), Default::default())
                .expect("compiled root collation");
            output.sort_by(|a, b| collator.compare(&a.id.0, &b.id.0));
            Ok(output)
        };
        tokio::time::timeout(Duration::from_secs(25), work)
            .await
            .map_err(|_| GenerationError::Timeout)?
    }
}
#[async_trait]
impl TextRunner for ApiTextRunner {
    async fn run(
        &self,
        request: &GenerationRequest,
        cancellation: CancellationToken,
    ) -> Result<String, GenerationError> {
        if request.investigation_timeout.is_some() {
            return Err(GenerationError::InvestigationUnsupported);
        }
        if !self.config.enabled {
            return Err(GenerationError::Disabled);
        }
        tokio::select! {biased;_=cancellation.cancelled()=>Err(GenerationError::Cancelled),reply=self.generate_text(&request.model_selection.model,&request.prompt)=>reply}
    }
}

#[derive(Default)]
pub struct ModelCache {
    cache: Mutex<BTreeMap<String, (TextGeneratorConfig, Vec<TextGeneratorModel>)>>,
}
impl ModelCache {
    pub async fn list(
        &self,
        id: &str,
        runner: &ApiTextRunner,
        refresh: bool,
        unsaved: bool,
        test_model: Option<Option<&str>>,
        policy_allows: bool,
    ) -> Result<(Vec<TextGeneratorModel>, Option<String>), GenerationError> {
        if !policy_allows {
            return Err(GenerationError::Disabled);
        }
        if !refresh
            && !unsaved
            && test_model.is_none()
            && let Some((config, models)) = self.cache.lock().await.get(id)
            && config == &runner.config
        {
            return Ok((models.clone(), None));
        }
        let models = runner.list_models().await?;
        if let Some(explicit) = test_model {
            let model = explicit
                .map(str::to_owned)
                .or_else(|| {
                    models
                        .iter()
                        .find(|m| {
                            ["haiku", "mini", "flash", "nano"]
                                .iter()
                                .any(|s| m.id.0.to_lowercase().contains(s))
                        })
                        .or(models.first())
                        .map(|m| m.id.0.clone())
                })
                .ok_or(GenerationError::EmptyOutput)?;
            runner.generate_text(&model, "Reply with OK.").await?;
            return Ok((models, Some(model)));
        }
        self.cache
            .lock()
            .await
            .insert(id.to_owned(), (runner.config.clone(), models.clone()));
        Ok((models, None))
    }
}
