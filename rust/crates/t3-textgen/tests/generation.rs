use async_trait::async_trait;
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use t3_textgen::{CancellationToken, generation::*, post_processing::post_process, prompts::*};

struct FakeRunner {
    requests: Mutex<Vec<(Operation, String, PathBuf)>>,
    reply: String,
    started: tokio::sync::Notify,
    hang: bool,
}
#[async_trait]
impl TextRunner for FakeRunner {
    async fn run(
        &self,
        request: &GenerationRequest,
        _cancellation: CancellationToken,
    ) -> Result<String, GenerationError> {
        self.requests.lock().expect("requests").push((
            request.operation,
            request.prompt.clone(),
            request.cwd.clone(),
        ));
        self.started.notify_one();
        if self.hang {
            std::future::pending().await
        } else {
            Ok(self.reply.clone())
        }
    }
}
fn context() -> GenerationContext {
    GenerationContext {
        cwd: PathBuf::from("private-checkout"),
        model_selection: serde_json::from_value(json!({"instanceId":"codex","model":"requested"}))
            .expect("selection"),
        project_id: None,
    }
}
fn runner(reply: &str, hang: bool) -> Arc<FakeRunner> {
    Arc::new(FakeRunner {
        requests: Mutex::new(Vec::new()),
        reply: reply.into(),
        started: tokio::sync::Notify::new(),
        hang,
    })
}
fn provider(id: &str, driver: &str) -> ProviderTarget {
    ProviderTarget {
        instance_id: id.into(),
        driver: driver.into(),
        enabled: true,
        project_enabled: true,
        installed: true,
        unauthenticated: false,
        probe_error: false,
        available: true,
        text_capable: true,
        text_generator: false,
        unsupported_models: Vec::new(),
        exhausted_models: Vec::new(),
        exhausted: false,
        custom_models: Vec::new(),
    }
}

#[tokio::test]
async fn title_links_deduplicate_cap_lookups_and_preserve_the_typescript_json_order() {
    use t3_textgen::title_context::{LinkedSubject, TitleLinkResolver, resolve_title_links};
    struct Links(Mutex<Vec<String>>);
    #[async_trait]
    impl TitleLinkResolver for Links {
        fn supports(&self, _: &std::path::Path, url: &reqwest::Url) -> bool {
            url.host_str() == Some("forge.example")
        }
        async fn resolve(
            &self,
            _: &std::path::Path,
            url: &reqwest::Url,
        ) -> Result<LinkedSubject, ()> {
            self.0.lock().expect("lookups").push(url.to_string());
            if url.path().ends_with("missing") {
                Err(())
            } else {
                Ok(LinkedSubject {
                    title: "Pairing failed".into(),
                    body: Some("Reconnect safely".into()),
                })
            }
        }
    }
    let links = Links(Mutex::new(Vec::new()));
    let result=resolve_title_links("https://unrelated.example/skip https://forge.example/issue?view=1#top, https://forge.example/issue https://forge.example/missing https://forge.example/ignored",std::path::Path::new("checkout"),&links,CancellationToken::new()).await.expect("links");
    assert_eq!(
        result,
        "https://forge.example/issue\n{\"title\":\"Pairing failed\",\"body\":\"Reconnect safely\"}\n\nhttps://forge.example/missing: unavailable"
    );
    assert_eq!(links.0.lock().expect("lookups").len(), 2);
}

#[tokio::test]
async fn fake_runner_parses_structured_results_and_never_exposes_reply_errors() {
    let runner = runner(
        "prefix ```json\n{\"subject\":\"Add search...\",\"body\":\"  Tests passed  \",\"branch\":\"Add search\"}\n```",
        false,
    );
    let service = TextGeneration::new(runner.clone());
    let generated = service
        .generate(
            context(),
            GenerationInput::Commit(CommitInput {
                include_branch: true,
                ..Default::default()
            }),
            None,
            CancellationToken::new(),
        )
        .await
        .expect("commit");
    assert_eq!(
        generated,
        Generated::Commit {
            subject: "Add search".into(),
            body: "Tests passed".into(),
            branch: Some("feature/add-search".into())
        }
    );
    assert_eq!(runner.requests.lock().expect("requests").len(), 1);
    for reply in [
        "",
        "I refuse: access_token=private-token",
        "{\"wrong\":\"private-code\"}",
    ] {
        let service = TextGeneration::new(self::runner(reply, false));
        let error = service
            .generate(
                context(),
                GenerationInput::ThreadTitle(ThreadTitleInput::default()),
                None,
                CancellationToken::new(),
            )
            .await
            .expect_err("invalid");
        assert_eq!(error, GenerationError::InvalidOutput);
        assert!(!format!("{error:?} {error}").contains("private"));
    }
}

