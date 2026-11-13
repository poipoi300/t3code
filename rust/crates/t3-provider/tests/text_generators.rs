#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{collections::VecDeque, sync::Arc};
use t3_contracts::settings::*;
use t3_provider::{
    secrets::{OnePassword, SecretResolver},
    sensitive::*,
    text_generators::*,
    transport::*,
};
use tokio::sync::Mutex;
struct Settings(Mutex<ServerSettings>);
#[async_trait]
impl SettingsSource for Settings {
    async fn settings(&self) -> ServiceResult<ServerSettings> {
        Ok(self.0.lock().await.clone())
    }
}
struct Http {
    replies: Mutex<VecDeque<Value>>,
    calls: Mutex<Vec<HttpRequest>>,
}
#[async_trait]
impl HttpTransport for Http {
    async fn send(&self, request: HttpRequest) -> ServiceResult<HttpResponse> {
        self.calls.lock().await.push(request);
        Ok(HttpResponse {
            status: 200,
            body: Sensitive::new(
                self.replies
                    .lock()
                    .await
                    .pop_front()
                    .expect("Unexpected HTTP request"),
            ),
        })
    }
}
fn setup(kind: &str, replies: Vec<Value>) -> (TextGenerators, Arc<Http>, Arc<Settings>) {
    let settings:ServerSettings=serde_json::from_value(json!({"textGenerators":{"generator":{"kind":kind,"baseUrl":"https://provider.invalid/v1/","apiKey":"secret-never-log"}}})).unwrap();
    let settings = Arc::new(Settings(Mutex::new(settings)));
    let http = Arc::new(Http {
        replies: Mutex::new(replies.into()),
        calls: Mutex::new(vec![]),
    });
    let service = TextGenerators::new(
        settings.clone(),
        Arc::new(SecretResolver::new(OnePassword::with_executable(
            "must-not-run".into(),
            Default::default(),
        ))),
        http.clone(),
        false,
    );
    (service, http, settings)
}
fn input(value: Value) -> ListTextGeneratorModelsInput {
    serde_json::from_value(value).unwrap()
}
#[tokio::test]
async fn openai_model_cache_is_bound_to_the_entire_config_and_redacted_drafts_reuse_the_saved_secret()
 {
    let (service, http, settings) = setup(
        "openaiCompatible",
        vec![
            json!({"data":[{"id":"z"},{"id":"a","name":"Model A"}]}),
            json!({"data":[{"id":"fresh"}]}),
            json!({"data":[{"id":"draft"}]}),
        ],
    );
    let first = service
        .list_models(input(json!({"id":"generator"})))
        .await
        .unwrap();
    assert_eq!(first.models[0].name.0, "Model A");
    service
        .list_models(input(json!({"id":"generator"})))
        .await
        .unwrap();
    assert_eq!(http.calls.lock().await.len(), 1);
    settings
        .0
        .lock()
        .await
        .text_generators
        .get_mut(&"generator".into())
        .unwrap()
        .base_url = "https://other.invalid/v1".into();
    assert_eq!(
        service
            .list_models(input(json!({"id":"generator"})))
            .await
            .unwrap()
            .models[0]
            .id
            .0,
        "fresh"
    );
    service.list_models(input(json!({"id":"generator","config":{"kind":"openaiCompatible","baseUrl":"https://draft.invalid/v1","apiKey":t3_config::secrets::REDACTED}}))).await.unwrap();
    let calls = http.calls.lock().await;
    assert_eq!(calls[0].url, "https://provider.invalid/v1/models");
    assert_eq!(
        calls[2].headers["authorization"].expose(),
        "Bearer secret-never-log"
    );
    assert!(!format!("{:?}", calls[2]).contains("secret-never-log"));
    assert!(!format!("{:?}", settings.0.lock().await.text_generators).contains("secret-never-log"));
}
#[tokio::test]
async fn anthropic_pagination_deduplicates_records_and_a_test_probe_uses_the_selected_model_without_caching()
 {
    let (service, http, _) = setup(
        "anthropic",
        vec![
            json!({"data":[{"id":"claude-opus","display_name":"Opus"}],"has_more":true,"last_id":"page-a"}),
            json!({"data":[{"id":"claude-opus","display_name":"Updated Opus"},{"id":"claude-haiku","display_name":"Haiku"}],"has_more":false}),
            json!({"content":[{"type":"text","text":"OK"}],"stop_reason":"end_turn"}),
            json!({"data":[{"id":"fresh"}],"has_more":false}),
        ],
    );
    let result = service
        .list_models(input(json!({"id":"generator","test":true})))
        .await
        .unwrap();
    assert_eq!(result.models.len(), 2);
    assert_eq!(result.tested_model.unwrap().unwrap().0, "claude-haiku");
    assert_eq!(result.models[1].name.0, "Updated Opus");
    let calls = http.calls.lock().await;
    assert!(calls[1].url.ends_with("limit=1000&after_id=page-a"));
    assert_eq!(calls[2].url, "https://provider.invalid/v1/messages");
    assert_eq!(calls[2].headers["x-api-key"].expose(), "secret-never-log");
    assert_eq!(
        serde_json::from_str::<Value>(calls[2].body.as_ref().unwrap().expose()).unwrap()["model"],
        "claude-haiku"
    );
    drop(calls);
    assert_eq!(
        service
            .list_models(input(json!({"id":"generator"})))
            .await
            .unwrap()
            .models[0]
            .id
            .0,
        "fresh"
    );
}
