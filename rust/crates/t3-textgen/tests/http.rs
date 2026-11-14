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

/// A listener that answers every request with `body` and records what it received.
async fn recorder(body: &'static [u8]) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let origin = format!("http://{}", listener.local_addr().expect("address"));
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut request = vec![0; 64 * 1024];
            let read = stream.read(&mut request).await.unwrap_or(0);
            log.lock()
                .expect("log")
                .push(String::from_utf8_lossy(&request[..read]).into_owned());
            let response = format!(
                "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.write_all(body).await;
        }
    });
    (origin, seen)
}

#[tokio::test]
async fn a_redirect_to_another_listener_never_receives_the_key() {
    let (elsewhere, seen) = recorder(
        br#"{"choices":[{"finish_reason":"stop","message":{"content":"stolen"}}],"data":[]}"#,
    )
    .await;
    for path in ["/chat/completions", "/models"] {
        let (url, task) = server(vec![(
            307,
            vec![("Location".into(), format!("{elsewhere}{path}"))],
            Vec::new(),
        )])
        .await;
        let runner = runner(&url, "openaiCompatible");
        let result = if path == "/models" {
            runner.list_models().await.map(|_| String::new())
        } else {
            runner.generate_text("mini", "private prompt").await
        };
        assert_eq!(result, Err(GenerationError::Provider), "{path}");
        assert!(task.await.expect("server")[0].contains("test-key"));
    }
    assert!(seen.lock().expect("log").is_empty());
}

#[tokio::test]
async fn cancellation_stops_a_request_in_flight() {
    use t3_textgen::generation::{GenerationRequest, Operation, TextRunner};
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("address"));
    let (received, arrived) = tokio::sync::oneshot::channel();
    let held = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");
        let mut buffer = vec![0; 64 * 1024];
        let _ = stream.read(&mut buffer).await;
        let _ = received.send(());
        // Never answers: the read ends only when the client drops the connection.
        while matches!(stream.read(&mut buffer).await, Ok(n) if n > 0) {}
    });
    let runner = runner(&url, "openaiCompatible");
    let request = GenerationRequest {
        operation: Operation::GenerateThreadTitle,
        cwd: std::path::PathBuf::from("checkout"),
        model_selection: serde_json::from_value(
            serde_json::json!({"instanceId":"api","model":"mini"}),
        )
        .expect("selection"),
        prompt: "private prompt".into(),
        output_schema: serde_json::Value::Null,
        attachments: Vec::new(),
        investigation_timeout: None,
    };
    let token = CancellationToken::new();
    let cancel = token.clone();
    let (result, ()) = tokio::join!(runner.run(&request, token), async move {
        arrived.await.expect("request reached the API");
        cancel.cancel();
    });
    assert_eq!(result, Err(GenerationError::Cancelled));
    held.await.expect("connection closed by the client");
}

#[tokio::test]
async fn reply_shapes_map_to_the_typescript_messages() {
    let cases: [(&str, &[u8], GenerationError); 6] = [
        (
            "openaiCompatible",
            br#"{"choices":[]}"#,
            GenerationError::EmptyOutput,
        ),
        (
            "openaiCompatible",
            br#"{"choices":[{"finish_reason":"stop","message":{"content":null}}]}"#,
            GenerationError::EmptyOutput,
        ),
        (
            "openaiCompatible",
            br#"{"choices":[{"finish_reason":"stop","message":{}}]}"#,
            GenerationError::UnreadableReply,
        ),
        (
            "openaiCompatible",
            br#"{"choices":[{"message":{"content":"text"}}]}"#,
            GenerationError::UnreadableReply,
        ),
        (
            "anthropic",
            br#"{"content":[{"type":"text","text":"x"}]}"#,
            GenerationError::UnreadableReply,
        ),
        ("anthropic", b"not json", GenerationError::UnreadableReply),
    ];
    for (kind, body, expected) in cases {
        let (url, task) = server(vec![(200, vec![], body.to_vec())]).await;
        let result = runner(&url, kind).generate_text("mini", "prompt").await;
        assert_eq!(
            result,
            Err(expected),
            "{kind} {}",
            String::from_utf8_lossy(body)
        );
        task.await.expect("server");
    }
    assert_eq!(
        GenerationError::UnreadableReply.to_string(),
        "The model API returned a reply T3 Code couldn't read."
    );
    // JavaScript's trim removes a byte-order mark and keeps U+0085.
    let (url, task) = server(vec![(
        200,
        vec![],
        "{\"choices\":[{\"finish_reason\":\"stop\",\"message\":{\"content\":\"\u{feff}\u{85}OK\u{feff}\"}}]}"
            .as_bytes()
            .to_vec(),
    )])
    .await;
    assert_eq!(
        runner(&url, "openaiCompatible")
            .generate_text("mini", "prompt")
            .await,
        Ok("\u{85}OK".into())
    );
    task.await.expect("server");
}

