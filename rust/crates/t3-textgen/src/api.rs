use crate::{
    CancellationToken,
    generation::{GenerationError, GenerationRequest, TextRunner},
};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use t3_contracts::wire::trim_js;
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString,
    settings::{TextGeneratorConfig, TextGeneratorKind, TextGeneratorModel},
    settings_secret_value::SettingsSecretValue,
};
use tokio::sync::Mutex;

/// Outbound HTTP the way Node's fetch does it, so deployments keep working unchanged: ambient
/// `HTTP(S)_PROXY`/`ALL_PROXY` variables are ignored, and the bundled Mozilla roots are trusted
/// plus every certificate in the PEM file `NODE_EXTRA_CA_CERTS` names. As in Node, an unreadable
/// file adds nothing rather than failing.
pub(crate) fn client_builder() -> reqwest::ClientBuilder {
    let extra = std::env::var_os("NODE_EXTRA_CA_CERTS")
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|pem| reqwest::Certificate::from_pem_bundle(&pem).ok())
        .unwrap_or_default();
    extra
        .into_iter()
        .fold(reqwest::Client::builder().no_proxy(), |builder, root| {
            builder.add_root_certificate(root)
        })
}
pub(crate) fn http_client() -> Result<reqwest::Client, GenerationError> {
    client_builder()
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
        response_json(response).await.map_err(|error| match error {
            GenerationError::InvalidOutput => GenerationError::UnreadableReply,
            other => other,
        })
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
            match self.config.kind {
                TextGeneratorKind::OpenaiCompatible => chat_completion_text(&reply),
                TextGeneratorKind::Anthropic => anthropic_message_text(&reply),
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
                let anthropic = matches!(self.config.kind, TextGeneratorKind::Anthropic);
                let name_key = if anthropic { "display_name" } else { "name" };
                for entry in model_page(&reply, name_key, anthropic)? {
                    let Some(id) = entry.get("id").and_then(Value::as_str).map(trim_js) else {
                        continue;
                    };
                    if id.is_empty() {
                        continue;
                    }
                    let name = entry
                        .get(name_key)
                        .and_then(Value::as_str)
                        .map(trim_js)
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
/// Checks a reply as TypeScript's schema decode does; a failure is "couldn't read".
fn shape(valid: bool) -> Result<(), GenerationError> {
    if valid {
        Ok(())
    } else {
        Err(GenerationError::UnreadableReply)
    }
}
fn null_or_string(value: Option<&Value>) -> bool {
    value.is_some_and(|value| value.is_null() || value.is_string())
}
/// An optional field is absent or of its type; JSON `null` is neither.
fn optional(value: Option<&Value>, of_type: fn(&Value) -> bool) -> bool {
    value.is_none_or(of_type)
}
/// `ChatCompletion`: every choice carries `finish_reason` and `message.content`, each nullable.
fn chat_completion_text(reply: &Value) -> Result<String, GenerationError> {
    let choices = reply.get("choices").and_then(Value::as_array);
    shape(choices.is_some_and(|choices| {
        choices.iter().all(|choice| {
            null_or_string(choice.get("finish_reason"))
                && null_or_string(
                    choice
                        .get("message")
                        .and_then(Value::as_object)
                        .and_then(|message| message.get("content")),
                )
        })
    }))?;
    let choice = choices.and_then(|choices| choices.first());
    if choice
        .and_then(|choice| choice.get("finish_reason"))
        .and_then(Value::as_str)
        == Some("length")
    {
        return Err(GenerationError::OutputLimit);
    }
    let text = choice
        .and_then(|choice| choice.pointer("/message/content"))
        .and_then(Value::as_str)
        .map(trim_js)
        .unwrap_or_default();
    if text.is_empty() {
        Err(GenerationError::EmptyOutput)
    } else {
        Ok(text.to_owned())
    }
}
/// `AnthropicMessage`: a nullable `stop_reason` and content blocks with a string `type`.
fn anthropic_message_text(reply: &Value) -> Result<String, GenerationError> {
    let content = reply.get("content").and_then(Value::as_array);
    shape(
        null_or_string(reply.get("stop_reason"))
            && content.is_some_and(|blocks| {
                blocks.iter().all(|block| {
                    block.get("type").is_some_and(Value::is_string)
                        && optional(block.get("text"), Value::is_string)
                })
            }),
    )?;
    if reply.get("stop_reason").and_then(Value::as_str) == Some("max_tokens") {
        return Err(GenerationError::OutputLimit);
    }
    let text = content
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<String>();
    let text = trim_js(&text);
    if text.is_empty() {
        Err(GenerationError::EmptyOutput)
    } else {
        Ok(text.to_owned())
    }
}
/// `OpenAIModelList` and `AnthropicModelPage`: entries with a string `id` and optional string name.
fn model_page<'a>(
    reply: &'a Value,
    name_key: &str,
    paged: bool,
) -> Result<&'a Vec<Value>, GenerationError> {
    let data = reply.get("data").and_then(Value::as_array);
    shape(
        data.is_some_and(|entries| {
            entries.iter().all(|entry| {
                entry.get("id").is_some_and(Value::is_string)
                    && optional(entry.get(name_key), Value::is_string)
            })
        }) && (!paged
            || optional(reply.get("has_more"), Value::is_boolean)
                && optional(reply.get("last_id"), |id| id.is_null() || id.is_string())),
    )?;
    data.ok_or(GenerationError::UnreadableReply)
}

/// The configuration a model listing or connection test uses: the saved one, or the supplied
/// unsaved one. A supplied key equal to the host's redaction `sentinel` stands for the saved key,
/// which is used only when [`t3_shared::text_generators::saved_api_key_applies`] holds (same kind
/// and origin); otherwise the key must be entered again, so it never reaches another address.
pub fn listing_config(
    supplied: Option<&TextGeneratorConfig>,
    saved: Option<&TextGeneratorConfig>,
    sentinel: &str,
) -> Result<TextGeneratorConfig, GenerationError> {
    let Some(supplied) = supplied else {
        return saved.cloned().ok_or(GenerationError::Unavailable);
    };
    let mut config = supplied.clone();
    if matches!(&supplied.api_key, SettingsSecretValue::Literal(key) if key == sentinel) {
        config.api_key = saved
            .filter(|saved| t3_shared::text_generators::saved_api_key_applies(supplied, saved))
            .map(|saved| saved.api_key.clone())
            .ok_or(GenerationError::KeyRequired)?;
    }
    Ok(config)
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
