use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use t3_textgen::{
    CancellationToken,
    api::{ApiTextRunner, GeneratorSecrets, ModelCache},
    generation::GenerationError,
    models::{ModelStore, SpeechModel},
    speech::SpeechError,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

type Response = (u16, Vec<(String, String)>, Vec<u8>);
async fn server(responses: Vec<Response>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let origin = format!("http://{}", listener.local_addr().expect("address"));
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, headers, body) in responses {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).await.expect("headers");
                request.push(byte[0]);
                assert!(request.len() < 100000);
            }
            let header = String::from_utf8(request.clone()).expect("header utf8");
            let length = header
                .lines()
                .find_map(|line| {
                    line.to_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            let mut bytes = vec![0; length];
            stream.read_exact(&mut bytes).await.expect("body");
            request.extend(bytes);
            requests.push(String::from_utf8_lossy(&request).into_owned());
            let mut response = format!("HTTP/1.1 {status} Test\r\nConnection: close\r\n");
            if !headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            {
                response.push_str(&format!("Content-Length: {}\r\n", body.len()));
            }
            for (key, value) in headers {
                response.push_str(&format!("{key}: {value}\r\n"));
            }
            response.push_str("\r\n");
            stream
                .write_all(response.as_bytes())
                .await
                .expect("response");
            stream.write_all(&body).await.expect("response body");
            stream.shutdown().await.expect("shutdown");
        }
        requests
    });
    (origin, task)
}
struct Secrets;
#[async_trait]
impl GeneratorSecrets for Secrets {
    async fn resolve(
        &self,
        _: &t3_contracts::settings_secret_value::SettingsSecretValue,
    ) -> Result<String, GenerationError> {
        Ok("test-key".into())
    }
}
fn runner(origin: &str, kind: &str) -> ApiTextRunner {
    ApiTextRunner::new(
        serde_json::from_value(
            serde_json::json!({"kind":kind,"baseUrl":origin,"apiKey":"test-key","enabled":true}),
        )
        .expect("config"),
        Arc::new(Secrets),
    )
    .expect("runner")
}