#[test]
fn a_redacted_key_is_substituted_only_for_the_saved_kind_and_origin() {
    use t3_contracts::{
        base_schemas::TrimmedNonEmptyString, settings::TextGeneratorConfig,
        settings_secret_value::SettingsSecretValue,
    };
    use t3_textgen::api::listing_config;
    const SENTINEL: &str = "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}";
    // Built directly: decoding would already refuse some of these addresses.
    let config = |kind: &str, url: &str, key: serde_json::Value| TextGeneratorConfig {
        kind: serde_json::from_value(kind.into()).expect("kind"),
        label: None,
        base_url: TrimmedNonEmptyString(url.into()),
        api_key: serde_json::from_value(key).expect("key"),
        enabled: true,
    };
    let reference = |path: &str| serde_json::json!({"provider": "1password", "reference": path});
    let saved = config(
        "openaiCompatible",
        "https://api.example.com/v1",
        "saved-key".into(),
    );
    let key = |supplied: &TextGeneratorConfig| {
        listing_config(Some(supplied), Some(&saved), SENTINEL).map(|c| c.api_key)
    };
    let saved_key = Ok(SettingsSecretValue::Literal("saved-key".into()));
    // The saved address, spelled differently.
    for same in [
        "https://API.example.com:443/v1/",
        "https://api.example.com/v1",
    ] {
        assert_eq!(
            key(&config("openaiCompatible", same, SENTINEL.into())),
            saved_key,
            "{same}"
        );
    }
    // A stored secret never goes elsewhere: not for the placeholder, nor for a reference.
    let saved_reference = config(
        "openaiCompatible",
        "https://api.example.com/v1",
        reference("op://Private/api/key"),
    );
    for stolen in [
        config(
            "openaiCompatible",
            "https://attacker.example/v1",
            SENTINEL.into(),
        ),
        config(
            "openaiCompatible",
            "https://api.example.com/v2",
            SENTINEL.into(),
        ),
        config(
            "openaiCompatible",
            "http://api.example.com/v1",
            SENTINEL.into(),
        ),
        config(
            "openaiCompatible",
            "https://api.example.com:8443/v1",
            SENTINEL.into(),
        ),
        config("anthropic", "https://api.example.com/v1", SENTINEL.into()),
        config(
            "openaiCompatible",
            "https://attacker.example/v1",
            reference("op://Private/other/password"),
        ),
        config(
            "openaiCompatible",
            "https://attacker.example/v1",
            reference("op://Private/api/key"),
        ),
        config(
            "openaiCompatible",
            "https://api.example.com/v1",
            reference("op://Private/other/password"),
        ),
        config(
            "anthropic",
            "https://api.example.com/v1",
            reference("op://Private/api/key"),
        ),
    ] {
        for saved in [&saved, &saved_reference] {
            assert_eq!(
                listing_config(Some(&stolen), Some(saved), SENTINEL).map(|c| c.api_key),
                Err(GenerationError::KeyRequired),
                "{} {:?} against {:?}",
                stolen.base_url.0,
                stolen.api_key,
                saved.api_key
            );
        }
        assert_eq!(
            listing_config(Some(&stolen), None, SENTINEL).map(|c| c.api_key),
            Err(GenerationError::KeyRequired)
        );
    }
    assert_eq!(
        listing_config(Some(&saved_reference), Some(&saved_reference), SENTINEL),
        Ok(saved_reference.clone())
    );
    // A freshly typed key goes to the saved address or an official endpoint only.
    for (url, allowed) in [
        ("https://api.example.com/v1/", true),
        ("https://api.openai.com/v1", true),
        ("https://attacker.example/v1", false),
        ("https://user:pass@api.example.com/v1", false),
        ("https://api.example.com/v1#fragment", false),
        ("not a url", false),
    ] {
        let fresh = config("openaiCompatible", url, "typed-key".into());
        assert_eq!(
            listing_config(Some(&fresh), Some(&saved), SENTINEL),
            if allowed {
                Ok(fresh.clone())
            } else {
                Err(GenerationError::EndpointRefused)
            },
            "{url}"
        );
    }
    assert_eq!(
        listing_config(None, Some(&saved), SENTINEL),
        Ok(saved.clone())
    );
    assert_eq!(
        listing_config(None, None, SENTINEL),
        Err(GenerationError::Unavailable)
    );
    assert_eq!(
        GenerationError::EndpointRefused.to_string(),
        "The text generator endpoint is not configured on this server."
    );
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
