use super::{checkpoint_capture_service::domain_event, checkpoint_restore_safety::expanded_id};
use crate::{
    api::*,
    error::{Error, Result},
    ports::decisions::dispatch_mode_limit::DispatchModes,
    stores::{
        StoreDatabase, projects,
        receipts::{self, AggregateId, CommandReceipt, ReceiptStatus},
    },
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use t3_contracts::{
    chat_attachment::ChatAttachment,
    composer_context::OrchestrationMessageContext,
    model_selection::ModelSelection,
    provider_policy::{ProviderInteractionMode, RuntimeMode},
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[derive(Clone)]
pub struct InitialMessage {
    pub message_id: Option<MessageId>,
    pub scheduled_task_id: Option<ScheduledTaskId>,
    pub sender_thread_id: Option<ThreadId>,
    pub text: String,
    pub attachments: Vec<ChatAttachment>,
    pub context: Option<OrchestrationMessageContext>,
}

#[derive(Clone)]
pub struct LaunchInput {
    pub command_id: CommandId,
    pub thread_id: Option<ThreadId>,
    pub reuse_existing_thread: bool,
    pub project_id: ProjectId,
    pub title: String,
    pub generate_title: bool,
    pub model_selection: ModelSelection,
    pub runtime_mode: RuntimeMode,
    pub interaction_mode: ProviderInteractionMode,
    pub workspace_strategy: OrchestrationV2ThreadLaunchWorkspaceStrategy,
    pub initial_message: Option<InitialMessage>,
    pub imported_native_thread: Option<CommandThreadCreateImportedNativeThread>,
    pub created_by: OrchestrationV2Actor,
    pub creation_source: OrchestrationV2CreationSource,
    pub link_origin: Option<OrchestrationV2LinkOrigin>,
    pub delegated_from: Option<OrchestrationV2DelegatedFrom>,
}

pub struct LaunchResult {
    pub thread_id: ThreadId,
    pub projection: OrchestrationV2ThreadProjection,
    pub resumed: bool,
    pub sequence: Option<u64>,
}

#[derive(Clone)]
pub struct PreparationInput {
    pub command_id: CommandId,
    pub thread_id: ThreadId,
    pub run_id: RunId,
}

pub struct SetupInput {
    pub project_id: ProjectId,
    pub thread_id: ThreadId,
    pub project_cwd: PathBuf,
    pub worktree_path: PathBuf,
}

pub enum SetupRun {
    Skipped,
    Started {
        terminal_id: String,
        asynchronous: bool,
        completion: Option<tokio::sync::oneshot::Receiver<Result<Option<i32>>>>,
    },
}

#[derive(Clone)]
pub struct GeneratedBranch {
    pub branch: String,
    pub exact_name: bool,
}

/// Services supplied by workspace/config/terminal composition, outside orchestration.
#[async_trait]
pub trait LaunchEnvironment: Send + Sync {
    async fn is_scratch(&self, project: &ProjectId) -> Result<bool>;
    async fn reject_during_clone(&self, project: &ProjectId) -> Result<()>;
    async fn assert_provider_enabled(
        &self,
        project: &ProjectId,
        command: &CommandId,
        instance: &t3_contracts::provider_instance::ProviderInstanceId,
    ) -> Result<()>;
    async fn folder_for_thread(
        &self,
        project: &ProjectId,
        thread: &ThreadId,
        text: &str,
    ) -> Result<Option<PathBuf>>;
    async fn repository_root(&self, cwd: PathBuf) -> Result<Option<PathBuf>>;
    async fn has_commit(&self, cwd: PathBuf, reference: &str) -> Result<bool>;
    async fn has_ref(&self, cwd: PathBuf, reference: &str) -> Result<bool>;
    async fn remote_for_base(&self, cwd: PathBuf, reference: &str) -> Result<Option<String>>;
    async fn fetch_worktree_base(
        &self,
        _cwd: PathBuf,
        _reference: &str,
        _remote: &str,
    ) -> Result<bool> {
        Err(Error::Unsupported("launch.fetch-worktree-base"))
    }
    async fn local_branches(&self, cwd: PathBuf) -> Result<Vec<String>>;
    async fn temporary_branch(&self, project: &ProjectId) -> Result<String>;
    async fn generate_branch(
        &self,
        project: &ProjectId,
        cwd: PathBuf,
        message: &InitialMessage,
        cancellation: CancellationToken,
    ) -> Result<GeneratedBranch>;
    async fn rename_branch(
        &self,
        cwd: PathBuf,
        old: &str,
        generated: GeneratedBranch,
        cancellation: CancellationToken,
    ) -> Result<String>;
    async fn setup(&self, input: SetupInput, cancellation: CancellationToken) -> Result<SetupRun>;
    async fn close_terminal(&self, thread: &ThreadId, terminal: &str) -> Result<()>;
}

/// Host-owned settings, folder policy, model naming and terminal lifecycle.
#[async_trait]
pub trait LaunchHost: Send + Sync {
    async fn is_scratch(&self, project: &ProjectId) -> Result<bool>;
    async fn reject_during_clone(&self, project: &ProjectId) -> Result<()>;
    async fn assert_provider_enabled(
        &self,
        project: &ProjectId,
        command: &CommandId,
        instance: &t3_contracts::provider_instance::ProviderInstanceId,
    ) -> Result<()>;
    async fn folder_for_thread(
        &self,
        project: &ProjectId,
        thread: &ThreadId,
        text: &str,
    ) -> Result<Option<PathBuf>>;
    async fn temporary_branch(&self, project: &ProjectId) -> Result<String>;
    async fn generate_branch(
        &self,
        project: &ProjectId,
        cwd: PathBuf,
        message: &InitialMessage,
        cancellation: CancellationToken,
    ) -> Result<GeneratedBranch>;
    async fn setup(&self, input: SetupInput, cancellation: CancellationToken) -> Result<SetupRun>;
    async fn close_terminal(&self, thread: &ThreadId, terminal: &str) -> Result<()>;
}

#[async_trait]
pub trait LaunchCommands: Send + Sync {
    async fn dispatch(
        &self,
        command: OrchestrationV2ServerCommand,
        limit: Option<DispatchModes>,
    ) -> Result<CommandReceipt>;
}

#[async_trait]
impl LaunchCommands for crate::runtime::dispatch::Dispatcher {
    async fn dispatch(
        &self,
        command: OrchestrationV2ServerCommand,
        limit: Option<DispatchModes>,
    ) -> Result<CommandReceipt> {
        self.dispatch_with_limit(command, limit).await
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupStage {
    Fetch,
    Checkout,
    Submodules,
    SetupScript,
    Agent,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageStatus {
    Pending,
    Running,
    Done,
    Skipped,
    Error,
}
#[derive(Clone, Debug)]
pub struct StageUpdate {
    pub stage: SetupStage,
    pub status: StageStatus,
    pub percent: Option<f64>,
    pub detail: Option<String>,
}

#[async_trait]
pub trait LaunchProgress: Send + Sync {
    async fn update(&self, thread: &ThreadId, update: StageUpdate) -> Result<()>;
}
struct NoLaunchProgress;
#[async_trait]
impl LaunchProgress for NoLaunchProgress {
    async fn update(&self, _: &ThreadId, _: StageUpdate) -> Result<()> {
        Ok(())
    }
}

pub struct PreparationObserver {
    thread: ThreadId,
    claimed: Mutex<Option<Workspace>>,
    allow_claim: bool,
    progress: Arc<dyn LaunchProgress>,
}
impl PreparationObserver {
    /// Register immediately after claiming the path, before checkout or submodule work can fail.
    pub fn claimed(&self, workspace: Workspace) -> Result<()> {
        if !self.allow_claim || workspace.worktree_path.is_none() {
            return Err(failure(
                "claim-worktree",
                "Only newly created worktrees may be claimed.",
            ));
        }
        let mut claimed = self.claimed.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(previous) = claimed.as_ref()
            && previous.worktree_path != workspace.worktree_path
        {
            return Err(failure(
                "claim-worktree",
                "A preparation cannot claim a second worktree.",
            ));
        }
        *claimed = Some(workspace);
        Ok(())
    }
    pub async fn update(&self, update: StageUpdate) -> Result<()> {
        self.progress.update(&self.thread, update).await
    }
    async fn status(&self, stage: SetupStage, status: StageStatus) -> Result<()> {
        self.update(StageUpdate {
            stage,
            status,
            percent: None,
            detail: None,
        })
        .await
    }
}

#[async_trait]
pub trait LaunchWorkspacePreparation: Send + Sync {
    async fn prepare(
        &self,
        project: ProjectId,
        thread: ThreadId,
        strategy: OrchestrationV2ThreadLaunchWorkspaceStrategy,
        observer: Arc<PreparationObserver>,
        cancellation: CancellationToken,
    ) -> Result<Workspace>;
}
struct BackendPreparation(Arc<dyn WorkspaceBackend>);
#[async_trait]
impl LaunchWorkspacePreparation for BackendPreparation {
    async fn prepare(
        &self,
        project: ProjectId,
        thread: ThreadId,
        strategy: OrchestrationV2ThreadLaunchWorkspaceStrategy,
        _: Arc<PreparationObserver>,
        cancellation: CancellationToken,
    ) -> Result<Workspace> {
        self.0
            .prepare(project, thread, strategy, cancellation)
            .await
    }
}

pub struct LaunchServices {
    pub database: StoreDatabase,
    pub projections: Arc<dyn ProjectionReader>,
    pub events: Arc<dyn EventCommitter>,
    pub commands: Arc<dyn LaunchCommands>,
    pub workspaces: Arc<dyn WorkspaceBackend>,
    pub environment: Arc<dyn LaunchEnvironment>,
    pub ids: Arc<dyn IdGenerator>,
    pub clock: Arc<dyn Clock>,
}

struct Inner {
    services: LaunchServices,
    preparation: Arc<dyn LaunchWorkspacePreparation>,
    progress: Arc<dyn LaunchProgress>,
    scheduled: Mutex<HashSet<CommandId>>,
    tasks: TaskTracker,
    cancellation: CancellationToken,
    launches: crate::ports::decisions::thread_command_executor::KeyedLanes,
    admission: tokio::sync::RwLock<()>,
}

#[derive(Clone)]
pub struct ThreadLaunchService(Arc<Inner>);

#[derive(Clone)]
struct Preparation {
    command_id: CommandId,
    project_id: ProjectId,
    thread_id: ThreadId,
    run_id: Option<RunId>,
    strategy: OrchestrationV2ThreadLaunchWorkspaceStrategy,
    message: Option<InitialMessage>,
    reused: bool,
}

fn failure(operation: &'static str, error: impl std::fmt::Display) -> Error {
    Error::Service {
        operation,
        detail: error.to_string(),
    }
}

fn branch(strategy: &OrchestrationV2ThreadLaunchWorkspaceStrategy) -> Option<String> {
    match strategy {
        OrchestrationV2ThreadLaunchWorkspaceStrategy::Root(s) => &s.branch,
        OrchestrationV2ThreadLaunchWorkspaceStrategy::ExistingWorktree(s) => &s.branch,
        OrchestrationV2ThreadLaunchWorkspaceStrategy::Worktree(s) => &s.branch,
    }
    .as_ref()
    .and_then(Option::as_ref)
    .map(|b| b.0.clone())
}

fn worktree_path(strategy: &OrchestrationV2ThreadLaunchWorkspaceStrategy) -> Option<PathBuf> {
    match strategy {
        OrchestrationV2ThreadLaunchWorkspaceStrategy::ExistingWorktree(s) => {
            Some(PathBuf::from(&s.worktree_path.0))
        }
        _ => None,
    }
}

async fn reserve_claim(
    strategy: &OrchestrationV2ThreadLaunchWorkspaceStrategy,
    project_root: &str,
    cancellation: &CancellationToken,
) -> Result<super::checkpoint_restore_safety::WorkspaceReservation> {
    use super::checkpoint_restore_safety::{ReservationKind, WorkspaceReservations};
    // A record without a worktree path claims its project root for isolation purposes.
    let folder = worktree_path(strategy).unwrap_or_else(|| PathBuf::from(project_root));
    WorkspaceReservations::global()
        .reserve(&folder, ReservationKind::TurnStart, cancellation)
        .await
}

fn temporary(branch: &str) -> bool {
    let normalized = branch.trim().to_ascii_lowercase();
    let branch = normalized.as_str();
    let hex = |byte: u8| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte);
    let suffix = branch
        .strip_prefix("t3/")
        .or_else(|| branch.strip_prefix("t3-"))
        .or_else(|| branch.strip_prefix("t3code/"))
        .or_else(|| branch.strip_prefix("t3code-"));
    if suffix.is_some_and(|suffix| suffix.len() == 8 && suffix.bytes().all(hex)) {
        return true;
    }
    branch.strip_prefix("t3code/").is_some_and(|suffix| {
        let bytes = suffix.as_bytes();
        bytes.len() == 36
            && bytes[14] == b'4'
            && b"89ab".contains(&bytes[19])
            && bytes.iter().enumerate().all(|(index, byte)| {
                if [8, 13, 18, 23].contains(&index) {
                    *byte == b'-'
                } else {
                    hex(*byte)
                }
            })
    })
}

impl ThreadLaunchService {
    pub fn new(services: LaunchServices) -> Self {
        let preparation = Arc::new(BackendPreparation(services.workspaces.clone()));
        Self::with_preparation(services, preparation, Arc::new(NoLaunchProgress))
    }

    pub fn with_preparation(
        services: LaunchServices,
        preparation: Arc<dyn LaunchWorkspacePreparation>,
        progress: Arc<dyn LaunchProgress>,
    ) -> Self {
        Self(Arc::new(Inner {
            services,
            preparation,
            progress,
            scheduled: Mutex::new(HashSet::new()),
            tasks: TaskTracker::new(),
            cancellation: CancellationToken::new(),
            launches: Default::default(),
            admission: Default::default(),
        }))
    }

    pub async fn drain(&self) {
        self.0.tasks.close();
        self.0.tasks.wait().await;
        self.0.tasks.reopen();
    }

    pub async fn shutdown(&self) {
        self.0.cancellation.cancel();
        let _admission = self.0.admission.write().await;
        self.0.tasks.close();
        self.0.tasks.wait().await;
    }

    async fn receipt(&self, command_id: &CommandId) -> Result<Option<CommandReceipt>> {
        let id = command_id.clone();
        self.0
            .services
            .database
            .read(move |db| receipts::get(db, &id))
            .await
    }

    async fn dispatch(&self, value: Value, limit: Option<DispatchModes>) -> Result<CommandReceipt> {
        self.0
            .services
            .commands
            .dispatch(serde_json::from_value(value)?, limit)
            .await
    }

    async fn initial_branch(
        &self,
        strategy: &OrchestrationV2ThreadLaunchWorkspaceStrategy,
        cwd: &str,
    ) -> Option<String> {
        if matches!(strategy, OrchestrationV2ThreadLaunchWorkspaceStrategy::Root(s) if s.branch.is_none())
        {
            self.0
                .services
                .workspaces
                .current_branch(PathBuf::from(cwd))
                .await
                .ok()
                .flatten()
        } else {
            branch(strategy)
        }
    }

    pub async fn check_worktree_base(
        &self,
        project_id: ProjectId,
        strategy: &OrchestrationV2ThreadLaunchWorkspaceStrategy,
        command_id: Option<CommandId>,
    ) -> Result<()> {
        let OrchestrationV2ThreadLaunchWorkspaceStrategy::Worktree(strategy) = strategy else {
            return Ok(());
        };
        let s = &self.0.services;
        let (scratch, receipt, project) = tokio::join!(
            s.environment.is_scratch(&project_id),
            async {
                match command_id {
                    Some(command) => self.receipt(&command).await.ok().flatten(),
                    None => None,
                }
            },
            async {
                let project_id = project_id.clone();
                s.database
                    .read(move |db| projects::get(db, &project_id, false))
                    .await
                    .ok()
                    .flatten()
            }
        );
        if scratch? || receipt.is_some_and(|receipt| receipt.status == ReceiptStatus::Accepted) {
            return Ok(());
        }
        let Some(project) = project else {
            return Ok(());
        };
        let cwd = PathBuf::from(&project.workspace_root);
        let reference = strategy.base_ref.0.as_str();
        let requested = strategy
            .branch
            .as_ref()
            .and_then(Option::as_ref)
            .filter(|branch| !temporary(&branch.0));
        let (repository, has_base, branches) = tokio::join!(
            s.environment.repository_root(cwd.clone()),
            async {
                if reference.starts_with(':') {
                    true
                } else {
                    s.environment
                        .has_commit(
                            cwd.clone(),
                            if reference == "-" { "@{-1}" } else { reference },
                        )
                        .await
                        .unwrap_or(true)
                }
            },
            async {
                if requested.is_some() {
                    s.environment
                        .local_branches(cwd.clone())
                        .await
                        .unwrap_or_default()
                } else {
                    Vec::new()
                }
            }
        );
        if matches!(repository, Ok(None)) {
            return Err(failure(
                "thread-launch.not-repository",
                format!(
                    "Project \"{}\" ({}) is not a Git repository.",
                    project.title, project.workspace_root
                ),
            ));
        }
        let resolves: Result<bool> = async {
            if has_base {
                return Ok(true);
            }
            if strategy.start_from_origin == Some(Some(true))
                && let Some(remote) = s
                    .environment
                    .remote_for_base(cwd.clone(), reference)
                    .await?
                && s.environment
                    .fetch_worktree_base(cwd.clone(), reference, &remote)
                    .await?
            {
                return Ok(true);
            }
            if s.environment.has_ref(cwd.clone(), reference).await? {
                return Ok(true);
            }
            Ok(!s.environment.has_commit(cwd.clone(), "HEAD").await?)
        }
        .await;
        if !resolves.unwrap_or(true) {
            return Err(failure(
                "thread-launch.base-ref",
                format!(
                    "Base ref \"{reference}\" does not resolve to a commit in project \"{}\" ({}).",
                    project.title, project.workspace_root
                ),
            ));
        }
        if let Some(Some(requested)) = &strategy.branch
            && !temporary(&requested.0)
        {
            for existing in branches {
                if existing.starts_with(&format!("{}/", requested.0))
                    || requested.0.starts_with(&format!("{existing}/"))
                {
                    return Err(failure(
                        "thread-launch.branch-conflict",
                        format!(
                            "Branch \"{}\" cannot be created in project \"{}\": branch \"{existing}\" exists.",
                            requested.0, project.title
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    pub async fn launch(&self, mut input: LaunchInput) -> Result<LaunchResult> {
        let _admission = self.0.admission.read().await;
        if self.0.cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        // Same-key callers share receipt recovery; independent launches prepare concurrently.
        let _lane = self.0.launches.acquire([input.command_id.0.clone()]).await;
        let s = &self.0.services;
        if self.0.cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if matches!(
            input.workspace_strategy,
            OrchestrationV2ThreadLaunchWorkspaceStrategy::Worktree(_)
        ) && s.environment.is_scratch(&input.project_id).await?
        {
            input.workspace_strategy = serde_json::from_value(json!({"type":"root"}))?;
        }
        s.environment
            .reject_during_clone(&input.project_id)
            .await
            .map_err(|e| failure("resolve-project", e))?;
        let project_id = input.project_id.clone();
        let project = s
            .database
            .read(move |db| projects::get(db, &project_id, false))
            .await?
            .ok_or_else(|| failure("resolve-project", "Project not found."))?;
        if input.reuse_existing_thread && input.thread_id.is_none() {
            return Err(failure(
                "update-thread",
                "Reusing an existing thread requires a thread id.",
            ));
        }
        let launch_receipt = self
            .receipt(&input.command_id)
            .await
            .map_err(|e| failure("read-receipt", e))?;
        s.environment
            .assert_provider_enabled(
                &input.project_id,
                &input.command_id,
                &input.model_selection.instance_id,
            )
            .await?;
        let reusable = launch_receipt
            .as_ref()
            .filter(|r| r.status == ReceiptStatus::Accepted && r.command_type == "thread.create");
        let candidate = input
            .thread_id
            .clone()
            .or_else(|| {
                reusable.and_then(|r| match &r.aggregate {
                    AggregateId::Thread(id) => Some(id.clone()),
                    _ => None,
                })
            })
            .unwrap_or_else(|| ThreadId(s.ids.next(IdKind::Thread)));
        if input.thread_id.is_none() && reusable.is_some() {
            let thread = s
                .projections
                .thread(candidate.clone())
                .await?
                .ok_or_else(|| failure("create-thread", "Thread not found."))?;
            if expanded_id(&thread.project_id)? != input.project_id {
                return Err(failure("resolve-project", "Project identity changed."));
            }
        }
        if input.reuse_existing_thread && launch_receipt.is_none() {
            let current = s
                .projections
                .records(
                    candidate.clone(),
                    vec![RecordKind::Runs],
                    RecordFilter::default(),
                )
                .await
                .map_err(|e| failure("update-thread", e))?;
            let count = s
                .projections
                .message_count(candidate.clone(), MessageFilter::default())
                .await?;
            if expanded_id(&current.thread.project_id)? != input.project_id
                || current.thread.archived_at.is_some()
                || current.thread.deleted_at.is_some()
                || count > 0
                || current
                    .records
                    .iter()
                    .any(|r| matches!(r, ProjectionRecordSet::Runs(runs) if !runs.is_empty()))
            {
                return Err(failure(
                    "update-thread",
                    "Only an empty active thread in the target project can change workspace during launch.",
                ));
            }
        }
        let mut strategy = input.workspace_strategy.clone();
        if matches!(
            strategy,
            OrchestrationV2ThreadLaunchWorkspaceStrategy::Root(_)
        ) && launch_receipt.is_none()
            && let Some(path) = s
                .environment
                .folder_for_thread(
                    &input.project_id,
                    &candidate,
                    input
                        .initial_message
                        .as_ref()
                        .map_or(&input.title, |m| &m.text),
                )
                .await
                .map_err(|e| failure("provision-worktree", e))?
        {
            strategy =
                serde_json::from_value(json!({"type":"existing_worktree", "worktreePath": path}))?;
        }
        let claim_reservation =
            reserve_claim(&strategy, &project.workspace_root, &self.0.cancellation).await?;
        let mut claim = if input.reuse_existing_thread {
            json!({"type":"thread.metadata.update", "commandId": input.command_id, "threadId": candidate, "expectedEmpty":true})
        } else {
            json!({"type":"thread.create", "commandId":input.command_id, "threadId":candidate, "projectId":input.project_id,"title":input.title,"modelSelection":input.model_selection,"runtimeMode":input.runtime_mode,"interactionMode":input.interaction_mode,"branch":self.initial_branch(&strategy, &project.workspace_root).await,"worktreePath":worktree_path(&strategy),"createdBy":input.created_by,"creationSource":input.creation_source})
        };
        if !input.reuse_existing_thread {
            for (key, value) in [
                (
                    "importedNativeThread",
                    serde_json::to_value(&input.imported_native_thread)?,
                ),
                ("linkOrigin", serde_json::to_value(&input.link_origin)?),
                (
                    "delegatedFrom",
                    serde_json::to_value(&input.delegated_from)?,
                ),
            ] {
                if !value.is_null() {
                    claim[key] = value;
                }
            }
        }
        let claimed = self.dispatch(claim, None).await.map_err(|e| {
            failure(
                if input.reuse_existing_thread {
                    "update-thread"
                } else {
                    "create-thread"
                },
                e,
            )
        })?;
        drop(claim_reservation);
        let AggregateId::Thread(thread_id) = &claimed.aggregate else {
            return Err(failure(
                "create-thread",
                "Launch receipt is not thread scoped.",
            ));
        };
        if *thread_id != candidate {
            return Err(failure(
                "create-thread",
                "Command cannot be replayed for another thread.",
            ));
        }
        let thread_id = thread_id.clone();
        let mut run_id = None;
        let mut sequence = None;
        let mut message_resumed = false;
        if let Some(message) = &input.initial_message {
            let command_id = CommandId(format!("{}:initial-message", input.command_id.0));
            let before = self.receipt(&command_id).await?;
            let mut command = json!({"type":"message.dispatch","commandId":command_id,"threadId":thread_id,"messageId":message.message_id.clone().unwrap_or_else(|| MessageId(s.ids.next(IdKind::Message))),"text":message.text,"attachments":message.attachments,"modelSelection":input.model_selection,"dispatchMode":{"type":"defer_start","workspaceStrategy":strategy},"createdBy":input.created_by,"creationSource":input.creation_source});
            if let Some(id) = &message.scheduled_task_id {
                command["scheduledTaskId"] = json!(id);
            }
            if let Some(id) = &message.sender_thread_id {
                command["senderThreadId"] = json!(id);
            }
            if let Some(context) = &message.context {
                command["context"] = json!(context);
            }
            if input.generate_title {
                command["titleSeed"] = json!(input.title);
            }
            let limit =
                (input.created_by == OrchestrationV2Actor::Agent).then_some(DispatchModes {
                    runtime_mode: input.runtime_mode,
                    interaction_mode: input.interaction_mode,
                });
            let receipt = self
                .dispatch(command, limit)
                .await
                .map_err(|e| failure("dispatch-message", e))?;
            message_resumed = before.is_some();
            sequence = Some(
                u64::try_from(receipt.result_sequence).map_err(|e| failure("read-receipt", e))?,
            );
            for event in s.events.events_by_command(command_id).await? {
                if let OrchestrationV2DomainEvent::RunCreated(event) = event.event {
                    run_id = Some(expanded_id(&event.payload.id)?);
                    break;
                }
            }
            if run_id.is_none() {
                return Err(failure(
                    "dispatch-message",
                    "Initial message was accepted without a durable run.",
                ));
            }
        }
        if run_id.is_some() || launch_receipt.is_none() {
            let current = match &run_id {
                Some(id) => Some(
                    s.projections
                        .records(
                            thread_id.clone(),
                            vec![RecordKind::Runs],
                            RecordFilter {
                                run_ids: Some(vec![id.clone()]),
                                ..Default::default()
                            },
                        )
                        .await?,
                ),
                None => None,
            };
            let preparing=current.as_ref().is_none_or(|records|records.records.iter().any(|set|matches!(set,ProjectionRecordSet::Runs(runs) if runs.iter().any(|run|expanded_id(&run.id).ok().as_ref() == run_id.as_ref() && run.status == OrchestrationV2RunStatus::Preparing))));
            if preparing {
                if launch_receipt.is_some()
                    && matches!(
                        strategy,
                        OrchestrationV2ThreadLaunchWorkspaceStrategy::Root(_)
                    )
                    && let Some(path) = current
                        .as_ref()
                        .and_then(|records| records.thread.worktree_path.as_ref())
                {
                    strategy = serde_json::from_value(
                        json!({"type":"existing_worktree","worktreePath":path,"branch":branch(&strategy)}),
                    )?;
                }
                self.schedule(Preparation {
                    command_id: input.command_id,
                    project_id: input.project_id,
                    thread_id: thread_id.clone(),
                    run_id,
                    strategy,
                    message: input.initial_message.clone(),
                    reused: false,
                })?;
            }
        }
        let projection = s
            .projections
            .projection(thread_id.clone())
            .await?
            .ok_or_else(|| failure("create-thread", "Thread not found."))?;
        Ok(LaunchResult {
            thread_id,
            projection,
            resumed: if input.initial_message.is_some() {
                message_resumed
            } else {
                launch_receipt.is_some()
            },
            sequence,
        })
    }

    pub async fn retry_preparation(&self, input: PreparationInput) -> Result<CommandReceipt> {
        let _admission = self.0.admission.read().await;
        if self.0.cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let receipt = self.dispatch(json!({"type":"prepared-run.retry","commandId":input.command_id,"threadId":input.thread_id,"runId":input.run_id}), None).await?;
        if let Err(error) = self.schedule_run(&input).await {
            self.fail(
                &input.command_id,
                &input.thread_id,
                Some(&input.run_id),
                &error,
            )
            .await;
        }
        Ok(receipt)
    }

    pub async fn prepare_run(&self, input: PreparationInput) {
        let _admission = self.0.admission.read().await;
        if self.0.cancellation.is_cancelled() {
            return;
        }
        if let Err(error) = self.schedule_run(&input).await {
            self.fail(
                &input.command_id,
                &input.thread_id,
                Some(&input.run_id),
                &error,
            )
            .await;
        }
    }

    async fn schedule_run(&self, input: &PreparationInput) -> Result<()> {
        let s = &self.0.services;
        let records = s
            .projections
            .records(
                input.thread_id.clone(),
                vec![RecordKind::Runs],
                RecordFilter {
                    run_ids: Some(vec![input.run_id.clone()]),
                    ..Default::default()
                },
            )
            .await?;
        let run = records.records.iter().find_map(|set| match set {
            ProjectionRecordSet::Runs(runs) => runs
                .iter()
                .find(|r| expanded_id(&r.id).as_ref().ok() == Some(&input.run_id)),
            _ => None,
        });
        let Some(run) = run.filter(|r| r.status == OrchestrationV2RunStatus::Preparing) else {
            return Ok(());
        };
        let Some(Some(mut strategy)) = run.workspace_preparation.clone() else {
            return Ok(());
        };
        let reused = matches!(
            strategy,
            OrchestrationV2ThreadLaunchWorkspaceStrategy::Worktree(_)
        ) && records.thread.worktree_path.is_some()
            && records.thread.branch.is_some();
        if reused {
            strategy = serde_json::from_value(
                json!({"type":"existing_worktree","worktreePath":records.thread.worktree_path,"branch":records.thread.branch}),
            )?;
        }
        let message = s
            .projections
            .run_message(input.thread_id.clone(), input.run_id.clone())
            .await?
            .map(|m| InitialMessage {
                message_id: None,
                scheduled_task_id: None,
                sender_thread_id: None,
                text: m.text,
                attachments: m.attachments,
                context: m.context.flatten(),
            });
        self.schedule(Preparation {
            command_id: input.command_id.clone(),
            project_id: expanded_id(&records.thread.project_id)?,
            thread_id: input.thread_id.clone(),
            run_id: Some(input.run_id.clone()),
            strategy,
            message,
            reused,
        })
    }

    fn schedule(&self, input: Preparation) -> Result<()> {
        if self.0.cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if !self
            .0
            .scheduled
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(input.command_id.clone())
        {
            return Ok(());
        }
        let service = self.clone();
        // Subscribe before inspecting durable run state so an interruption cannot fall in the gap.
        let events = self.0.services.events.subscribe();
        self.0.tasks.spawn(async move {
            let result = service.run_preparation(&input, events).await;
            if let Err(error) = result {
                service
                    .fail(
                        &input.command_id,
                        &input.thread_id,
                        input.run_id.as_ref(),
                        &error,
                    )
                    .await;
            }
            service
                .0
                .scheduled
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&input.command_id);
        });
        Ok(())
    }

    async fn run_preparation(
        &self,
        input: &Preparation,
        mut events: tokio::sync::broadcast::Receiver<
            t3_contracts::application_event::ApplicationStoredEvent,
        >,
    ) -> Result<()> {
        if let Some(run_id) = &input.run_id {
            let run = self
                .0
                .services
                .projections
                .run(input.thread_id.clone(), run_id.clone())
                .await?;
            if run.is_none_or(|r| r.status != OrchestrationV2RunStatus::Preparing) {
                return Ok(());
            }
        }
        let cancellation = self.0.cancellation.child_token();
        let preparation = self.prepare(input, cancellation.clone());
        tokio::pin!(preparation);
        loop {
            tokio::select! {
                result = &mut preparation => return result,
                _ = self.0.cancellation.cancelled() => { cancellation.cancel(); let _ = preparation.await; return Err(Error::Cancelled) },
                event = events.recv(), if input.run_id.is_some() => {
                    use t3_contracts::application_event::ApplicationStoredEvent;
                    let stop = match event {
                        Ok(ApplicationStoredEvent::Orchestration(stored)) => match stored.event {
                            OrchestrationV2DomainEvent::RunUpdated(event) => event.thread_id == input.thread_id && expanded_id(&event.payload.id).as_ref().ok() == input.run_id.as_ref() && matches!(event.payload.status, OrchestrationV2RunStatus::Interrupted | OrchestrationV2RunStatus::Cancelled),
                            _ => false,
                        },
                        Ok(_) => false,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let run = self.0.services.projections.run(input.thread_id.clone(), input.run_id.clone().ok_or(Error::Stale)?).await?;
                            run.is_none_or(|r| matches!(r.status, OrchestrationV2RunStatus::Interrupted | OrchestrationV2RunStatus::Cancelled))
                        },
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => true,
                    };
                    if stop { cancellation.cancel(); let _ = preparation.await; return Err(Error::Cancelled) }
                }
            }
        }
    }

    async fn fail(
        &self,
        command_id: &CommandId,
        thread_id: &ThreadId,
        run_id: Option<&RunId>,
        error: &Error,
    ) {
        if let Some(run) = run_id {
            let message = if matches!(error, Error::Cancelled) {
                "Worktree setup cancelled.".into()
            } else {
                format!("Workspace preparation failed: {error}")
            };
            let _ = self.dispatch(json!({"type":"prepared-run.fail","commandId":format!("{}:fail",command_id.0),"threadId":thread_id,"runId":run,"failure":{"class":"validation_error","message":message,"code":null,"retryable":false}}),None).await;
        }
    }

    async fn prepare(&self, input: &Preparation, cancellation: CancellationToken) -> Result<()> {
        let s = &self.0.services;
        let id = input.project_id.clone();
        let project = s
            .database
            .read(move |db| projects::get(db, &id, false))
            .await?
            .ok_or_else(|| failure("resolve-project", "Project no longer exists."))?;
        let mut strategy = input.strategy.clone();
        let generated = matches!(&strategy,OrchestrationV2ThreadLaunchWorkspaceStrategy::Worktree(w) if w.branch.as_ref().and_then(Option::as_ref).is_none());
        let mut rename = generated || branch(&strategy).as_deref().is_some_and(temporary);
        if let OrchestrationV2ThreadLaunchWorkspaceStrategy::Worktree(w) = &mut strategy {
            let mut name = if generated {
                s.environment.temporary_branch(&input.project_id).await?
            } else {
                branch(&input.strategy)
                    .ok_or_else(|| failure("provision-worktree", "Missing branch"))?
            };
            let parts: Vec<_> = name.split('/').collect();
            for depth in 1..parts.len() {
                if rename
                    && s.environment
                        .has_commit(
                            PathBuf::from(&project.workspace_root),
                            &format!("refs/heads/{}", parts[..depth].join("/")),
                        )
                        .await?
                {
                    name = if generated {
                        name.replace('/', "-")
                    } else {
                        let normalized = name.trim().to_ascii_lowercase();
                        let start = normalized.find(['-', '/']).map_or(0, |index| index + 1);
                        format!("t3-{}", &normalized[start..start + 8])
                    };
                    break;
                }
            }
            w.branch = Some(Some(TrimmedNonEmptyString(name)));
        }
        rename &= !input.reused;
        let observer = Arc::new(PreparationObserver {
            thread: input.thread_id.clone(),
            claimed: Mutex::new(None),
            allow_claim: matches!(
                strategy,
                OrchestrationV2ThreadLaunchWorkspaceStrategy::Worktree(_)
            ),
            progress: self.0.progress.clone(),
        });
        let mut created: Option<Workspace> = None;
        let mut recorded = false;
        let mut released = false;
        let mut checkout_claim = None;
        let mut terminal = None;
        let result = async {
            if cancellation.is_cancelled() { return Err(Error::Cancelled) }
            if matches!(strategy,OrchestrationV2ThreadLaunchWorkspaceStrategy::Worktree(_)) {self.progress(input,"worktree").await?;}
            observer.status(SetupStage::Checkout,if matches!(strategy,OrchestrationV2ThreadLaunchWorkspaceStrategy::Worktree(_)) {StageStatus::Running} else {StageStatus::Skipped}).await?;
            let reservation = reserve_claim(&strategy, &project.workspace_root, &cancellation).await?;
            let mut workspace = self.0.preparation.prepare(input.project_id.clone(),input.thread_id.clone(),strategy.clone(),observer.clone(),cancellation.clone()).await.map_err(|e| if cancellation.is_cancelled() {Error::Cancelled} else {failure("provision-worktree",e)})?;
            if matches!(strategy,OrchestrationV2ThreadLaunchWorkspaceStrategy::Worktree(_)) {created = Some(workspace.clone());}
            if let Some(path) = &workspace.worktree_path {
                use super::checkpoint_restore_safety::{ReservationKind, WorkspaceReservations};
                checkout_claim = Some(WorkspaceReservations::global().reserve(path, ReservationKind::TurnStart, &cancellation).await?);
            }
            if matches!(strategy,OrchestrationV2ThreadLaunchWorkspaceStrategy::Worktree(_)) {observer.status(SetupStage::Checkout,StageStatus::Done).await?;}
            if !input.reused {
                self.dispatch(json!({"type":"thread.metadata.update","commandId":format!("{}:workspace",input.command_id.0),"threadId":input.thread_id,"branch":workspace.branch,"worktreePath":workspace.worktree_path}),None).await.map_err(|e| failure("update-thread",e))?;
            }
            drop(reservation);
            recorded = true;
            let rename_cancellation = cancellation.child_token();
            let naming = async {
                if rename && let (Some(path),Some(old),Some(message)) = (&workspace.worktree_path,&workspace.branch,&input.message) {
                    let generated = s.environment.generate_branch(&input.project_id,path.clone(),message,rename_cancellation.clone()).await?;
                    let renamed = s.environment.rename_branch(path.clone(),old,generated,rename_cancellation.clone()).await?;
                    Ok(Some((old.clone(),renamed)))
                } else {Ok(None)}
            };
            if workspace.worktree_path.is_some() {self.progress(input,"setup").await?;observer.status(SetupStage::SetupScript,StageStatus::Running).await?;}
            let setup = async {
                match &workspace.worktree_path {
                    Some(path) => s.environment.setup(SetupInput {project_id:input.project_id.clone(),thread_id:input.thread_id.clone(),project_cwd:PathBuf::from(&project.workspace_root),worktree_path:path.clone()},cancellation.clone()).await,
                    None => Ok(SetupRun::Skipped),
                }
            };
            let (naming, setup) = tokio::join!(naming,setup);
            if let Ok(SetupRun::Started {terminal_id,..})=&setup { terminal=Some(terminal_id.clone()); }
            match naming {
                Ok(Some((old,new))) => {
                    workspace.branch = Some(new.clone());
                    if let Some(created) = &mut created {created.branch = Some(new.clone());}
                    self.record_rename(input,&workspace, &old,&new).await;
                }
                Err(error) if !matches!(error,Error::Cancelled) => {self.rename_notice(input,workspace.branch.as_deref().unwrap_or(""),&error).await;}
                Err(error) => return Err(error),
                _ => {},
            }
            let setup = setup.map_err(|e| if cancellation.is_cancelled(){Error::Cancelled}else{failure("run-setup-script",e)})?;
            let mut asynchronous = None;
            if matches!(&setup,SetupRun::Skipped) {observer.status(SetupStage::SetupScript,StageStatus::Skipped).await?;}
            if let SetupRun::Started {terminal_id, asynchronous: is_async, completion} = setup {
                terminal = Some(terminal_id);
                if let Some(completion) = completion {
                    if is_async {asynchronous = Some(completion)} else {
                        tokio::select! {
                            _ = cancellation.cancelled() => return Err(Error::Cancelled),
                            result = completion => {let exit = result.map_err(|e| failure("run-setup-script",e))??;if exit != Some(0){observer.status(SetupStage::SetupScript,StageStatus::Error).await?;return Err(failure("run-setup-script",format!("Setup script exited with {exit:?}.")))} observer.status(SetupStage::SetupScript,StageStatus::Done).await?;}
                        }
                    }
                }
            }
            if cancellation.is_cancelled() {return Err(Error::Cancelled)}
            if let Some(run) = &input.run_id {
                observer.status(SetupStage::Agent,StageStatus::Running).await?;
                self.dispatch(json!({"type":"prepared-run.release","commandId":format!("{}:release",input.command_id.0),"threadId":input.thread_id,"runId":run}),None).await.map_err(|e| failure("release-run",e))?;
                released = true;
            }
            if let Some(completion) = asynchronous {
                tokio::select! {_ = cancellation.cancelled()=>return Err(Error::Cancelled), result = completion => {observer.status(SetupStage::SetupScript,if matches!(result,Ok(Ok(Some(0)))) {StageStatus::Done} else {StageStatus::Error}).await?;}}
            }
            Ok(())
        }.await;
        if result.is_err() {
            if created.is_none() {
                created = observer
                    .claimed
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
            }
            let cancelled = cancellation.is_cancelled() || matches!(result, Err(Error::Cancelled));
            if cancelled && let Some(terminal) = terminal {
                let _ = s
                    .environment
                    .close_terminal(&input.thread_id, &terminal)
                    .await;
            }
            if let Some(workspace) = created
                && ((cancelled && !released) || !recorded)
                && s.workspaces
                    .remove(workspace, CancellationToken::new())
                    .await
                    .is_ok()
            {
                let _ = self.dispatch(json!({"type":"thread.metadata.update","commandId":format!("{}:cancel-workspace",input.command_id.0),"threadId":input.thread_id,"worktreePath":null,"branch":null}),None).await;
            }
        }
        drop(checkout_claim);
        result
    }

    async fn progress(&self, input: &Preparation, phase: &str) -> Result<()> {
        if let Some(run) = &input.run_id {
            self.dispatch(json!({"type":"prepared-run.progress","commandId":format!("{}:progress:{phase}",input.command_id.0),"threadId":input.thread_id,"runId":run,"phase":phase}),None).await?;
        }
        Ok(())
    }

    async fn record_rename(
        &self,
        input: &Preparation,
        workspace: &Workspace,
        old: &str,
        new: &str,
    ) {
        let _ = self.dispatch(json!({"type":"thread.metadata.update","commandId":format!("{}:branch-rename",input.command_id.0),"threadId":input.thread_id,"branch":new,"worktreePath":workspace.worktree_path,"expectedBranch":old,"expectedWorktreePath":workspace.worktree_path}),None).await;
        let mut visited = HashSet::from([input.thread_id.clone()]);
        loop {
            let Ok(snapshot) = self
                .0
                .services
                .projections
                .thread_shells(ShellQuery {
                    worktree_path: workspace
                        .worktree_path
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned()),
                    ..Default::default()
                })
                .await
            else {
                break;
            };
            let sharing: Vec<_> = snapshot
                .threads
                .into_iter()
                .chain(snapshot.archived_threads)
                .filter(|t| {
                    !visited.contains(&t.id) && t.branch.as_ref().map(|b| b.0.as_str()) == Some(old)
                })
                .collect();
            if sharing.is_empty() {
                break;
            }
            for thread in sharing {
                visited.insert(thread.id.clone());
                let _ = self.dispatch(json!({"type":"thread.metadata.update","commandId":format!("{}:branch-rename:{}",input.command_id.0,thread.id.0),"threadId":thread.id,"branch":new,"expectedBranch":old,"expectedWorktreePath":workspace.worktree_path}),None).await;
            }
        }
    }

    async fn rename_notice(&self, input: &Preparation, branch: &str, error: &Error) {
        if let Some(run) = &input.run_id {
            let s = &self.0.services;
            let now = s.clock.now();
            let item = json!({"id":format!("turn-item:run:{}:signal:branch-rename",super::checkpoint_service::encode_id_part(&run.0)),"threadId":input.thread_id,"runId":run,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":"Branch not renamed","startedAt":now,"completedAt":now,"updatedAt":now,"type":"system_notice","message":format!("Could not rename the worktree branch, so it stays {branch}. {error}")});
            if let Ok(event) = domain_event(
                s.ids.as_ref(),
                &input.thread_id,
                &now,
                "turn-item.updated",
                item,
                json!({"runId":run}),
            ) {
                let _ = s
                    .events
                    .commit(CommitRequest {
                        receipt: None,
                        guard: CommitGuard::None,
                        decision: Decision {
                            events: vec![event],
                            ..Default::default()
                        },
                    })
                    .await;
            }
        }
    }
}
