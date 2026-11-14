use crate::{Error, ProjectService, Result, managed::ManagedProjectFolders};
use async_trait::async_trait;
use std::{
    collections::BTreeSet,
    path::Path,
    sync::{Arc, Mutex},
};
use t3_contracts::{
    base_schemas::{ProjectId, ThreadId},
    project::{ProjectCreateFromThreadInput, ProjectCreateFromThreadResult},
};
use t3_orchestration::{
    api::{CommandDispatcher, ProjectionReader, ShellQuery},
    ports::decisions::orchestrator::thread_has_unfinished_work,
};
use tokio_util::task::TaskTracker;

#[async_trait]
pub trait MoveThreadTasks: Send + Sync {
    async fn move_thread_tasks(&self, threads: Vec<ThreadId>, project: ProjectId) -> Result<()>;
}
#[async_trait]
impl MoveThreadTasks for t3_scheduler::ScheduledTaskService {
    async fn move_thread_tasks(&self, threads: Vec<ThreadId>, project: ProjectId) -> Result<()> {
        t3_scheduler::ScheduledTaskService::move_thread_tasks(self, &threads, &project)
            .await
            .map_err(|e| Error::Operation {
                operation: "moveThreadTasks",
                detail: e.to_string(),
            })
    }
}
#[derive(Clone)]
pub struct ScratchThreadPromotion {
    pub projects: ProjectService,
    pub folders: ManagedProjectFolders,
    pub threads: Arc<dyn ProjectionReader>,
    pub commands: Arc<dyn CommandDispatcher>,
    pub scheduled_tasks: Arc<dyn MoveThreadTasks>,
    scope: Arc<PromotionScope>,
}
struct PromotionScope {
    admission: Mutex<bool>,
    tasks: TaskTracker,
}
impl ScratchThreadPromotion {
    pub fn new(
        projects: ProjectService,
        folders: ManagedProjectFolders,
        threads: Arc<dyn ProjectionReader>,
        commands: Arc<dyn CommandDispatcher>,
        scheduled_tasks: Arc<dyn MoveThreadTasks>,
    ) -> Self {
        Self {
            projects,
            folders,
            threads,
            commands,
            scheduled_tasks,
            scope: Arc::new(PromotionScope {
                admission: Mutex::new(true),
                tasks: TaskTracker::new(),
            }),
        }
    }
    pub async fn create_from_thread(
        &self,
        input: ProjectCreateFromThreadInput,
    ) -> Result<ProjectCreateFromThreadResult> {
        let _foreground = {
            let admission = self
                .scope
                .admission
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if !*admission {
                return Err(Error::Unavailable("ScratchThreadPromotion is shut down"));
            }
            self.scope.tasks.token()
        };
        let thread = self
            .threads
            .shell(input.thread_id)
            .await?
            .ok_or_else(|| Error::Invalid("Thread not found.".into()))?;
        let project = self.projects.get_by_id(&thread.project_id, false).await?;
        let folder = thread
            .worktree_path
            .as_ref()
            .map(|s| s.0.clone())
            .or_else(|| project.as_ref().map(|p| p.workspace_root.0.clone()));
        let scratch = self.folders.scratch_root().await;
        let is_scratch = self.folders.is_scratch_project(&thread.project_id).await;
        let already_promoted = project
            .as_ref()
            .zip(folder.as_ref())
            .is_some_and(|(p, f)| p.workspace_root.0 == *f && !is_scratch);
        let fail = || {
            Error::Invalid(
                "Only a No project thread with its own folder can become a project.".into(),
            )
        };
        let folder = folder.ok_or_else(fail)?;
        let scratch = scratch.ok_or_else(fail)?;
        let parent = Path::new(&folder).parent().ok_or_else(fail)?;
        let real_parent =
            t3_workspace::paths::canonicalize(parent).unwrap_or_else(|_| parent.into());
        let real_scratch = t3_workspace::paths::canonicalize(&scratch).unwrap_or(scratch);
        if real_parent != real_scratch || !(is_scratch || already_promoted) {
            return Err(fail());
        }
        let snapshot = self
            .threads
            .thread_shells(ShellQuery {
                worktree_path: Some(folder.clone()),
                ..Default::default()
            })
            .await?;
        let mut seen = BTreeSet::new();
        let mut moves = Vec::new();
        if is_scratch {
            seen.insert(thread.id.clone());
            moves.push(thread.clone());
        }
        for candidate in snapshot
            .threads
            .into_iter()
            .chain(snapshot.archived_threads)
        {
            if self.folders.is_scratch_project(&candidate.project_id).await
                && seen.insert(candidate.id.clone())
            {
                moves.push(candidate);
            }
        }
        for candidate in &moves {
            use t3_orchestration::api::RecordKind;
            let projection = crate::projection::selected_projection(
                self.threads.as_ref(),
                candidate.id.clone(),
                vec![RecordKind::Runs, RecordKind::RuntimeRequests],
            )
            .await?;
            if thread_has_unfinished_work(&projection) {
                return Err(Error::Invalid(format!(
                    "\"{}\" is still working. Finish or stop it first.",
                    candidate.title
                )));
            }
        }
        let this = self.clone();
        let task = {
            let admission = self
                .scope
                .admission
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if !*admission {
                return Err(Error::Unavailable("ScratchThreadPromotion is shut down"));
            }
            self.scope.tasks.spawn(async move {
            let id=uuid::Uuid::new_v4().to_string();
            let create=serde_json::from_value(serde_json::json!({"type":"project.create","commandId":format!("scratch-promotion-project:{id}"),"projectId":id,"title":thread.title,"workspaceRoot":folder}))?;
            let project_id=match this.projects.bootstrap(create).await {Ok((p,_))=>p.id,Err(Error::Conflict{conflicting_project_id,..})=>ProjectId(conflicting_project_id),Err(e)=>return Err(e)};
            this.scheduled_tasks.move_thread_tasks(moves.iter().map(|t|t.id.clone()).collect(),project_id.clone()).await?;
            for id in moves.iter().map(|t|&t.id) {
                this.commands.dispatch(serde_json::from_value(serde_json::json!({"type":"thread.project.adopt-folder","commandId":format!("scratch-promotion:{}",uuid::Uuid::new_v4()),"threadId":id,"projectId":project_id}))?).await?;
            }
            Ok(ProjectCreateFromThreadResult {project_id})
            })
        };
        task.await?
    }
    pub async fn shutdown(&self) {
        *self
            .scope
            .admission
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = false;
        self.scope.tasks.close();
        self.scope.tasks.wait().await;
    }
}

pub async fn thread_workspace_root(
    threads: &dyn ProjectionReader,
    projects: &ProjectService,
    id: ThreadId,
) -> Option<String> {
    let thread = threads.shell(id).await.ok()??;
    if let Some(path) = thread.worktree_path {
        return Some(path.0);
    }
    Some(
        projects
            .get_by_id(&thread.project_id, false)
            .await
            .ok()??
            .workspace_root
            .0,
    )
}
