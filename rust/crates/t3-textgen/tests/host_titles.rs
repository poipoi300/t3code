use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use serde_json::json;
use t3_contracts::{base_schemas::ProjectId, model_selection::ModelSelection};
use t3_textgen::{
    CancellationToken, TextGeneration, TextRunner,
    generation::{
        GenerationContext, GenerationError, GenerationRequest, ProviderTarget, RoutingState,
    },
    host_titles::{HostTitleGenerator, TitleRoutingSource},
    prompts::{ForkSource, ThreadTitleInput, thread_title_prompt},
    title_context::{LinkedSubject, TitleLinkResolver},
};

struct Routes {
    providers: Mutex<Vec<ProviderTarget>>,
    projects: Mutex<Vec<Option<ProjectId>>>,
}
#[async_trait]
impl TitleRoutingSource for Routes {
    async fn snapshot(
        &self,
        project: Option<&ProjectId>,
        _: CancellationToken,
    ) -> Result<RoutingState, GenerationError> {
        self.projects
            .lock()
            .expect("projects")
            .push(project.cloned());
        Ok(RoutingState {
            providers: self.providers.lock().expect("providers").clone(),
            default_selection: None,
        })
    }
}
struct Capture {
    prompt: String,
    cwd: PathBuf,
    model: ModelSelection,
    attachments: Vec<t3_contracts::chat_attachment::ChatAttachment>,
}
#[derive(Default)]
struct Runner(Mutex<Vec<Capture>>);
#[async_trait]
impl TextRunner for Runner {
    async fn run(
        &self,
        request: &GenerationRequest,
        _: CancellationToken,
    ) -> Result<String, GenerationError> {
        self.0.lock().expect("requests").push(Capture {
            prompt: request.prompt.clone(),
            cwd: request.cwd.clone(),
            model: request.model_selection.clone(),
            attachments: request.attachments.clone(),
        });
        Ok(r#"{"title":"Pairing Expiry","needsRefinement":true}"#.into())
    }
}
#[derive(Default)]
struct Links(Mutex<Vec<String>>);
#[async_trait]
impl TitleLinkResolver for Links {
    fn supports(&self, _: &std::path::Path, url: &reqwest::Url) -> bool {
        url.host_str() == Some("forge.example")
    }
    async fn resolve(&self, _: &std::path::Path, url: &reqwest::Url) -> Result<LinkedSubject, ()> {
        self.0.lock().expect("links").push(url.to_string());
        Ok(LinkedSubject {
            title: "Pairing expiry".into(),
            body: Some("Preserve remote access".into()),
        })
    }
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
        unsupported_models: vec![],
        exhausted_models: vec![],
        exhausted: false,
        custom_models: vec![],
    }
}
fn context() -> GenerationContext {
    GenerationContext{cwd:PathBuf::from("worktree"),project_id:Some(ProjectId("project".into())),model_selection:serde_json::from_value(json!({"instanceId":"codex","model":"configured-title-model","options":{"reasoningEffort":"low"}})).expect("selection")}
}
fn input() -> ThreadTitleInput {
    ThreadTitleInput{message:"Fix https://forge.example/issue?tab=details#comment".into(),project_title:Some("Acme".into()),previous_title:Some("Pairing".into()),fork_source:Some(ForkSource{title:"Remote connections".into(),history:"USER:\nPreserve remote access".into()}),instructions:Some("Use a concise title".into()),attachments:vec![serde_json::from_value(json!({"type":"image","id":"diagram","name":"diagram.png","mimeType":"image/png","sizeBytes":10})).expect("attachment")],linked_context:None}
}

#[tokio::test]
async fn enriches_rich_input_and_refreshes_project_routing_for_every_generation() {
    let runner = Arc::new(Runner::default());
    let links = Arc::new(Links::default());
    let routing = Arc::new(Routes {
        providers: Mutex::new(vec![
            provider("codex", "codex"),
            provider("claudeAgent", "claudeAgent"),
        ]),
        projects: Mutex::new(vec![]),
    });
    let titles = HostTitleGenerator::new(
        Arc::new(TextGeneration::new(runner.clone())),
        routing.clone(),
    )
    .with_links(links.clone());
    let result = titles
        .generate(context(), input(), CancellationToken::new())
        .await
        .expect("title");
    assert_eq!(result.title, "Pairing Expiry");
    assert!(result.needs_refinement);
    let mut enriched = input();
    enriched.linked_context=Some("https://forge.example/issue\n{\"title\":\"Pairing expiry\",\"body\":\"Preserve remote access\"}".into());
    {
        let requests = runner.0.lock().expect("requests");
        assert_eq!(requests[0].prompt, thread_title_prompt(&enriched));
        assert_eq!(requests[0].cwd, context().cwd);
        assert_eq!(requests[0].model, context().model_selection);
        assert_eq!(requests[0].attachments, input().attachments);
    }
    routing.providers.lock().expect("providers")[0].exhausted = true;
    let mut supplied = input();
    supplied.linked_context = Some(String::new());
    titles
        .generate(context(), supplied, CancellationToken::new())
        .await
        .expect("fresh fallback");
    assert_eq!(
        runner.0.lock().expect("requests")[1].model.instance_id.0,
        "claudeAgent"
    );
    assert_eq!(links.0.lock().expect("links").len(), 1);
    assert_eq!(
        *routing.projects.lock().expect("projects"),
        vec![Some(ProjectId("project".into())); 2]
    );
}

