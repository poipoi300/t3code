use crate::{Error, Result, clamp_text};
use async_trait::async_trait;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use t3_contracts::{
    base_schemas::ProjectId, project_clone::*, source_control::SourceControlRepositoryInfo,
};
use t3_orchestration::api::Clock;
use tokio::{
    sync::{Mutex as AsyncMutex, watch},
    task::JoinHandle,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[derive(Debug, Clone, PartialEq)]
pub struct CloneProgress {
    pub stage: ProjectCloneStage,
    pub percent: Option<i64>,
    pub detail: Option<String>,
}

pub fn parse_git_clone_progress_line(line: &str) -> Option<CloneProgress> {
    t3_forge::clone_progress::parse_git_clone_progress_line(line).map(|p| CloneProgress {
        stage: p.stage,
        percent: p.percent.map(i64::from),
        detail: p.detail,
    })
}

/// Prepared URLs are deliberately not Debug: clone_url may contain credentials.
#[derive(Clone)]
pub struct PreparedClone {
    pub destination_path: String,
    pub remote_url: String,
    pub clone_url: String,
    pub repository: Option<SourceControlRepositoryInfo>,
}

#[async_trait]
pub trait CloneRepository: Send + Sync {
    async fn prepare(&self, input: &ProjectCloneStartInput) -> Result<PreparedClone>;
    /// Uses t3-git, with no execution deadline; cancellation must kill and reap owned subprocesses.
    async fn clone_repository(
        &self,
        input: &ProjectCloneStartInput,
        prepared: &PreparedClone,
        progress: Arc<dyn Fn(CloneProgress) + Send + Sync>,
        cancel: CancellationToken,
    ) -> Result<()>;
    /// Refuses non-clone contents, symlink/junction redirection and inspection errors.
    async fn discard(&self, destination: &str) -> Result<()>;
}

pub struct ForgeCloneRepository {
    pub registry: Arc<t3_forge::registry::ForgeRegistry>,
    pub git: t3_git::Git,
    pub cwd: std::path::PathBuf,
    pub home: std::path::PathBuf,
}
impl ForgeCloneRepository {
    fn service(
        &self,
        cancel: CancellationToken,
    ) -> t3_forge::services::SourceControlRepositoryService {
        t3_forge::services::SourceControlRepositoryService::new(
            self.registry.clone(),
            self.git.clone().with_cancellation(cancel),
            self.cwd.clone(),
        )
        .with_home(self.home.clone())
    }
}
fn clone_input(
    input: &ProjectCloneStartInput,
) -> Result<t3_contracts::source_control::SourceControlCloneRepositoryInput> {
    Ok(serde_json::from_value(serde_json::to_value(input)?)?)
}
fn repository_error(error: t3_forge::services::RepositoryError) -> Error {
    Error::Operation {
        operation: "cloneRepository",
        detail: error.wire.detail,
    }
}
#[async_trait]
impl CloneRepository for ForgeCloneRepository {
    async fn prepare(&self, input: &ProjectCloneStartInput) -> Result<PreparedClone> {
        let prepared = self
            .service(CancellationToken::new())
            .prepare_clone(clone_input(input)?)
            .await
            .map_err(repository_error)?;
        Ok(PreparedClone {
            destination_path: prepared.destination_path.to_string_lossy().into(),
            remote_url: prepared.remote_url,
            clone_url: prepared.clone_url,
            repository: prepared.repository,
        })
    }
    async fn clone_repository(
        &self,
        input: &ProjectCloneStartInput,
        prepared: &PreparedClone,
        progress: Arc<dyn Fn(CloneProgress) + Send + Sync>,
        cancel: CancellationToken,
    ) -> Result<()> {
        let callback = Arc::new(move |p: t3_forge::clone_progress::GitCloneProgressLine| {
            progress(CloneProgress {
                stage: p.stage,
                percent: p.percent.map(i64::from),
                detail: p.detail,
            })
        });
        let service = self.service(cancel.clone());
        let result = service
            .clone_repository(
                t3_contracts::source_control::SourceControlCloneRepositoryInput {
                    provider: None,
                    repository: None,
                    remote_url: Some(Some(prepared.clone_url.clone().into())),
                    destination_path: prepared.destination_path.clone().into(),
                    protocol: input.protocol.clone(),
                },
                t3_forge::services::CloneOptions {
                    timeout: None,
                    on_progress: Some(callback),
                },
            )
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(_) if cancel.is_cancelled() => Err(Error::Cancelled),
            Err(e) => Err(repository_error(e)),
        }
    }
    async fn discard(&self, destination: &str) -> Result<()> {
        self.service(CancellationToken::new())
            .discard_clone(std::path::Path::new(destination))
            .await
            .map_err(repository_error)
    }
}
#[async_trait]
pub trait CloneHooks: Send + Sync {
    async fn create_project(
        &self,
        input: &ProjectCloneStartInput,
        prepared: &PreparedClone,
    ) -> Result<()>;
    async fn on_cloned(&self, project_id: &ProjectId, workspace_root: &str) -> Result<()>;
}
struct Tracked {
    order: u64,
    snapshot: ProjectCloneSnapshot,
    input: ProjectCloneStartInput,
    prepared: PreparedClone,
    hooks: Arc<dyn CloneHooks>,
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
    generation: u64,
}
#[derive(Default)]
struct State {
    entries: BTreeMap<String, Tracked>,
    sequence: u64,
    closed: bool,
}
struct Inner {
    state: Mutex<State>,
    actions: AsyncMutex<()>,
    changes: watch::Sender<Vec<ProjectCloneSnapshot>>,
    backend: Arc<dyn CloneRepository>,
    clock: Arc<dyn Clock>,
    tasks: TaskTracker,
    stop: CancellationToken,
}
#[derive(Clone)]
pub struct ProjectCloneTracker(Arc<Inner>);
impl ProjectCloneTracker {
    pub fn new(backend: Arc<dyn CloneRepository>, clock: Arc<dyn Clock>) -> Self {
        Self(Arc::new(Inner {
            state: Mutex::new(State::default()),
            actions: AsyncMutex::new(()),
            changes: watch::channel(vec![]).0,
            backend,
            clock,
            tasks: TaskTracker::new(),
            stop: CancellationToken::new(),
        }))
    }
    fn publish(&self, s: &State) {
        let mut entries = s.entries.values().collect::<Vec<_>>();
        entries.sort_by_key(|e| e.order);
        self.0
            .changes
            .send_replace(entries.into_iter().map(|e| e.snapshot.clone()).collect());
    }
    pub fn subscribe_project_clones(&self) -> watch::Receiver<Vec<ProjectCloneSnapshot>> {
        self.0.changes.subscribe()
    }
    pub fn get(&self, id: &ProjectId) -> Option<ProjectCloneSnapshot> {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .get(&id.0)
            .map(|e| e.snapshot.clone())
    }
    pub async fn start(
        &self,
        input: ProjectCloneStartInput,
        hooks: Arc<dyn CloneHooks>,
    ) -> Result<ProjectCloneStartResult> {
        let task = {
            let state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.closed {
                return Err(Error::Unavailable("ProjectCloneTracker is shut down"));
            }
            let this = self.clone();
            self.0
                .tasks
                .spawn(async move { this.start_inner(input, hooks).await })
        };
        task.await?
    }
    async fn start_inner(
        &self,
        mut input: ProjectCloneStartInput,
        hooks: Arc<dyn CloneHooks>,
    ) -> Result<ProjectCloneStartResult> {
        let _action = self.0.actions.lock().await;
        let prepared = self.0.backend.prepare(&input).await?;
        let original = input.project_id.clone();
        {
            let mut s = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.entries.values().any(|e| {
                e.snapshot.project_id == input.project_id
                    || e.prepared.destination_path == prepared.destination_path
            }) {
                return Err(Error::Invalid(
                    "A clone into this destination is already in progress.".into(),
                ));
            }
            s.sequence += 1;
            let sequence = s.sequence;
            let snapshot = ProjectCloneSnapshot {
                project_id: input.project_id.clone(),
                remote_url: prepared.remote_url.clone().into(),
                destination_path: prepared.destination_path.clone().into(),
                repository: prepared.repository.clone(),
                phase: ProjectClonePhase::Running,
                stage: ProjectCloneStage::Connecting,
                percent: None,
                detail: None,
                stalled: Some(false),
                error: None,
                started_at: self.0.clock.now(),
                ended_at: None,
                sequence,
            };
            s.entries.insert(
                input.project_id.0.clone(),
                Tracked {
                    order: sequence,
                    snapshot,
                    input: input.clone(),
                    prepared: prepared.clone(),
                    hooks: hooks.clone(),
                    cancel: CancellationToken::new(),
                    task: None,
                    generation: sequence,
                },
            );
        }
        match hooks.create_project(&input, &prepared).await {
            Ok(()) => {}
            Err(Error::Conflict {
                conflicting_project_id,
                ..
            }) => {
                let mut s = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
                if s.entries.contains_key(&conflicting_project_id) {
                    s.entries.remove(&original.0);
                    self.publish(&s);
                    return Err(Error::Invalid(
                        "A clone into this destination is already in progress.".into(),
                    ));
                }
                if let Some(mut e) = s.entries.remove(&original.0) {
                    input.project_id = ProjectId(conflicting_project_id.clone());
                    e.snapshot.project_id = input.project_id.clone();
                    e.input.project_id = input.project_id.clone();
                    s.entries.insert(conflicting_project_id, e);
                }
            }
            Err(error) => {
                let mut s = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
                s.entries.remove(&original.0);
                self.publish(&s);
                return Err(error);
            }
        }
        {
            let s = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
            self.publish(&s);
        }
        self.launch(&input.project_id);
        Ok(ProjectCloneStartResult {
            project_id: input.project_id,
            cwd: prepared.destination_path.into(),
            remote_url: prepared.remote_url.into(),
            repository: prepared.repository,
        })
    }
    fn modify(&self, id: &ProjectId, generation: u64, f: impl FnOnce(&mut ProjectCloneSnapshot)) {
        let mut s = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if !s
            .entries
            .get(&id.0)
            .is_some_and(|e| e.generation == generation)
        {
            return;
        }
        s.sequence += 1;
        let seq = s.sequence;
        if let Some(e) = s.entries.get_mut(&id.0) {
            f(&mut e.snapshot);
            e.snapshot.sequence = seq;
        }
        self.publish(&s);
    }
    fn launch(&self, id: &ProjectId) {
        let mut s = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(e) = s.entries.get_mut(&id.0) else {
            return;
        };
        let (input, prepared, hooks, cancel, generation) = (
            e.input.clone(),
            e.prepared.clone(),
            e.hooks.clone(),
            e.cancel.clone(),
            e.generation,
        );
        let this = self.clone();
        let id = id.clone();
        e.task=Some(self.0.tasks.spawn(async move {
            let (pulse,mut pulses)=watch::channel(0u64);
            let progress_this=this.clone();let progress_id=id.clone();
            let progress=Arc::new(move |update:CloneProgress| {
                pulse.send_modify(|n|*n+=1);
                progress_this.modify(&progress_id,generation,|s|{s.stage=update.stage;s.percent=update.percent;s.detail=update.detail.map(|s|clamp_text(&s,PROJECT_CLONE_DETAIL_MAX_LENGTH));s.stalled=Some(false);});
            });
            let stall_this=this.clone();let stall_id=id.clone();
            let stall=async move {
                loop {
                    match tokio::time::timeout(Duration::from_secs(20),pulses.changed()).await {
                        Ok(Ok(()))=>{},Ok(Err(_))=>return,Err(_)=>{
                            stall_this.modify(&stall_id,generation,|s|s.stalled=Some(true));
                            if pulses.changed().await.is_err(){return;}
                        }
                    }
                }
            };
            let run=this.0.backend.clone_repository(&input,&prepared,progress,cancel.clone());
            tokio::pin!(run);tokio::pin!(stall);
            let result=tokio::select! {biased; r=&mut run=>r,_=this.0.stop.cancelled()=>{cancel.cancel();run.await},_=cancel.cancelled()=>run.await,_=&mut stall=>run.await};
            let (phase,error)=match result {Ok(())=>(ProjectClonePhase::Done,None),Err(Error::Cancelled)=>(ProjectClonePhase::Cancelled,None),Err(error)=>(ProjectClonePhase::Failed,Some(clamp_text(&error.to_string(),PROJECT_CLONE_ERROR_MAX_LENGTH)))};
            this.modify(&id,generation,|s|{s.phase=phase.clone();s.error=error;s.ended_at=Some(this.0.clock.now());s.stalled=Some(false);if phase==ProjectClonePhase::Done{s.percent=Some(100);}});
            if phase==ProjectClonePhase::Done {
                let retention=this.clone();let retain_id=id.clone();
                let expires=tokio::time::Instant::now()+Duration::from_secs(30);
                this.0.tasks.spawn(async move {tokio::select! {_=retention.0.stop.cancelled()=>{},_=tokio::time::sleep_until(expires)=>{
                    let mut s=retention.0.state.lock().unwrap_or_else(|e|e.into_inner());
                    if s.entries.get(&retain_id.0).is_some_and(|e|e.generation==generation&&e.snapshot.phase==ProjectClonePhase::Done) {s.entries.remove(&retain_id.0);retention.publish(&s);}
                }}});
                tokio::select! {biased;_=cancel.cancelled()=>{},_=this.0.stop.cancelled()=>{},_=hooks.on_cloned(&id,&prepared.destination_path)=>{}}
            }
        }));
    }
    async fn stop_task(&self, id: &ProjectId) {
        let task = {
            let mut s = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
            s.entries.get_mut(&id.0).and_then(|e| {
                e.cancel.cancel();
                e.task.take()
            })
        };
        if let Some(task) = task {
            let _ = task.await;
        }
    }
    pub async fn cancel(&self, id: &ProjectId) -> Result<bool> {
        let this = self.clone();
        let id = id.clone();
        self.0
            .tasks
            .spawn(async move {
                let _action = this.0.actions.lock().await;
                if !this
                    .get(&id)
                    .is_some_and(|s| s.phase == ProjectClonePhase::Running)
                {
                    return Ok(false);
                }
                this.stop_task(&id).await;
                if let Some(s) = this.get(&id)
                    && s.phase != ProjectClonePhase::Done
                {
                    let _ = this.0.backend.discard(&s.destination_path.0).await;
                }
                Ok(true)
            })
            .await?
    }
    pub async fn retry(&self, id: &ProjectId) -> Result<bool> {
        let _action = self.0.actions.lock().await;
        let Some(snapshot) = self.get(id) else {
            return Ok(false);
        };
        if !matches!(
            snapshot.phase,
            ProjectClonePhase::Failed | ProjectClonePhase::Cancelled
        ) {
            return Ok(false);
        }
        self.stop_task(id).await;
        self.0.backend.discard(&snapshot.destination_path.0).await?;
        {
            let mut s = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.closed {
                return Err(Error::Unavailable("ProjectCloneTracker is shut down"));
            }
            s.sequence += 1;
            let seq = s.sequence;
            if let Some(e) = s.entries.get_mut(&id.0) {
                e.generation = seq;
                e.cancel = CancellationToken::new();
                e.snapshot.phase = ProjectClonePhase::Running;
                e.snapshot.stage = ProjectCloneStage::Connecting;
                e.snapshot.percent = None;
                e.snapshot.detail = None;
                e.snapshot.stalled = Some(false);
                e.snapshot.error = None;
                e.snapshot.started_at = self.0.clock.now();
                e.snapshot.ended_at = None;
                e.snapshot.sequence = seq;
            }
            self.publish(&s);
        }
        self.launch(id);
        Ok(true)
    }
    pub async fn discard(&self, id: &ProjectId) -> Result<()> {
        let _action = self.0.actions.lock().await;
        self.stop_task(id).await;
        if let Some(s) = self.get(id)
            && s.phase != ProjectClonePhase::Done
        {
            let _ = self.0.backend.discard(&s.destination_path.0).await;
        }
        let mut s = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        s.entries.remove(&id.0);
        self.publish(&s);
        Ok(())
    }
    pub fn reject_commands_during_clone(&self, command: &serde_json::Value) -> Result<()> {
        let id = match command["type"].as_str() {
            Some("thread.create") => command["projectId"].as_str(),
            Some("thread.turn.start") => command["bootstrap"]["createThread"]["projectId"].as_str(),
            _ => None,
        };
        if let Some(snapshot) = id.and_then(|id| self.get(&ProjectId(id.into())))
            && snapshot.phase != ProjectClonePhase::Done
        {
            return Err(Error::Invalid(
                if snapshot.phase == ProjectClonePhase::Running {
                    "The repository is still being cloned."
                } else {
                    "The repository was not cloned. Retry the clone first."
                }
                .into(),
            ));
        }
        Ok(())
    }
    pub async fn shutdown(&self) {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .closed = true;
        self.0.stop.cancel();
        self.0.tasks.close();
        self.0.tasks.wait().await;
    }
}
