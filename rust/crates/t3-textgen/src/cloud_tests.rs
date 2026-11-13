use super::*;
use std::sync::{
    Mutex as StdMutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
struct Credentials {
    source: StdMutex<Option<SettingsSecretValue>>,
    remove_on_resolve: AtomicBool,
}
#[async_trait]
impl CloudCredentials for Credentials {
    async fn save(
        &self,
        _: CloudSpeechProvider,
        value: Option<SettingsSecretValue>,
    ) -> Result<(), ()> {
        *self.source.lock().expect("source") = value;
        Ok(())
    }
    async fn current(&self, _: CloudSpeechProvider) -> Result<Option<SettingsSecretValue>, ()> {
        Ok(self.source.lock().expect("source").clone())
    }
    async fn resolve_fresh(&self, _: &SettingsSecretValue) -> Result<String, ()> {
        if self.remove_on_resolve.load(Ordering::SeqCst) {
            *self.source.lock().expect("source") = None;
        }
        Ok("saved-test-key".into())
    }
}
fn credentials() -> Arc<Credentials> {
    Arc::new(Credentials {
        source: StdMutex::new(Some(
            serde_json::from_value(json!("saved-test-key")).expect("source"),
        )),
        remove_on_resolve: AtomicBool::new(false),
    })
}
async fn server(responses: Vec<&'static str>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let origin = format!("http://{}", listener.local_addr().expect("address"));
    let handle = tokio::spawn(async move {
        let mut captured = Vec::new();
        for response in responses {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut header = Vec::new();
            let mut byte = [0];
            while !header.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).await.expect("header");
                header.push(byte[0]);
            }
            let text = String::from_utf8_lossy(&header);
            let length = text
                .lines()
                .find_map(|line| {
                    line.to_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            let mut body = vec![0; length];
            socket.read_exact(&mut body).await.expect("body");
            captured.push(format!("{text}{}", String::from_utf8_lossy(&body)));
            let reply = format!(
                "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{response}",
                response.len()
            );
            socket.write_all(reply.as_bytes()).await.expect("response");
        }
        captured
    });
    (origin, handle)
}
#[tokio::test]
async fn cloud_uploads_use_saved_keys_exact_models_wav_and_zero_retention() {
    for provider in [
        CloudSpeechProvider::Openai,
        CloudSpeechProvider::Xai,
        CloudSpeechProvider::Openrouter,
    ] {
        let responses = if provider == CloudSpeechProvider::Openrouter {
            vec![
                r#"{"data":[{"id":"acme/voice","name":"Voice"},{"id":"openrouter/auto"}]}"#,
                r#"{"text":"Robert"}"#,
            ]
        } else {
            vec![r#"{"text":"Robert"}"#]
        };
        let (url, task) = server(responses).await;
        let mut service = CloudSpeech::new(credentials()).expect("service");
        service.endpoints = [url.clone(), url.clone(), url.clone()];
        service.catalog_url = format!("{url}/models");
        let options:SpeechTranscriptionOptions=serde_json::from_value(json!({"speechLanguage":"fr","speechTranslateToEnglish":true,"speechCustomWords":[{"term":"Bob","aliases":["Robert"]}],"speechRemoveFillerWords":true,"speechSilenceFiltering":false})).expect("options");
        assert_eq!(
            service
                .transcribe(
                    provider.clone(),
                    &0.25f32.to_le_bytes(),
                    &options,
                    Some("acme/voice"),
                    CancellationToken::new()
                )
                .await
                .expect("transcribe"),
            "Bob"
        );
        let requests = task.await.expect("server");
        let upload = requests.last().expect("upload");
        assert!(
            upload
                .to_lowercase()
                .contains("authorization: bearer saved-test-key")
        );
        match provider {
            CloudSpeechProvider::Openrouter => {
                let body: Value =
                    serde_json::from_str(upload.split_once("\r\n\r\n").expect("body").1)
                        .expect("json");
                assert_eq!(body["model"], "acme/voice");
                assert_eq!(body["provider"]["zdr"], true);
                assert_eq!(body["language"], "fr");
                let wave = STANDARD
                    .decode(body["input_audio"]["data"].as_str().expect("data"))
                    .expect("base64");
                assert_eq!(&wave[..4], b"RIFF");
                assert_eq!(wave.len(), 46);
            }
            CloudSpeechProvider::Openai => {
                assert!(upload.contains("gpt-transcribe"));
                assert!(upload.contains("name=\"languages[]\""));
                assert!(upload.contains("name=\"keywords[]\""));
                assert!(!upload.contains("speechTranslateToEnglish"));
            }
            CloudSpeechProvider::Xai => {
                assert!(upload.contains("grok-voice-transcribe-2.0"));
                assert!(upload.contains("name=\"keyterm\""));
                assert!(
                    upload.find("name=\"filler_words\"").expect("filler")
                        < upload.find("name=\"file\"").expect("file")
                );
                assert!(upload.contains("name=\"vad_threshold\""));
            }
        }
    }
}
#[tokio::test]
async fn credential_removal_prevents_an_upload_and_invalid_models_fail_before_network() {
    let credentials = credentials();
    credentials.remove_on_resolve.store(true, Ordering::SeqCst);
    let service = CloudSpeech::new(credentials).expect("service");
    let options:SpeechTranscriptionOptions=serde_json::from_value(json!({"speechLanguage":"en","speechTranslateToEnglish":false,"speechCustomWords":[],"speechRemoveFillerWords":false})).expect("options");
    let result = service
        .transcribe(
            CloudSpeechProvider::Openai,
            &[0; 4],
            &options,
            None,
            CancellationToken::new(),
        )
        .await;
    assert!(matches!(
        result,
        Err(CloudTranscriptionError::Cloud(CloudError {
            reason: CloudSpeechErrorReason::MissingKey,
            ..
        }))
    ));
    for model in [
        "openrouter/auto",
        "acme/voice:nitro",
        "https://example.com",
        "acme",
    ] {
        assert!(!valid_model_id(model));
    }
    assert!(!valid_key("https://example.com/key"));
    assert!(!valid_key("key with spaces"));
    assert!(valid_key("sk-valid"));
    let wave = wave(&[-1.0, 0.0, 1.0]).expect("wave");
    assert_eq!(&wave[44..], &[0, 128, 0, 0, 255, 127]);
}