#[tokio::test]
async fn disabled_generator_fails_before_link_lookup_or_provider_work() {
    let runner = Arc::new(Runner::default());
    let links = Arc::new(Links::default());
    let mut disabled = provider("codex", "codex");
    disabled.text_generator = true;
    disabled.enabled = false;
    let routing = Arc::new(Routes {
        providers: Mutex::new(vec![disabled]),
        projects: Mutex::new(vec![]),
    });
    let titles = HostTitleGenerator::new(Arc::new(TextGeneration::new(runner.clone())), routing)
        .with_links(links.clone());
    assert!(matches!(
        titles
            .generate(context(), input(), CancellationToken::new())
            .await,
        Err(GenerationError::Disabled)
    ));
    assert!(runner.0.lock().expect("requests").is_empty());
    assert!(links.0.lock().expect("links").is_empty());
}

#[tokio::test]
async fn cancellation_covers_routing_and_cancels_its_owned_work() {
    struct Blocked(tokio::sync::mpsc::UnboundedSender<CancellationToken>);
    #[async_trait]
    impl TitleRoutingSource for Blocked {
        async fn snapshot(
            &self,
            _: Option<&ProjectId>,
            cancellation: CancellationToken,
        ) -> Result<RoutingState, GenerationError> {
            self.0.send(cancellation).expect("entered");
            std::future::pending().await
        }
    }
    let (send, mut entered) = tokio::sync::mpsc::unbounded_channel();
    let runner = Arc::new(Runner::default());
    let titles = HostTitleGenerator::new(
        Arc::new(TextGeneration::new(runner.clone())),
        Arc::new(Blocked(send)),
    );
    let token = CancellationToken::new();
    let child = token.clone();
    let task = tokio::spawn(async move { titles.generate(context(), input(), child).await });
    let owned = entered.recv().await.expect("routing started");
    token.cancel();
    assert!(matches!(
        task.await.expect("join"),
        Err(GenerationError::Cancelled)
    ));
    assert!(owned.is_cancelled());
    assert!(runner.0.lock().expect("requests").is_empty());
}

#[tokio::test]
async fn cancellation_drops_pending_link_resolution_before_starting_the_model() {
    struct PendingLinks(
        tokio::sync::mpsc::UnboundedSender<()>,
        Arc<std::sync::atomic::AtomicBool>,
    );
    #[async_trait]
    impl TitleLinkResolver for PendingLinks {
        fn supports(&self, _: &std::path::Path, _: &reqwest::Url) -> bool {
            true
        }
        async fn resolve(
            &self,
            _: &std::path::Path,
            _: &reqwest::Url,
        ) -> Result<LinkedSubject, ()> {
            struct Dropped(Arc<std::sync::atomic::AtomicBool>);
            impl Drop for Dropped {
                fn drop(&mut self) {
                    self.0.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
            let _dropped = Dropped(self.1.clone());
            self.0.send(()).expect("link entered");
            std::future::pending().await
        }
    }
    let (send, mut entered) = tokio::sync::mpsc::unbounded_channel();
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let runner = Arc::new(Runner::default());
    let routes = Arc::new(Routes {
        providers: Mutex::new(vec![]),
        projects: Mutex::new(vec![]),
    });
    let titles = HostTitleGenerator::new(Arc::new(TextGeneration::new(runner.clone())), routes)
        .with_links(Arc::new(PendingLinks(send, dropped.clone())));
    let token = CancellationToken::new();
    let child = token.clone();
    let task = tokio::spawn(async move { titles.generate(context(), input(), child).await });
    entered.recv().await.expect("lookup started");
    token.cancel();
    assert!(matches!(
        task.await.expect("join"),
        Err(GenerationError::Cancelled)
    ));
    assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    assert!(runner.0.lock().expect("requests").is_empty());
}