#[tokio::test]
async fn configured_apis_generate_and_paginate_with_the_exact_request_shapes() {
    let (url,task)=server(vec![(200,vec![],br#"{"choices":[{"finish_reason":"stop","message":{"content":"  {\"title\":\"Search\"}  "}}]}"#.to_vec())]).await;
    assert_eq!(
        runner(&url, "openaiCompatible")
            .generate_text("mini", "private prompt")
            .await
            .expect("generate"),
        "{\"title\":\"Search\"}"
    );
    let requests = task.await.expect("server");
    assert!(requests[0].starts_with("POST /chat/completions"));
    assert!(
        requests[0]
            .to_lowercase()
            .contains("authorization: bearer test-key")
    );
    assert!(requests[0].contains("\"role\":\"user\""));
    let (url,task)=server(vec![(200,vec![],br#"{"data":[{"id":"b","display_name":"Bee"}],"has_more":true,"last_id":"b"}"#.to_vec()),(200,vec![],br#"{"data":[{"id":"a"},{"id":"b","display_name":"Updated"}],"has_more":true,"last_id":"b"}"#.to_vec())]).await;
    let models = runner(&url, "anthropic").list_models().await.expect("list");
    assert_eq!(
        models.iter().map(|m| m.id.0.as_str()).collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert_eq!(models[1].name.0, "Updated");
    let requests = task.await.expect("server");
    assert!(requests[1].contains("after_id=b"));
    assert!(requests[0].contains("anthropic-version: 2023-06-01"));
}

#[tokio::test]
async fn model_cache_reaches_the_api_for_explicit_tests_and_does_not_expose_error_bodies() {
    let (url, task) = server(vec![
        (
            200,
            vec![],
            br#"{"data":[{"id":"big"},{"id":"mini"}]}"#.to_vec(),
        ),
        (200, vec![], br#"{"data":[{"id":"mini"}]}"#.to_vec()),
        (
            200,
            vec![],
            br#"{"choices":[{"finish_reason":"stop","message":{"content":"OK"}}]}"#.to_vec(),
        ),
    ])
    .await;
    let runner = runner(&url, "openaiCompatible");
    let cache = ModelCache::default();
    let first = cache
        .list("one", &runner, false, false, None, true)
        .await
        .expect("first");
    assert_eq!(
        cache
            .list("one", &runner, false, false, None, true)
            .await
            .expect("cached")
            .0,
        first.0
    );
    let tested = cache
        .list("one", &runner, false, false, Some(None), true)
        .await
        .expect("test");
    assert_eq!(tested.1.as_deref(), Some("mini"));
    let requests = task.await.expect("server");
    assert_eq!(requests.len(), 3);
    assert!(requests[2].contains("Reply with OK."));
    let (url, task) = server(vec![(
        429,
        vec![],
        br#"{"error":{"message":"secret prompt and private-token"}}"#.to_vec(),
    )])
    .await;
    let error = self::runner(&url, "openaiCompatible")
        .generate_text("mini", "private code")
        .await
        .expect_err("error");
    assert_eq!(error, GenerationError::RateLimit);
    assert!(!error.to_string().contains("private"));
    task.await.expect("server");
}

fn model(bytes: &[u8]) -> SpeechModel {
    SpeechModel {
        id: "test/model".into(),
        revision: "pinned".into(),
        name: "Test".into(),
        description: String::new(),
        filename: "test.gguf".into(),
        size: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(bytes)),
        license: "MIT".into(),
        languages: vec!["en".into()],
        benchmark: serde_json::Value::Null,
        recommended: false,
        supports_streaming: false,
        supports_language_detection: false,
        supports_translation: false,
    }
}
fn store(root: &std::path::Path) -> ModelStore {
    ModelStore::new(root, root.join("home"), "win32".into()).expect("store")
}

#[tokio::test]
async fn model_download_resumes_and_verifies_before_replacing_an_existing_file() {
    let root = tempfile::tempdir().expect("temp");
    let store = store(root.path());
    let bytes = b"abcdefgh";
    let model = model(bytes);
    let (url, task) = server(vec![
        (
            200,
            vec![("Content-Length".into(), "8".into())],
            b"abcd".to_vec(),
        ),
        (
            206,
            vec![("Content-Range".into(), "bytes 4-7/8".into())],
            b"efgh".to_vec(),
        ),
    ])
    .await;
    let path = store
        .download_from(&model, &url, &(), CancellationToken::new())
        .await
        .expect("resumed");
    assert_eq!(std::fs::read(&path).expect("file"), bytes);
    assert!(store.installed_path(&model).is_some());
    let requests = task.await.expect("server");
    assert!(requests[1].to_lowercase().contains("range: bytes=4-"));
    let (url, task) = server(vec![(200, vec![], b"corrupt!".to_vec())]).await;
    assert_eq!(
        store
            .download_from(&model, &url, &(), CancellationToken::new())
            .await,
        Err(SpeechError::Verification)
    );
    assert_eq!(std::fs::read(path).expect("retained"), bytes);
    task.await.expect("server");
    assert!(
        std::fs::read_dir(root.path().join("speech/models"))
            .expect("directory")
            .flatten()
            .all(|e| !e.file_name().to_string_lossy().ends_with(".part"))
    );
}

#[test]
fn linking_and_removal_preserve_external_models_and_restart_only_cleans_owned_partials() {
    let root = tempfile::tempdir().expect("temp");
    let store = store(root.path());
    let bytes = b"model bytes";
    let model = model(bytes);
    let external = root.path().join("external.gguf");
    std::fs::write(&external, bytes).expect("external");
    assert_eq!(
        store
            .link(std::slice::from_ref(&model), &external)
            .expect("link"),
        vec!["test/model"]
    );
    assert!(store.installed_path(&model).is_some());
    assert_eq!(
        store
            .linked_from(&model)
            .and_then(|p| std::fs::canonicalize(p).ok()),
        std::fs::canonicalize(&external).ok()
    );
    store.remove(&model).expect("remove");
    assert_eq!(std::fs::read(&external).expect("retained"), bytes);
    let directory = root.path().join("speech/models");
    for name in [
        "test.gguf.12345678-1234-4567-89ab-123456789abc.part",
        "test.gguf",
        "personal.part",
    ] {
        std::fs::write(directory.join(name), b"keep").expect("file");
    }
    let _store = self::store(root.path());
    assert!(
        !directory
            .join("test.gguf.12345678-1234-4567-89ab-123456789abc.part")
            .exists()
    );
    assert!(directory.join("test.gguf").exists());
    assert!(directory.join("personal.part").exists());
    let mut invalid = model;
    invalid.filename = "../outside".into();
    assert_eq!(store.path(&invalid), Err(SpeechError::ModelNotFound));
}
