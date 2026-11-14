//! IndexDraftLanding, useHandleNewThread, useScratchProject and draft creation/send composition.
mod default_project;
mod drafts;
pub use default_project::default_project;
mod links;
pub use links::{LinkAction, LinkChoice, LinkText};
mod open;
mod policy;
mod submit;
mod view;
use crate::{
    ClientError,
    composition::ClientRuntime,
    operations::{self, ComposerDispatchMode},
    platform::Platform,
    services::resources::{Resource, ResourcePolicy},
    thread_views::{
        composer::{
            attachment_intake::AttachmentSession,
            send::{SendOptions, SendState},
            target::ComposerTarget,
            web_drafts, web_models, web_sessions,
        },
        drafts::web_has_user_content,
        outbox::{DispatchMode, QueuedCreation},
    },
    types::{EnvironmentId, Generation, SyncStatus, ThreadId, ThreadKey},
    workspace::{
        ProjectKey,
        project_ops::ProjectCommands,
        projects::{self, EnvironmentProject, GroupingSettings, LogicalProjectKey},
    },
};
pub use drafts::{DraftAuthority, DraftEdit};
use futures::{StreamExt, stream::LocalBoxStream};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use t3_contracts::{
    environment::ScopedProjectRef,
    orchestration_v2::OrchestrationV2Command,
    rpc::{Empty, WsOrchestrationV2DispatchCommandRpc, WsProjectsEnsureScratchRpc},
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    Home,
    Draft(String),
    Link(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    Draft(String),
    Thread(ThreadKey),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectChoice {
    pub key: ProjectKey,
    pub title: String,
    pub environment_label: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct DraftView {
    pub id: String,
    pub key: ThreadKey,
    pub prompt: String,
    pub cursor: usize,
    pub revision: u64,
    pub project: ProjectKey,
    pub project_title: String,
    pub projects: Vec<ProjectChoice>,
    pub is_scratch: bool,
    pub can_start_scratch: bool,
    pub editable: bool,
    pub can_operate: bool,
    pub busy: bool,
    pub has_content: bool,
    pub unavailable: bool,
    pub disabled_reason: Option<String>,
    pub banner: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum NewThreadView {
    Loading,
    Empty {
        can_start_scratch: bool,
    },
    StartFailed,
    LinkFailed {
        title: String,
        message: String,
    },
    LinkChoice {
        title: LinkText,
        message: LinkText,
        choices: Vec<LinkChoice>,
    },
    Draft(Rc<DraftView>),
    Navigate(Destination),
}
pub struct NewThreadState {
    changed: Resource<()>,
    busy: RefCell<BTreeSet<String>>,
    attempts: RefCell<BTreeMap<String, u64>>,
    next: Cell<u64>,
    destinations: RefCell<BTreeMap<Target, Destination>>,
    errors: RefCell<BTreeMap<Target, String>>,
    views: RefCell<BTreeMap<String, Rc<DraftView>>>,
    grouping: RefCell<GroupingSettings>,
    project_order: RefCell<Vec<String>>,
    last_used: RefCell<Option<String>>,
    links: RefCell<BTreeMap<String, links::LinkSession>>,
}
impl Default for NewThreadState {
    fn default() -> Self {
        Self {
            changed: Resource::new(ResourcePolicy::default(), Generation::default()),
            busy: RefCell::default(),
            attempts: RefCell::default(),
            next: Cell::new(0),
            destinations: RefCell::default(),
            errors: RefCell::default(),
            views: RefCell::default(),
            grouping: RefCell::default(),
            project_order: RefCell::default(),
            last_used: RefCell::default(),
            links: RefCell::default(),
        }
    }
}
#[derive(Clone)]
pub struct NewThreadRuntime<P: Platform + Clone + 'static> {
    runtime: ClientRuntime<P>,
    platform: P,
    drafts: Rc<dyn DraftAuthority>,
    state: Rc<NewThreadState>,
}
impl<P: Platform + Clone + 'static> NewThreadRuntime<P> {
    pub fn new(runtime: ClientRuntime<P>, platform: P, state: Rc<NewThreadState>) -> Self {
        let drafts = Rc::new(drafts::ComposerAuthority(runtime.clone()));
        Self::with_drafts(runtime, platform, state, drafts)
    }
    pub fn with_drafts(
        runtime: ClientRuntime<P>,
        platform: P,
        state: Rc<NewThreadState>,
        drafts: Rc<dyn DraftAuthority>,
    ) -> Self {
        Self {
            runtime,
            platform,
            state,
            drafts,
        }
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        futures::stream::select_all(vec![
            self.runtime.store().subscribe().map(|_| ()).boxed_local(),
            self.drafts.changes(),
            self.runtime.message_changes(),
            self.state
                .changed
                .observe(self.platform.monotonic_ms())
                .map(|_| ())
                .boxed_local(),
        ])
        .boxed_local()
    }
    pub fn watch(&self, target: Target) -> LocalBoxStream<'static, Rc<NewThreadView>> {
        let changes = self.changes();
        let this = self.clone();
        futures::stream::unfold(
            (this, target, changes),
            |(this, target, mut changes)| async move {
                changes.next().await?;
                let view = match this.resolve(&target).await {
                    Ok(view) => view,
                    Err(error) => {
                        this.state
                            .errors
                            .borrow_mut()
                            .insert(target.clone(), error.to_string());
                        NewThreadView::StartFailed
                    }
                };
                Some((Rc::new(view), (this, target, changes)))
            },
        )
        .boxed_local()
    }
    pub fn set_grouping(&self, grouping: GroupingSettings) {
        *self.state.grouping.borrow_mut() = grouping;
        self.state.changed.invalidate();
    }
    pub fn set_start_preferences(&self, order: Vec<String>, last_used: Option<String>) {
        *self.state.project_order.borrow_mut() = order;
        *self.state.last_used.borrow_mut() = last_used;
        self.state.changed.invalidate();
    }
    pub fn projects(&self) -> Vec<EnvironmentProject> {
        let snapshot = self.runtime.store().snapshot();
        let mut projects = snapshot
            .environments
            .iter()
            .flat_map(|(environment, state)| {
                state.shell.snapshot.iter().flat_map(move |shell| {
                    shell
                        .projects
                        .iter()
                        .map(move |project| EnvironmentProject {
                            environment: environment.clone(),
                            project: project.clone(),
                            is_scratch: state
                                .config
                                .as_ref()
                                .and_then(|c| c.scratch_workspace_root.as_ref())
                                .is_some_and(|root| {
                                    projects::normalize_project_path_for_comparison(&root.0)
                                        == projects::normalize_project_path_for_comparison(
                                            &project.workspace_root.0,
                                        )
                                }),
                        })
                })
            })
            .filter(|p| p.project.archived_at.as_ref().is_none_or(Option::is_none))
            .collect::<Vec<_>>();
        let activity = |project: &EnvironmentProject| {
            let rows = snapshot
                .environments
                .get(&project.environment)
                .and_then(|e| e.shell.snapshot.as_ref())
                .map(|s| &s.threads);
            let times = rows
                .into_iter()
                .flatten()
                .filter(|t| t.project_id == project.project.id)
                .map(|t| crate::time::epoch_ms(&t.updated_at.0))
                .collect::<Vec<_>>();
            if times.is_empty() {
                crate::time::epoch_ms(&project.project.updated_at.0)
            } else {
                times.into_iter().flatten().max()
            }
        };
        projects.sort_by(|a, b| {
            activity(b)
                .cmp(&activity(a))
                .then_with(|| a.project.title.0.cmp(&b.project.title.0))
                .then_with(|| a.key().cmp(&b.key()))
        });
        projects
    }
    fn project(&self, key: &ProjectKey) -> Result<EnvironmentProject, ClientError> {
        self.projects()
            .into_iter()
            .find(|p| p.key() == *key)
            .ok_or_else(|| {
                ClientError::InvalidPayload("The project is no longer available.".into())
            })
    }
    pub fn draft_key(&self, id: &str) -> Option<ThreadKey> {
        let doc = self.drafts.document();
        let doc = doc.borrow();
        let s = &doc["draftThreadsByThreadKey"][id];
        Some(ThreadKey {
            environment: EnvironmentId(s["environmentId"].as_str()?.into()),
            thread: ThreadId(s["threadId"].as_str()?.into()),
        })
    }
    pub fn attachments(&self, id: &str) -> Option<AttachmentSession> {
        let key = self.draft_key(id)?;
        self.drafts
            .attachments(&ComposerTarget::Draft {
                draft_id: id.into(),
                environment_id: Some(key.environment),
            })
            .ok()
    }
    pub fn model_session(
        &self,
        id: &str,
    ) -> Result<crate::thread_views::composer::model_service::ComposerModelsSession<P>, ClientError>
    {
        let key = self.draft_key(id).ok_or(ClientError::Cancelled)?;
        Ok(self.runtime.composer_models(&key))
    }
    pub fn can_operate(&self, key: &ThreadKey) -> bool {
        let input: OrchestrationV2Command = serde_json::from_value(
            json!({"type":"thread.stop","commandId":"availability","threadId":key.thread}),
        )
        .expect("thread operation availability");
        self.runtime
            .client(&key.environment)
            .and_then(|c| c.session())
            .is_some_and(|s| s.available::<WsOrchestrationV2DispatchCommandRpc>(&input))
    }
    fn scratch_environment(&self, fallback: Option<&EnvironmentId>) -> Option<EnvironmentId> {
        let snapshot = self.runtime.store().snapshot();
        let offering = self
            .runtime
            .environments()
            .into_iter()
            .filter(|e| {
                operations::available_scratch_workspace_root(
                    Some(e.state.phase),
                    snapshot
                        .environments
                        .get(&e.id)
                        .and_then(|s| s.config.as_deref()),
                )
                .is_some()
            })
            .map(|e| e.id)
            .collect::<Vec<_>>();
        let primary = self
            .runtime
            .environments()
            .into_iter()
            .find(|e| {
                e.routes
                    .iter()
                    .any(|r| r.mode == crate::connection::ConnectionMode::Local)
            })
            .map(|e| e.id);
        primary
            .filter(|id| offering.contains(id))
            .or_else(|| fallback.filter(|id| offering.contains(id)).cloned())
            .or_else(|| (offering.len() == 1).then(|| offering[0].clone()))
    }
    pub async fn resolve(&self, target: &Target) -> Result<NewThreadView, ClientError> {
        self.drafts.load().await?;
        if let Some(destination) = self.state.destinations.borrow().get(target).cloned() {
            return Ok(NewThreadView::Navigate(destination));
        }
        match target {
            Target::Home => {
                if self.state.errors.borrow().contains_key(target) {
                    return Ok(NewThreadView::StartFailed);
                }
                let snapshot = self.runtime.store().snapshot();
                let ready = !snapshot.environments.is_empty()
                    && snapshot
                        .environments
                        .values()
                        .all(|e| e.shell.status == SyncStatus::Live);
                let project = default_project(
                    &self.projects(),
                    &self.state.project_order.borrow(),
                    self.state.last_used.borrow().as_deref(),
                    ready,
                );
                if let Some(project) = project {
                    let id = self.open_project(project, None, None).await?;
                    return Ok(NewThreadView::Navigate(Destination::Draft(id)));
                }
                Ok(if ready {
                    NewThreadView::Empty {
                        can_start_scratch: self.scratch_environment(None).is_some(),
                    }
                } else {
                    NewThreadView::Loading
                })
            }
            Target::Draft(id) => self.draft_view(id),
            Target::Link(search) => self.resolve_link(search).await,
        }
    }
    pub async fn retry(&self, target: Target) -> Result<(), ClientError> {
        self.state.errors.borrow_mut().remove(&target);
        self.state.destinations.borrow_mut().remove(&target);
        let result = self.resolve(&target).await;
        self.state.changed.invalidate();
        result.map(|_| ())
    }
    pub fn set_prompt(&self, id: &str, prompt: String, cursor: usize) -> Result<u64, ClientError> {
        self.drafts.set_prompt(id, prompt, cursor)
    }
    pub async fn select_project(&self, id: String, project: ProjectKey) -> Result<(), ClientError> {
        let destination = self.open_project(project, Some(&id), None).await?;
        if destination != id {
            self.state
                .destinations
                .borrow_mut()
                .insert(Target::Draft(id), Destination::Draft(destination));
        }
        self.state.changed.invalidate();
        Ok(())
    }
    pub async fn start_scratch(&self, id: Option<String>) -> Result<(), ClientError> {
        let fallback = id
            .as_deref()
            .and_then(|id| self.draft_key(id))
            .map(|k| k.environment);
        let project = self.open_scratch_project(fallback.as_ref()).await?;
        let destination = self
            .open_project(project.key(), id.as_deref(), None)
            .await?;
        self.state.destinations.borrow_mut().insert(
            id.map_or(Target::Home, Target::Draft),
            Destination::Draft(destination),
        );
        self.state.changed.invalidate();
        Ok(())
    }
    async fn open_scratch_project(
        &self,
        fallback: Option<&EnvironmentId>,
    ) -> Result<EnvironmentProject, ClientError> {
        let environment = self
            .scratch_environment(fallback)
            .ok_or(ClientError::Unsupported("scratch project"))?;
        let endpoint = self
            .runtime
            .client(&environment)
            .and_then(|c| c.session())
            .ok_or(ClientError::Disconnected)?;
        if !endpoint.available::<WsProjectsEnsureScratchRpc>(&Empty {}) {
            return Err(ClientError::PermissionDenied {
                method: "projects.ensureScratch".into(),
            });
        }
        let project = ProjectCommands::default()
            .open_scratch(
                environment.clone(),
                &endpoint,
                &self.platform,
                self.runtime.store(),
                || {
                    self.runtime
                        .store()
                        .snapshot()
                        .environments
                        .get(&environment)
                        .and_then(|e| e.config.as_ref())
                        .and_then(|c| c.scratch_workspace_root.as_ref())
                        .map(|r| r.0.clone())
                },
            )
            .await
            .map_err(|e| ClientError::InvalidPayload(e.to_string()))?;
        Ok(project)
    }
}