#[tokio::test(start_paused = true)]
async fn timeout_and_cancellation_drop_in_flight_generation() {
    for cancel in [false, true] {
        let runner = runner("", true);
        let service = Arc::new(TextGeneration::new(runner.clone()));
        let token = CancellationToken::new();
        let child = token.clone();
        let task = tokio::spawn(async move {
            service
                .generate(
                    context(),
                    GenerationInput::ThreadTitle(ThreadTitleInput::default()),
                    None,
                    child,
                )
                .await
        });
        runner.started.notified().await;
        if cancel {
            token.cancel();
        } else {
            tokio::time::advance(std::time::Duration::from_secs(180)).await;
        }
        assert_eq!(
            task.await.expect("join"),
            Err(if cancel {
                GenerationError::Cancelled
            } else {
                GenerationError::Timeout
            })
        );
    }
}

#[test]
fn fallback_respects_probe_state_project_limits_driver_order_and_custom_models() {
    let selected = context().model_selection;
    let mut codex = provider("codex", "codex");
    codex.probe_error = true;
    codex.installed = false;
    let mut custom = provider("work-claude", "claudeAgent");
    custom.custom_models = vec!["my-haiku".into()];
    let cursor = provider("cursor", "cursor");
    let mut routing = RoutingState {
        providers: vec![custom.clone(), cursor.clone(), codex],
        default_selection: Some(
            serde_json::from_value(
                json!({"instanceId":"work-claude","model":"my-haiku","options":{"effort":"low"}}),
            )
            .expect("custom"),
        ),
    };
    assert_eq!(
        routing
            .resolve(&selected, Operation::GenerateThreadTitle)
            .expect("fallback")
            .instance_id
            .0,
        "cursor"
    );
    routing.providers[1].project_enabled = false;
    assert_eq!(
        routing
            .resolve(&selected, Operation::GenerateThreadTitle)
            .expect("fallback")
            .model,
        "my-haiku"
    );
    routing.providers[0].exhausted_models = vec!["my-haiku".into()];
    assert_eq!(
        routing
            .resolve(&selected, Operation::GenerateThreadTitle)
            .expect("fallback")
            .model,
        "claude-haiku-5-5"
    );
    routing.providers[0].exhausted = true;
    assert_eq!(
        routing
            .resolve(&selected, Operation::GenerateThreadTitle)
            .expect("no alternative"),
        selected
    );
    let mut pending = provider("codex", "codex");
    pending.installed = false;
    routing.providers = vec![pending, cursor];
    assert_eq!(
        routing
            .resolve(&selected, Operation::GenerateThreadTitle)
            .expect("pending probe"),
        selected
    );
    routing.providers[0].text_generator = true;
    routing.providers[0].enabled = false;
    assert_eq!(
        routing.resolve(&selected, Operation::GenerateThreadTitle),
        Err(GenerationError::Disabled)
    );
    assert_eq!(
        routing.resolve(&selected, Operation::GenerateReviewGuide),
        Err(GenerationError::InvestigationUnsupported)
    );
}

#[tokio::test]
async fn voice_cleanup_uses_an_empty_directory_preserves_blank_output_and_applies_aliases() {
    for reply in ["{\"transcription\":\"\"}", "{\"transcription\":\"Robert\"}"] {
        let runner = runner(reply, false);
        let service = TextGeneration::new(runner.clone());
        let input=serde_json::from_value(json!({"options":{"speechCustomWords":[{"term":"Bob","aliases":["Robert"]}],"speechPostProcessingPrompt":{"mode":"default","customInstructions":"ignored"}},"transcript":"original speech","draft":{"text":"before after","selection":{"start":7,"end":7}}})).expect("cleanup");
        let result = post_process(&service, context(), input, None, CancellationToken::new())
            .await
            .expect("cleanup");
        assert_eq!(
            result.text,
            if reply.contains("Robert") {
                "Bob"
            } else {
                "original speech"
            }
        );
        let requests = runner.requests.lock().expect("requests");
        assert_ne!(requests[0].2, PathBuf::from("private-checkout"));
        assert!(!requests[0].2.exists());
        assert!(requests[0].1.contains("<composer_before>\nbefore "));
    }
}
