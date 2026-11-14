use crate::{
    secrets::SecretResolver,
    sensitive::{SecretString, Sensitive, ServiceError, ServiceResult},
    transport::{HttpRequest, HttpTransport},
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use t3_contracts::{
    provider_instance::ProviderInstanceId, settings::*, settings_secret_value::SettingsSecretValue,
};
use tokio::sync::Mutex;

#[async_trait]
pub trait SettingsSource: Send + Sync {
    async fn settings(&self) -> ServiceResult<ServerSettings>;
}
#[async_trait]
impl SettingsSource for t3_config::settings::SettingsService {
    async fn settings(&self) -> ServiceResult<ServerSettings> {
        self.snapshot().await.map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Storage,
        ))
    }
}
struct Cached {
    config: TextGeneratorConfig,
    models: Vec<TextGeneratorModel>,
}
pub struct TextGenerators {
    settings: Arc<dyn SettingsSource>,
    secrets: Arc<SecretResolver>,
    http: Arc<dyn HttpTransport>,
    cache: Mutex<BTreeMap<ProviderInstanceId, Cached>>,
    allow_listed: bool,
}
impl TextGenerators {
    pub fn new(
        settings: Arc<dyn SettingsSource>,
        secrets: Arc<SecretResolver>,
        http: Arc<dyn HttpTransport>,
        allow_listed: bool,
    ) -> Self {
        Self {
            settings,
            secrets,
            http,
            cache: Mutex::new(BTreeMap::new()),
            allow_listed,
        }
    }
    async fn key(
        &self,
        config: &TextGeneratorConfig,
        settings: &ServerSettings,
    ) -> ServiceResult<SecretString> {
        match &config.api_key {
            SettingsSecretValue::Literal(value) => Ok(Sensitive::new(value.clone())),
            SettingsSecretValue::Reference(reference) => self
                .secrets
                .read(
                    &settings.secret_providers,
                    std::slice::from_ref(reference),
                    false,
                )
                .await
                .into_iter()
                .next()
                .flatten()
                .map(Sensitive::new)
                .ok_or(ServiceError::Storage),
        }
    }
    fn request(
        config: &TextGeneratorConfig,
        key: &SecretString,
        path: &str,
        body: Option<Value>,
    ) -> HttpRequest {
        let anthropic = config.kind == TextGeneratorKind::Anthropic;
        let mut headers = BTreeMap::new();
        if anthropic {
            headers.insert("x-api-key".into(), key.clone());
            headers.insert(
                "anthropic-version".into(),
                Sensitive::new("2023-06-01".into()),
            );
        } else if !key.expose().is_empty() {
            headers.insert(
                "authorization".into(),
                Sensitive::new(format!("Bearer {}", key.expose())),
            );
        }
        if body.is_some() {
            headers.insert(
                "content-type".into(),
                Sensitive::new("application/json".into()),
            );
        }
        HttpRequest {
            method: if body.is_some() { "POST" } else { "GET" },
            url: format!("{}{path}", config.base_url.0.trim_end_matches('/')),
            headers,
            body: body.map(|v| Sensitive::new(v.to_string())),
        }
    }
    async fn json(&self, request: HttpRequest) -> ServiceResult<Value> {
        let response = self.http.send(request).await?;
        if !(200..300).contains(&response.status) {
            return Err(ServiceError::Response);
        }
        Ok(response.body.into_inner())
    }
    async fn models(
        &self,
        config: &TextGeneratorConfig,
        key: &SecretString,
    ) -> ServiceResult<Vec<TextGeneratorModel>> {
        let anthropic = config.kind == TextGeneratorKind::Anthropic;
        let mut models = vec![];
        let mut ids = BTreeMap::<String, TextGeneratorModel>::new();
        let mut after = None::<String>;
        for _ in 0..if anthropic { 10 } else { 1 } {
            let mut request = Self::request(config, key, "/models", None);
            if anthropic {
                let mut url = url::Url::parse(&request.url).map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Response,
                ))?;
                url.query_pairs_mut().append_pair("limit", "1000");
                if let Some(after) = after.as_ref() {
                    url.query_pairs_mut().append_pair("after_id", after);
                }
                request.url = url.into();
            }
            let page = self.json(request).await?;
            let data = page["data"].as_array().ok_or(ServiceError::Response)?;
            for entry in data {
                let id = entry["id"].as_str().ok_or(ServiceError::Response)?.trim();
                let name = entry.get(if anthropic { "display_name" } else { "name" });
                if name.is_some_and(|v| !v.is_string()) {
                    return Err(ServiceError::Response);
                }
                if id.is_empty() {
                    continue;
                }
                let name = name
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(id);
                let model = TextGeneratorModel {
                    id: id.to_owned().into(),
                    name: name.to_owned().into(),
                };
                if anthropic {
                    ids.insert(id.into(), model);
                } else {
                    models.push(model);
                }
            }
            if !anthropic {
                break;
            }
            let last = page["last_id"].as_str().filter(|s| !s.is_empty());
            if page["has_more"].as_bool() != Some(true)
                || last.is_none()
                || last == after.as_deref()
            {
                break;
            }
            after = last.map(str::to_owned);
        }
        if anthropic {
            models = ids.into_values().collect();
        }
        models.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(models)
    }
    pub async fn list_models(
        &self,
        input: ListTextGeneratorModelsInput,
    ) -> ServiceResult<ListTextGeneratorModelsResult> {
        if self.allow_listed {
            return Err(ServiceError::Disabled);
        }
        let settings = self.settings.settings().await?;
        let saved = settings.text_generators.get(&input.id);
        let supplied = input.config.as_ref().and_then(Option::as_ref);
        let mut config = supplied
            .or(saved)
            .cloned()
            .ok_or(ServiceError::Unavailable)?;
        if matches!(&config.api_key,SettingsSecretValue::Literal(s) if s==t3_config::secrets::REDACTED)
        {
            config.api_key = saved
                .filter(|saved| saved.kind == config.kind && saved.base_url == config.base_url)
                .map(|s| s.api_key.clone())
                .unwrap_or_else(|| SettingsSecretValue::Literal(String::new()));
        }
        let test = input.test.flatten() == Some(true);
        let fresh = supplied.is_some() || test;
        if !fresh
            && input.refresh.flatten() != Some(true)
            && let Some(cached) = self
                .cache
                .lock()
                .await
                .get(&input.id)
                .filter(|c| c.config == config)
        {
            return Ok(ListTextGeneratorModelsResult {
                models: cached.models.clone(),
                tested_model: None,
            });
        }
        let key = self.key(&config, &settings).await?;
        let models = tokio::time::timeout(Duration::from_secs(25), self.models(&config, &key))
            .await
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Network,
            ))??;
        if test {
            let explicit = input.model.flatten();
            let model = explicit
                .as_ref()
                .map(|s| s.0.as_str())
                .or_else(|| {
                    models
                        .iter()
                        .find(|m| {
                            ["haiku", "mini", "flash", "nano"]
                                .iter()
                                .any(|needle| m.id.0.to_lowercase().contains(needle))
                        })
                        .or_else(|| models.first())
                        .map(|m| m.id.0.as_str())
                })
                .ok_or(ServiceError::Unavailable)?;
            let body =
                json!({"model":model,"messages":[{"role":"user","content":"Reply with OK."}]});
            let mut request = Self::request(
                &config,
                &key,
                if config.kind == TextGeneratorKind::Anthropic {
                    "/messages"
                } else {
                    "/chat/completions"
                },
                Some(body),
            );
            if config.kind == TextGeneratorKind::Anthropic {
                let mut body: Value = serde_json::from_str(
                    request
                        .body
                        .as_ref()
                        .ok_or(ServiceError::Response)?
                        .expose(),
                )
                .map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Response,
                ))?;
                body["max_tokens"] = json!(8192);
                request.body = Some(Sensitive::new(body.to_string()));
            }
            let response = tokio::time::timeout(Duration::from_secs(120), self.json(request))
                .await
                .map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Network,
                ))??;
            let valid = if config.kind == TextGeneratorKind::Anthropic {
                response["stop_reason"].as_str() != Some("max_tokens")
                    && response["content"].as_array().is_some_and(|a| {
                        a.iter().any(|v| {
                            v["type"] == "text"
                                && v["text"].as_str().is_some_and(|s| !s.trim().is_empty())
                        })
                    })
            } else {
                response["choices"][0]["finish_reason"].as_str() != Some("length")
                    && response["choices"][0]["message"]["content"]
                        .as_str()
                        .is_some_and(|s| !s.trim().is_empty())
            };
            if !valid {
                return Err(ServiceError::Response);
            }
            return Ok(ListTextGeneratorModelsResult {
                models: models.clone(),
                tested_model: Some(Some(model.to_owned().into())),
            });
        }
        self.cache.lock().await.insert(
            input.id,
            Cached {
                config,
                models: models.clone(),
            },
        );
        Ok(ListTextGeneratorModelsResult {
            models,
            tested_model: None,
        })
    }
}
impl std::fmt::Debug for TextGenerators {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TextGenerators([REDACTED])")
    }
}
