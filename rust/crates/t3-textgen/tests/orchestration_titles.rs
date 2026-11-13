#![cfg(feature = "orchestration-host")]

use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use serde_json::json;
use t3_contracts::{base_schemas::ProjectId, settings::ServerSettings};
use t3_orchestration::{
    api::*,
    composition::{OrchestrationRuntime, RuntimePorts, StaticSettings},
    error::{Error, Result},
    ports::{
        checkpoints_forks::checkpoint_service::CheckpointMetadata,
        sessions_recovery::provider_adapter_registry::AdapterRegistry,
    },
    testing::{ManualClock, TestDatabase},
};
use t3_textgen::{
    CancellationToken, TextGeneration, TextRunner,
    generation::{GenerationContext, GenerationError, GenerationRequest, RoutingState},
    host_titles::{HostTitleGenerator, TitleRoutingSource},
    orchestration::{LegacyTitleContext, LegacyTitleContextSource, OrchestrationTitleGenerator},
};

const NOW: &str = "2026-11-13T12:00:00.000Z";
const THREAD: &str = "thread:title-host";

struct Routes;
#[async_trait]
impl TitleRoutingSource for Routes {
    async fn snapshot(
        &self,
        _: Option<&ProjectId>,
        _: CancellationToken,
    ) -> std::result::Result<RoutingState, GenerationError> {
        Ok(RoutingState {
            providers: vec![],
            default_selection: None,
        })
    }
}
struct Observed {
    prompt: String,
    cwd: PathBuf,
    model: String,
    cancellation: CancellationToken,
}
struct Runner {
    send: tokio::sync::mpsc::UnboundedSender<Observed>,
    block: bool,
}
#[async_trait]
impl TextRunner for Runner {
    async fn run(
        &self,
        request: &GenerationRequest,
        cancellation: CancellationToken,
    ) -> std::result::Result<String, GenerationError> {
        self.send
            .send(Observed {
                prompt: request.prompt.clone(),
                cwd: request.cwd.clone(),
                model: request.model_selection.model.clone(),
                cancellation,
            })
            .map_err(|_| GenerationError::Provider)?;
        if self.block {
            std::future::pending().await
        } else {
            Ok(r#"{"title":"Fix Pairing Expiry","needsRefinement":false}"#.into())
        }
    }
}
struct NoCheckpoints;
#[async_trait]
impl CheckpointBackend for NoCheckpoints {
    async fn capture(&self, _: CaptureCheckpoint, _: CancellationToken) -> Result<CheckpointRef> {
        Err(Error::Unsupported(
            "title test does not capture checkpoints",
        ))
    }
    async fn lookup(
        &self,
        _: OrchestrationV2CheckpointScope,
        _: u64,
    ) -> Result<Option<CheckpointRef>> {
        Ok(None)
    }
    async fn diff(&self, _: PathBuf, _: CheckpointRef, _: CheckpointRef) -> Result<CheckpointDiff> {
        Err(Error::Unsupported("title test does not diff checkpoints"))
    }
    async fn restore(
        &self,
        _: OrchestrationV2CheckpointScope,
        _: u64,
        _: CancellationToken,
    ) -> Result<()> {
        Err(Error::Unsupported(
            "title test does not restore checkpoints",
        ))
    }
    async fn prune(&self, _: OrchestrationV2CheckpointScope, _: Vec<u64>) -> Result<()> {
        Ok(())
    }
}
#[async_trait]
impl CheckpointMetadata for NoCheckpoints {
    fn reference(&self, scope: &CheckpointScopeId, ordinal: u64) -> CheckpointRef {
        CheckpointRef(format!("refs/t3/test/{}/{ordinal}", scope.0))
    }
    async fn is_repository(&self, _: PathBuf) -> Result<bool> {
        Ok(false)
    }
    async fn delete_refs(
        &self,
        _: OrchestrationV2CheckpointScope,
        _: Vec<OrchestrationV2Checkpoint>,
    ) -> Result<()> {
        Ok(())
    }
}
struct Fixture {
    runtime: OrchestrationRuntime,
    _database: TestDatabase,
    entered: tokio::sync::mpsc::UnboundedReceiver<Observed>,
    workspace: PathBuf,
}
async fn fixture(block: bool) -> Result<Fixture> {
    let database = TestDatabase::temporary()?;
    let workspace = database
        .directory
        .as_ref()
        .expect("directory")
        .path()
        .join("workspace");
    std::fs::create_dir(&workspace)?;
    let (send, entered) = tokio::sync::mpsc::unbounded_channel();
    let titles = HostTitleGenerator::new(
        Arc::new(TextGeneration::new(Arc::new(Runner { send, block }))),
        Arc::new(Routes),
    );
    let mut ports = RuntimePorts::new(
        database.stores.clone(),
        Arc::new(AdapterRegistry::new(vec![])),
        Arc::new(NoCheckpoints),
        Arc::new(ManualClock::new(NOW)),
    );
    let mut settings = serde_json::to_value(ServerSettings::default())?;
    settings["threadTitleInstructions"] = json!("Global editorial instruction");
    settings["projectSettingsOverrides"]["project:title-host"] = json!({"threadTitleInstructions":"Project-specific editorial instruction","textGenerationModelSelection":{"instanceId":"codex","model":"project-title-model"}});
    ports.settings = Arc::new(StaticSettings::new(serde_json::from_value(settings)?));
    ports.titles = Some(Arc::new(OrchestrationTitleGenerator::new(titles)));
    let runtime = OrchestrationRuntime::start(ports).await?;
    runtime.project_command(serde_json::from_value(json!({"type":"project.create","commandId":"create-project","projectId":"project:title-host","title":"Acme","workspaceRoot":workspace}))?).await?;
    runtime.dispatch_command(serde_json::from_value(json!({"type":"thread.create","commandId":"create-thread","threadId":THREAD,"projectId":"project:title-host","title":"Original title","modelSelection":{"instanceId":"codex","model":"conversation-model"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"}))?).await?;
    runtime.events.commit(CommitRequest{receipt:None,guard:CommitGuard::None,decision:Decision{events:vec![serde_json::from_value(json!({"id":"title-message-event","threadId":THREAD,"type":"message.updated","occurredAt":NOW,"payload":{"id":"title-message","threadId":THREAD,"runId":null,"nodeId":null,"role":"user","text":"Fix pairing expiry and preserve remote access.","attachments":[],"streaming":false,"createdBy":"user","creationSource":"web","createdAt":NOW,"updatedAt":NOW}}))?],..Default::default()}}).await?;
    Ok(Fixture {
        runtime,
        _database: database,
        entered,
        workspace,
    })
}
async fn request_title(runtime: &OrchestrationRuntime) -> Result<()> {
    runtime.dispatch_command(serde_json::from_value(json!({"type":"thread.metadata.update","commandId":"request-title","threadId":THREAD,"regenerateTitle":true}))?).await?;
    Ok(())
}

#[tokio::test]
async fn composed_title_effect_uses_project_settings_and_persists_the_generated_title() -> Result<()>
{
    let mut fixture = fixture(false).await?;
    request_title(&fixture.runtime).await?;
    fixture.runtime.effect_worker().drain(100).await?;
    let observed = fixture.entered.recv().await.expect("title runner");
    assert_eq!(observed.model, "project-title-model");
    assert_eq!(observed.cwd, fixture.workspace);
    assert!(
        observed
            .prompt
            .contains("Project-specific editorial instruction")
    );
    assert!(!observed.prompt.contains("Global editorial instruction"));
    assert!(
        observed
            .prompt
            .contains("The previous title was \"Original title\"")
    );
    assert!(
        observed
            .prompt
            .contains("Never include the project name (\"Acme\")")
    );
    assert!(
        observed
            .prompt
            .contains("USER:\nFix pairing expiry and preserve remote access.")
    );
    let thread = fixture
        .runtime
        .projections
        .thread(ThreadId(THREAD.into()))
        .await?
        .expect("thread");
    assert_eq!(thread.title.0, "Fix Pairing Expiry");
    let thread = serde_json::to_value(thread)?;
    assert_eq!(thread["lastTitleRegeneration"]["status"], "renamed");
    assert!(thread["titleRegeneration"].is_null());
    assert!(observed.cancellation.is_cancelled());
    fixture.runtime.shutdown().await
}

#[tokio::test]
async fn runtime_shutdown_cancels_the_host_runner_and_preserves_the_pending_title_marker()
-> Result<()> {
    let mut fixture = fixture(true).await?;
    request_title(&fixture.runtime).await?;
    let observed = fixture.entered.recv().await.expect("title runner started");
    fixture.runtime.shutdown().await?;
    assert!(observed.cancellation.is_cancelled());
    let thread = serde_json::to_value(
        fixture
            .runtime
            .projections
            .thread(ThreadId(THREAD.into()))
            .await?
            .expect("thread"),
    )?;
    assert_eq!(thread["title"], "Original title");
    assert_eq!(thread["titleRegeneration"]["requestId"], "request-title");
    Ok(())
}

#[tokio::test]
async fn legacy_adapter_requires_real_metadata_and_uses_only_the_supplied_history() -> Result<()> {
    struct Metadata;
    #[async_trait]
    impl LegacyTitleContextSource for Metadata {
        async fn resolve(
            &self,
            thread: ThreadId,
            _: CancellationToken,
        ) -> std::result::Result<LegacyTitleContext, GenerationError> {
            assert_eq!(thread.0, "legacy");
            Ok(LegacyTitleContext {
                generation: GenerationContext {
                    cwd: PathBuf::from("legacy-workspace"),
                    project_id: Some(ProjectId("legacy-project".into())),
                    model_selection: serde_json::from_value(
                        json!({"instanceId":"codex","model":"legacy-title-model"}),
                    )
                    .expect("selection"),
                },
                project_title: Some("Legacy Project".into()),
                previous_title: Some("Pairing".into()),
                fork_source: None,
                instructions: None,
            })
        }
    }
    let (send, mut entered) = tokio::sync::mpsc::unbounded_channel();
    let adapter = OrchestrationTitleGenerator::new(HostTitleGenerator::new(
        Arc::new(TextGeneration::new(Arc::new(Runner { send, block: false }))),
        Arc::new(Routes),
    ));
    assert!(matches!(
        adapter
            .generate(ThreadId("legacy".into()), vec![], CancellationToken::new())
            .await,
        Err(Error::Unsupported(_))
    ));
    let messages = vec![serde_json::from_value(
        json!({"role":"user","text":"Fix pairing expiry","threadId":"legacy","runId":null,"itemId":"legacy-message","providerThreadId":null,"status":"completed","kind":"user_message"}),
    )?];
    let title = adapter
        .with_legacy_context(Arc::new(Metadata))
        .generate(
            ThreadId("legacy".into()),
            messages,
            CancellationToken::new(),
        )
        .await?;
    assert_eq!(title, "Fix Pairing Expiry");
    let request = entered.recv().await.expect("request");
    assert_eq!(request.cwd, PathBuf::from("legacy-workspace"));
    assert_eq!(request.model, "legacy-title-model");
    assert!(
        request
            .prompt
            .contains("Thread contents:\nUSER:\nFix pairing expiry")
    );
    Ok(())
}
