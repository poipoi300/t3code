//! CommandPalette add-project flow and useNewProject composition.
use crate::{
    ClientError,
    composition::ClientRuntime,
    connection::Phase,
    navigation::new_thread::Destination,
    operations,
    platform::{Platform, PlatformError},
    rpc::{RpcCallError, RpcEndpoint, SessionEndpoint},
    services::resources::{Resource, ResourcePolicy},
    types::{EnvironmentId, Generation, ProjectId, ThreadKey},
    workspace::{
        ProjectKey,
        project_ops::{ProjectCommands, paths, source_control::SourceControl},
        projects::EnvironmentProject,
    },
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};
use t3_contracts::{rpc::*, server::ServerConfig, source_control::SourceControlDiscoveryResult};

const PROVIDERS: &[(&str, &str)] = &[
    ("github", "GitHub"),
    ("cursor-origin", "Cursor Origin"),
    ("gitlab", "GitLab"),
    ("gitcafe", "GitCafe"),
    ("forgejo", "Forgejo"),
    ("bitbucket", "Bitbucket"),
    ("azure-devops", "Azure DevOps"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Label {
    Key(String),
    Content(String),
}
impl Label {
    fn key(value: &str) -> Self {
        Self::Key(value.into())
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Step {
    #[default]
    Environments,
    Sources,
    LocalFolder,
    NewProject,
    Repository,
    CloneDestination,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Environment,
    Folder,
    NewRepository,
    Repository,
    Settings,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub title: Label,
    pub description: Option<Label>,
    pub glyph: Glyph,
    pub disabled: bool,
    pub setup_required: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub label: Label,
    pub items: Vec<Item>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Summary {
    New {
        name: String,
        path: String,
        root: String,
        environment: Option<String>,
    },
    Repository {
        title: String,
        url: String,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub revision: u64,
    pub step: Step,
    pub query: String,
    pub placeholder: Label,
    pub groups: Vec<Group>,
    pub summary: Option<Summary>,
    pub can_submit: bool,
    pub submit_label: Label,
    pub submit_accessibility: Label,
    pub can_go_back: bool,
    pub folder_picker_label: Option<Label>,
    pub can_pick_folder: bool,
    pub loading_folders: bool,
    pub error: Option<String>,
    pub empty_message: Option<Label>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub title: String,
    pub description: String,
    pub warning: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishTarget {
    pub kind: String,
    pub label: String,
    pub owner: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Publishing {
    pub selected: PublishTarget,
    pub targets: Vec<PublishTarget>,
    pub publishes: bool,
    pub can_change: bool,
    pub repository: String,
}
#[derive(Clone, Default)]
struct Draft {
    step: Step,
    environment: Option<EnvironmentId>,
    query: String,
    source: String,
    remote: Option<String>,
    repository: Option<String>,
    stack: Vec<(Step, String)>,
    publishes: bool,
    publish_kind: Option<String>,
}
struct State {
    draft: RefCell<Draft>,
    revision: Cell<u64>,
    busy: Cell<bool>,
    completed: Cell<bool>,
    changed: Resource<()>,
    errors: RefCell<Option<String>>,
    notices: RefCell<Vec<Notice>>,
    discoveries: RefCell<BTreeMap<EnvironmentId, Option<Rc<SourceControlDiscoveryResult>>>>,
    active_project: RefCell<Option<ProjectKey>>,
    watchers: Cell<usize>,
    browses: RefCell<
        BTreeMap<(EnvironmentId, String), Rc<t3_contracts::filesystem::FilesystemBrowseResult>>,
    >,
    browse_errors: RefCell<BTreeMap<(EnvironmentId, String), String>>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            draft: RefCell::default(),
            revision: Cell::new(0),
            busy: Cell::new(false),
            completed: Cell::new(false),
            changed: Resource::new(ResourcePolicy::default(), Generation::default()),
            errors: RefCell::default(),
            notices: RefCell::default(),
            discoveries: RefCell::default(),
            active_project: RefCell::default(),
            watchers: Cell::new(0),
            browses: RefCell::default(),
            browse_errors: RefCell::default(),
        }
    }
}
#[derive(Clone)]
pub struct AddProjectSession<P: Platform + Clone + 'static> {
    runtime: ClientRuntime<P>,
    platform: P,
    state: Rc<State>,
    projects: ProjectCommands,
    source_control: SourceControl,
    spawn: Rc<dyn Fn(LocalBoxFuture<'static, ()>)>,
}
struct Busy(Rc<State>);
struct Demand(Rc<State>);
impl Drop for Demand {
    fn drop(&mut self) {
        self.0.watchers.set(self.0.watchers.get().saturating_sub(1));
    }
}
impl Drop for Busy {
    fn drop(&mut self) {
        self.0.busy.set(false);
        self.0.changed.invalidate();
    }
}
impl<P: Platform + Clone + 'static> AddProjectSession<P> {
    pub fn new(
        runtime: ClientRuntime<P>,
        platform: P,
        spawn: Rc<dyn Fn(LocalBoxFuture<'static, ()>)>,
    ) -> Self {
        Self {
            runtime,
            platform,
            state: Rc::default(),
            projects: ProjectCommands::default(),
            source_control: SourceControl::default(),
            spawn,
        }
    }
    pub fn set_active_project(&self, project: Option<ProjectKey>) {
        *self.state.active_project.borrow_mut() = project;
    }
    pub fn publishing(&self) -> Option<Publishing> {
        let draft = self.state.draft.borrow();
        if draft.step != Step::NewProject {
            return None;
        }
        let environment = draft.environment.as_ref()?;
        let config = self.config(environment).ok()?;
        let discovery = self
            .state
            .discoveries
            .borrow()
            .get(environment)
            .cloned()
            .flatten()?;
        let settings = serde_json::to_value(&config.settings).ok()?;
        let mut targets = vec![];
        for kind in ["github", "cursor-origin", "gitlab", "gitcafe", "forgejo"] {
            if !settings["offeredSourceControlProviders"]
                .get(kind)
                .and_then(Value::as_bool)
                .unwrap_or(kind == "github")
            {
                continue;
            }
            if !operations::add_project_remote_source_readiness(Some(&discovery), kind).ready {
                continue;
            }
            let provider = discovery
                .source_control_providers
                .iter()
                .find(|p| p.kind.0 == kind)?;
            let owner = match &provider.auth.account {
                t3_contracts::source_control::WireOption::Some { value } => Some(value.0.clone()),
                t3_contracts::source_control::WireOption::None => None,
            };
            if matches!(kind, "gitcafe" | "forgejo") && owner.is_none() {
                continue;
            }
            targets.push(PublishTarget {
                kind: kind.into(),
                label: provider.label.0.clone(),
                owner,
            });
        }
        let selected = targets
            .iter()
            .find(|t| Some(t.kind.as_str()) == draft.publish_kind.as_deref())
            .or_else(|| targets.first())?
            .clone();
        let folder = new_project_folder_name(draft.query.trim());
        let repository = if draft.query.trim().is_empty() {
            selected
                .owner
                .clone()
                .unwrap_or_else(|| format!("Your {} account", selected.label))
        } else {
            selected
                .owner
                .as_ref()
                .map_or_else(|| folder.clone(), |owner| format!("{owner}/{folder}"))
        };
        Some(Publishing {
            selected,
            targets,
            publishes: draft.publishes,
            can_change: !self.state.busy.get(),
            repository,
        })
    }
    pub fn set_publishing(&self, publishes: bool, kind: Option<String>) -> Result<(), ClientError> {
        let view = self
            .publishing()
            .filter(|v| v.can_change)
            .ok_or(ClientError::Cancelled)?;
        if kind
            .as_ref()
            .is_some_and(|kind| !view.targets.iter().any(|target| &target.kind == kind))
        {
            return Err(ClientError::Cancelled);
        }
        let mut draft = self.state.draft.borrow_mut();
        draft.publishes = publishes;
        draft.publish_kind = kind.or(Some(view.selected.kind));
        drop(draft);
        self.changed();
        Ok(())
    }
    pub fn notices(&self) -> Vec<Notice> {
        self.state.notices.borrow().clone()
    }
    pub fn completed(&self) -> LocalBoxStream<'static, ()> {
        let state = self.state.clone();
        self.state
            .changed
            .observe(self.platform.monotonic_ms())
            .filter_map(move |_| futures::future::ready(state.completed.get().then_some(())))
            .take(1)
            .boxed_local()
    }
    fn changed(&self) {
        self.state
            .revision
            .set(self.state.revision.get().wrapping_add(1));
        self.state.changed.invalidate();
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        futures::stream::select(
            self.runtime.store().subscribe().map(|_| ()),
            self.state
                .changed
                .observe(self.platform.monotonic_ms())
                .map(|_| ()),
        )
        .boxed_local()
    }
    fn endpoint(&self, environment: &EnvironmentId) -> Result<SessionEndpoint, ClientError> {
        if !self
            .runtime
            .environments()
            .iter()
            .any(|e| &e.id == environment && e.state.phase == Phase::Connected)
        {
            return Err(ClientError::Disconnected);
        }
        self.runtime
            .client(environment)
            .and_then(|c| c.session())
            .ok_or(ClientError::Disconnected)
    }
    fn config(&self, environment: &EnvironmentId) -> Result<Rc<ServerConfig>, ClientError> {
        self.runtime
            .store()
            .snapshot()
            .environments
            .get(environment)
            .and_then(|e| e.config.clone())
            .ok_or(ClientError::Disconnected)
    }
    fn all_projects(&self) -> Vec<EnvironmentProject> {
        self.runtime
            .store()
            .snapshot()
            .environments
            .iter()
            .flat_map(|(env, state)| {
                state.shell.snapshot.iter().flat_map(move |shell| {
                    shell
                        .projects
                        .iter()
                        .map(move |project| EnvironmentProject {
                            environment: env.clone(),
                            project: project.clone(),
                            is_scratch: false,
                        })
                })
            })
            .collect()
    }
    fn current_cwd(&self, environment: &EnvironmentId) -> Option<String> {
        let key = self.state.active_project.borrow().clone()?;
        if &key.environment != environment {
            return None;
        }
        self.all_projects()
            .iter()
            .find(|project| project.key() == key)
            .map(|p| p.project.workspace_root.0.clone())
    }
    fn path(&self, environment: &EnvironmentId, raw: &str) -> Result<String, ClientError> {
        let config = self.config(environment)?;
        let windows = config.environment.platform.os
            == t3_contracts::environment::ExecutionEnvironmentPlatformOs::Windows;
        operations::resolve_add_project_path(
            raw,
            self.current_cwd(environment).as_deref(),
            if windows { "win32" } else { "linux" },
        )
        .map_err(|message| ClientError::InvalidPayload(message.into()))
    }
    fn push(&self, step: Step, query: String) {
        let mut draft = self.state.draft.borrow_mut();
        let prior = (draft.step, draft.query.clone());
        draft.stack.push(prior);
        draft.step = step;
        draft.query = query;
        self.state.errors.borrow_mut().take();
        drop(draft);
        self.changed();
    }
    pub fn set_query(&self, query: String) -> Result<(), ClientError> {
        if self.state.busy.get() {
            return Err(ClientError::Cancelled);
        }
        self.state.draft.borrow_mut().query = query;
        self.state.errors.borrow_mut().take();
        self.changed();
        Ok(())
    }
    pub fn back(&self) -> Result<(), ClientError> {
        if self.state.busy.get() {
            return Err(ClientError::Cancelled);
        }
        let mut draft = self.state.draft.borrow_mut();
        if let Some((step, query)) = draft.stack.pop() {
            draft.step = step;
            draft.query = query;
        }
        drop(draft);
        self.state.errors.borrow_mut().take();
        self.changed();
        Ok(())
    }
    fn item(
        id: String,
        title: Label,
        description: Option<Label>,
        glyph: Glyph,
        disabled: bool,
    ) -> Item {
        Item {
            id,
            title,
            description,
            glyph,
            disabled,
            setup_required: false,
        }
    }
    pub fn watch(&self) -> LocalBoxStream<'static, Rc<View>> {
        if self.state.watchers.get() == 0 {
            *self.state.draft.borrow_mut() = Draft::default();
            self.state.errors.borrow_mut().take();
            self.state.completed.set(false);
            self.changed();
        }
        self.state.watchers.set(self.state.watchers.get() + 1);
        let demand = Demand(self.state.clone());
        let this = self.clone();
        let changes = self.changes();
        futures::stream::unfold(
            (this, changes, 0u8, demand),
            |(this, mut changes, phase, demand)| async move {
                if phase == 0 {
                    let view = this.resolve_io(false).await;
                    return Some((Rc::new(view), (this, changes, 1, demand)));
                }
                if phase == 2 {
                    changes.next().await?;
                }
                loop {
                    let outcome = {
                        let job = this.resolve_io(true);
                        futures::pin_mut!(job);
                        match futures::future::select(job, changes.next()).await {
                            futures::future::Either::Left((view, _)) => Some(Some(view)),
                            futures::future::Either::Right((Some(_), _)) => Some(None),
                            futures::future::Either::Right((None, _)) => None,
                        }
                    };
                    match outcome {
                        Some(Some(view)) => {
                            return Some((Rc::new(view), (this, changes, 2, demand)));
                        }
                        Some(None) => {}
                        None => return None,
                    }
                }
            },
        )
        .boxed_local()
    }
    pub async fn resolve(&self) -> View {
        self.resolve_io(true).await
    }
    async fn resolve_io(&self, io: bool) -> View {
        if self.state.draft.borrow().step == Step::Environments
            && self.state.draft.borrow().stack.is_empty()
        {
            let environments = self.runtime.environments();
            if environments.len() == 1 && environments[0].state.phase == Phase::Connected {
                let mut draft = self.state.draft.borrow_mut();
                draft.step = Step::Sources;
                draft.environment = Some(environments[0].id.clone());
            }
        }
        let draft = self.state.draft.borrow().clone();
        let mut view = View {
            revision: self.state.revision.get(),
            step: draft.step,
            query: draft.query.clone(),
            placeholder: Label::key("Search..."),
            groups: vec![],
            summary: None,
            can_submit: false,
            submit_label: Label::key("Add project"),
            submit_accessibility: Label::key("Add project"),
            can_go_back: !draft.stack.is_empty(),
            folder_picker_label: None,
            can_pick_folder: false,
            loading_folders: false,
            error: self.state.errors.borrow().clone(),
            empty_message: None,
        };
        if draft.step == Step::Environments {
            let items = self
                .runtime
                .environments()
                .iter()
                .map(|environment| {
                    Self::item(
                        format!("environment:{}", environment.id.0),
                        Label::Content(environment.label.clone()),
                        Some(Label::Content(environment.id.0.clone())),
                        Glyph::Environment,
                        environment.state.phase != Phase::Connected,
                    )
                })
                .collect::<Vec<_>>();
            view.groups.push(Group {
                label: Label::key("Environments"),
                items,
            });
            if self.runtime.environments().is_empty() {
                view.groups[0].items.push(Self::item(
                    "connect".into(),
                    Label::key("Connect an environment"),
                    Some(Label::key("No connected environment can add a project")),
                    Glyph::Settings,
                    false,
                ));
            }
            return view;
        }
        let Some(environment) = draft.environment.as_ref() else {
            return view;
        };
        let config = match self.config(environment) {
            Ok(config) => config,
            Err(error) => {
                view.error = Some(error.to_string());
                return view;
            }
        };
        if draft.step == Step::Sources {
            view.placeholder = Label::key("Choose a source...");
            let connected = self.endpoint(environment).is_ok();
            let mut items = vec![];
            if config.new_projects_root.is_some() {
                items.push(Self::item(
                    "new".into(),
                    Label::key("New project"),
                    Some(Label::key("Start a new Git repository from a name")),
                    Glyph::NewRepository,
                    !connected,
                ));
            }
            items.push(Self::item(
                "local".into(),
                Label::key("Local folder"),
                Some(Label::key("Browse a folder on disk")),
                Glyph::Folder,
                !connected,
            ));
            items.push(Self::item(
                "source:url".into(),
                Label::key("Git URL"),
                Some(Label::key("Clone from a remote URL")),
                Glyph::Repository,
                !connected,
            ));
            if io
                && !self.state.discoveries.borrow().contains_key(environment)
                && let Ok(endpoint) = self.endpoint(environment)
            {
                let discovery = self
                    .source_control
                    .discovery(environment, &endpoint, Empty {})
                    .await
                    .ok()
                    .map(Rc::new);
                self.state
                    .discoveries
                    .borrow_mut()
                    .insert(environment.clone(), discovery);
            }
            let discovery = self
                .state
                .discoveries
                .borrow()
                .get(environment)
                .cloned()
                .flatten();
            let mut providers = PROVIDERS.to_vec();
            providers.sort_by(|a, b| {
                let ready = |kind: &str| {
                    operations::add_project_remote_source_readiness(discovery.as_deref(), kind)
                        .ready
                };
                ready(b.0)
                    .cmp(&ready(a.0))
                    .then_with(|| a.1.cmp(b.1))
                    .then_with(|| a.0.cmp(b.0))
            });
            let settings = serde_json::to_value(&config.settings).unwrap_or_default();
            for (kind, label) in providers {
                let offered = settings["offeredSourceControlProviders"]
                    .get(kind)
                    .and_then(Value::as_bool)
                    .unwrap_or(kind == "github");
                if !offered {
                    continue;
                }
                let ready =
                    operations::add_project_remote_source_readiness(discovery.as_deref(), kind);
                let mut item = Self::item(
                    format!("source:{kind}"),
                    Label::Key(format!("{label} repository")),
                    ready.hint.map(Label::Content),
                    Glyph::Repository,
                    !ready.ready || !connected,
                );
                item.setup_required = !ready.ready;
                items.push(item);
            }
            view.groups.push(Group {
                label: Label::key("Sources"),
                items,
            });
            return view;
        }
        let busy = self.state.busy.get();
        view.can_submit =
            !draft.query.trim().is_empty() && !busy && self.endpoint(environment).is_ok();
        match draft.step {
            Step::NewProject => {
                view.placeholder = Label::key("Project name...");
                view.submit_label = Label::key("Create project");
                view.submit_accessibility = view.submit_label.clone();
                let root = config
                    .new_projects_root
                    .as_ref()
                    .map(|p| p.0.clone())
                    .unwrap_or_default();
                view.can_submit &= !root.is_empty();
                view.summary = Some(Summary::New {
                    name: draft.query.trim().into(),
                    path: operations::get_clone_destination_path(
                        &root,
                        Some(&new_project_folder_name(draft.query.trim())),
                    ),
                    root,
                    environment: self
                        .runtime
                        .environments()
                        .iter()
                        .find(|e| e.id == *environment)
                        .map(|e| e.label.clone()),
                });
            }
            Step::Repository => {
                view.placeholder = Label::key("Repository...");
                view.submit_label = Label::key("Continue");
                view.submit_accessibility = view.submit_label.clone();
            }
            Step::LocalFolder | Step::CloneDestination => {
                view.placeholder = Label::key("Folder path...");
                if draft.step == Step::CloneDestination {
                    view.submit_label = Label::key("Clone repository");
                    view.submit_accessibility = view.submit_label.clone();
                    view.summary = Some(Summary::Repository {
                        title: draft.repository.clone().unwrap_or_default(),
                        url: draft.remote.clone().unwrap_or_default(),
                    });
                }
                if !draft.query.trim().is_empty()
                    && let Ok(endpoint) = self.endpoint(environment)
                {
                    let key = (environment.clone(), draft.query.clone());
                    let input = serde_json::from_value(
                        json!({"partialPath":draft.query,"cwd":self.current_cwd(environment)}),
                    );
                    if io
                        && !self.state.browses.borrow().contains_key(&key)
                        && let Ok(input) = input
                    {
                        match endpoint.call::<WsFilesystemBrowseRpc>(input).await {
                            Ok(result) => {
                                self.state.browse_errors.borrow_mut().remove(&key);
                                self.state
                                    .browses
                                    .borrow_mut()
                                    .insert(key.clone(), Rc::new(result));
                            }
                            Err(error) => {
                                self.state
                                    .browse_errors
                                    .borrow_mut()
                                    .insert(key.clone(), error.to_string());
                            }
                        }
                    }
                    if let Some(result) = self.state.browses.borrow().get(&key).cloned() {
                        view.groups.push(Group {
                            label: Label::key("Folders"),
                            items: result
                                .entries
                                .iter()
                                .map(|entry| {
                                    Self::item(
                                        format!("folder:{}", entry.full_path.0),
                                        Label::Content(entry.name.0.clone()),
                                        Some(Label::Content(entry.full_path.0.clone())),
                                        Glyph::Folder,
                                        false,
                                    )
                                })
                                .collect(),
                        });
                    } else if let Some(error) = self.state.browse_errors.borrow().get(&key) {
                        view.error = Some(error.clone());
                    } else {
                        view.loading_folders = true;
                    }
                }
            }
            _ => {}
        }
        view
    }
    pub async fn select_item(&self, id: &str) -> Result<Option<Destination>, ClientError> {
        let view = self.resolve().await;
        if self.state.busy.get()
            || !view
                .groups
                .iter()
                .flat_map(|g| &g.items)
                .any(|item| item.id == id && !item.disabled)
        {
            return Err(ClientError::Cancelled);
        }
        if id == "connect" {
            let route=crate::navigation::Route::Settings(crate::navigation::SettingsPage::Connections);
            self.runtime.navigation().navigate(crate::navigation::AppUrlRequest::Push(route.address().ok_or(ClientError::Cancelled)?)).await?;
            return Ok(None);
        }
        if let Some(environment) = id.strip_prefix("environment:") {
            self.state.draft.borrow_mut().environment = Some(EnvironmentId(environment.into()));
            self.push(Step::Sources, String::new());
        } else if id == "new" {
            self.push(Step::NewProject, String::new());
        } else if id == "local" {
            let environment = self
                .state
                .draft
                .borrow()
                .environment
                .clone()
                .ok_or(ClientError::Cancelled)?;
            let config = self.config(&environment)?;
            let base = config.settings.add_project_base_directory.0.trim();
            self.push(
                Step::LocalFolder,
                if base.is_empty() {
                    "~/".into()
                } else {
                    paths::ensure_browse_directory_path(base)
                },
            );
        } else if let Some(source) = id.strip_prefix("source:") {
            self.state.draft.borrow_mut().source = source.into();
            self.push(Step::Repository, String::new());
        } else if let Some(path) = id.strip_prefix("folder:") {
            self.set_query(paths::ensure_browse_directory_path(path))?;
        }
        Ok(None)
    }
    fn begin(&self, revision: u64) -> Result<Busy, ClientError> {
        if revision != self.state.revision.get() || self.state.busy.replace(true) {
            return Err(ClientError::Cancelled);
        }
        self.state.changed.invalidate();
        Ok(Busy(self.state.clone()))
    }
    pub async fn submit(&self, revision: u64) -> Result<Option<Destination>, ClientError> {
        let _busy = self.begin(revision)?;
        let result = self.submit_inner().await;
        if let Err(error) = &result {
            *self.state.errors.borrow_mut() = Some(error.to_string());
        }
        self.state.changed.invalidate();
        result
    }
    async fn submit_inner(&self) -> Result<Option<Destination>, ClientError> {
        let draft = self.state.draft.borrow().clone();
        let environment = draft.environment.clone().ok_or(ClientError::Cancelled)?;
        let endpoint = self.endpoint(&environment)?;
        match draft.step {
            Step::Repository => {
                if let Some(message) = operations::github_incomplete_owner_repository_message(
                    &draft.source,
                    draft.query.trim(),
                ) {
                    return Err(ClientError::InvalidPayload(message.into()));
                }
                let config = self.config(&environment)?;
                let (remote, name) = if draft.source == "url" {
                    (
                        operations::normalize_pasted_clone_url(draft.query.trim()),
                        None,
                    )
                } else {
                    let input = serde_json::from_value(
                        json!({"provider":draft.source,"repository":draft.query.trim()}),
                    )?;
                    let repository = self
                        .source_control
                        .repository(&environment, &endpoint, input)
                        .await
                        .map_err(RpcCallError::into_client_error)?;
                    let https = matches!(
                        repository.provider.0.as_str(),
                        "github" | "cursor-origin" | "forgejo"
                    );
                    (
                        operations::get_default_clone_url(&repository, https).into(),
                        Some(repository.name_with_owner.0),
                    )
                };
                let ghq = config.settings.ghq_clone_layout;
                let base = paths::ensure_browse_directory_path(
                    config.settings.add_project_base_directory.0.trim(),
                );
                let base = if base.is_empty() { "~/".into() } else { base };
                let destination = operations::get_clone_destination_path(
                    &base,
                    Some(&operations::get_clone_directory_path(
                        &remote,
                        ghq,
                        name.as_deref(),
                    )),
                );
                {
                    let mut state = self.state.draft.borrow_mut();
                    state.remote = Some(remote);
                    state.repository = name.or(Some(draft.query));
                }
                self.push(Step::CloneDestination, destination);
                Ok(None)
            }
            Step::LocalFolder => {
                let path = self.path(&environment, &draft.query)?;
                if let Some(existing) =
                    operations::find_existing_add_project(&self.all_projects(), &environment, &path)
                        .cloned()
                {
                    if existing
                        .project
                        .archived_at
                        .as_ref()
                        .is_some_and(Option::is_some)
                    {
                        let input = serde_json::from_value(
                            json!({"type":"project.update","commandId":operations::allocate_command_id(&self.platform,None)?,"projectId":existing.project.id,"archived":false}),
                        )?;
                        if let Err(error) = self
                            .projects
                            .update(environment.clone(), &endpoint, input)
                            .await
                        {
                            self.state.notices.borrow_mut().push(Notice {
                                title: "Failed to unarchive project".into(),
                                description: error.to_string(),
                                warning: true,
                            });
                        }
                    }
                    return self.open_existing(existing.key()).await.map(Some);
                }
                let id = ProjectId(operations::allocate_command_id(&self.platform, None)?.0);
                let changes = self.runtime.store().subscribe();
                let generation = self.generation(&environment);
                let command = operations::build_project_create_command(
                    operations::allocate_command_id(&self.platform, None)?,
                    id.clone(),
                    path,
                );
                self.projects
                    .mutate(environment.clone(), &endpoint, command)
                    .await
                    .map_err(RpcCallError::into_client_error)?;
                let key = ProjectKey {
                    environment,
                    project: id,
                };
                self.wait_project(&key, generation, changes, 10_000).await?;
                self.open_draft(key).await.map(Some)
            }
            Step::NewProject => {
                let publish = self
                    .publishing()
                    .filter(|p| p.publishes)
                    .map(|p| p.selected);
                let generation = self.generation(&environment);
                let changes = self.runtime.store().subscribe();
                let result = self
                    .projects
                    .create_new(
                        environment.clone(),
                        &endpoint,
                        serde_json::from_value(json!({"name":draft.query.trim()}))?,
                    )
                    .await
                    .map_err(RpcCallError::into_client_error)?;
                let value = serde_json::to_value(&result)?;
                let warning = value["commitError"].as_str();
                self.state.notices.borrow_mut().push(Notice {
                    title: if warning.is_some() {
                        format!("Created {} without a first commit", draft.query.trim())
                    } else {
                        format!("Created {}", draft.query.trim())
                    },
                    description: warning.map_or_else(
                        || result.workspace_root.0.clone(),
                        |error| format!("{error} The project is in {}.", result.workspace_root.0),
                    ),
                    warning: warning.is_some(),
                });
                if let Some(target) = publish {
                    let this = self.clone();
                    let endpoint = endpoint.clone();
                    let env = environment.clone();
                    let cwd = result.workspace_root.0.clone();
                    (self.spawn)(async move {
                        let folder=operations::infer_project_title_from_path(&cwd);
                        let repository=target.owner.as_ref().map_or(folder.clone(),|owner|format!("{owner}/{folder}"));
                        let result=async {
                            let input=serde_json::from_value(json!({"cwd":cwd,"provider":target.kind,"repository":repository,"visibility":"private"}))?;
                            this.source_control.publish_repository(env,&endpoint,&this.platform,input).await.map_err(RpcCallError::into_client_error)
                        }.await;
                        let notice=match result {Ok(reply)=>Notice {title:format!("Published to {}",target.label),description:reply.repository.name_with_owner.0,warning:false},
                            Err(error)=>Notice{title:format!("Could not create the {} repository",target.label),description:format!("{error} Use Publish Repository in the Git menu to try again."),warning:true}};
                        this.state.notices.borrow_mut().push(notice);this.state.changed.invalidate();
                    }.boxed_local());
                }
                let key = ProjectKey {
                    environment,
                    project: result.project_id,
                };
                let opened = async {
                    self.wait_project(&key, generation, changes, 10_000).await?;
                    self.open_draft(key).await
                }
                .await;
                match opened {
                    Ok(destination) => Ok(Some(destination)),
                    Err(error) => {
                        self.state.notices.borrow_mut().push(Notice {
                            title: "Failed to open project".into(),
                            description: error.to_string(),
                            warning: true,
                        });
                        self.state.completed.set(true);
                        Ok(None)
                    }
                }
            }
            Step::CloneDestination => {
                let path = self.path(&environment, &draft.query)?;
                let id = ProjectId(operations::allocate_command_id(&self.platform, None)?.0);
                let input = serde_json::from_value(
                    json!({"projectId":id,"title":operations::infer_project_title_from_path(&path),"createdAt":crate::time::iso_from_epoch_ms(self.platform.now_epoch_ms()),"remoteUrl":draft.remote.ok_or(ClientError::Cancelled)?,"destinationPath":path}),
                )?;
                let changes = self.runtime.store().subscribe();
                let generation = self.generation(&environment);
                let result = self
                    .source_control
                    .start_project_clone(environment.clone(), &endpoint, input)
                    .await
                    .map_err(RpcCallError::into_client_error)?;
                self.state.completed.set(true);
                let this = self.clone();
                (self.spawn)(
                    async move {
                        let key = ProjectKey {
                            environment,
                            project: result.project_id,
                        };
                        let _ = this.wait_project(&key, generation, changes, 3_000).await;
                        if this.generation(&key.environment) != generation {
                            return;
                        }
                        let opened = async {
                            let Destination::Draft(id) = this.open_draft(key).await? else {
                                return Err(ClientError::Cancelled);
                            };
                            let path = crate::navigation::Route::Draft(id)
                                .address()
                                .ok_or(ClientError::Cancelled)?;
                            this.runtime
                                .navigation()
                                .navigate(crate::navigation::AppUrlRequest::Push(path))
                                .await
                        }
                        .await;
                        if let Err(error) = opened {
                            this.state.notices.borrow_mut().push(Notice {
                                title: "Failed to open project".into(),
                                description: error.to_string(),
                                warning: true,
                            });
                            this.state.changed.invalidate();
                        }
                    }
                    .boxed_local(),
                );
                Ok(None)
            }
            _ => Err(ClientError::Cancelled),
        }
    }
    async fn open_draft(&self, key: ProjectKey) -> Result<Destination, ClientError> {
        self.runtime
            .new_threads()
            .open_project(key, None, None)
            .await
            .map(Destination::Draft)
    }
    async fn open_existing(&self, key: ProjectKey) -> Result<Destination, ClientError> {
        let shell = self.runtime.store().shared_shell(&key.environment);
        let order = if self
            .runtime
            .settings()
            .client_settings()
            .sidebar_thread_sort_order
            == t3_contracts::settings::SidebarThreadSortOrder::CreatedAt
        {
            crate::thread_views::threads::sort::SortOrder::CreatedAt
        } else {
            crate::thread_views::threads::sort::SortOrder::LatestUserMessage
        };
        let timestamp = |thread: &t3_contracts::orchestration_v2::OrchestrationV2ThreadShell| {
            crate::thread_views::threads::sort::get_thread_sort_timestamp(
                &crate::thread_views::threads::sort::ThreadSortInput {
                    created_at: &thread.created_at.0,
                    updated_at: &thread.updated_at.0,
                    latest_user_message_at: thread
                        .latest_user_message_at
                        .as_ref()
                        .map(|d| d.0.as_str()),
                    latest_user_authored_message_at: thread
                        .latest_user_authored_message_at
                        .as_ref()
                        .map(|d| d.as_ref().map(|d| d.0.as_str())),
                    messages: &[],
                },
                order,
                &crate::time::epoch_ms,
            )
        };
        let latest = shell.as_ref().and_then(|s| {
            s.threads
                .iter()
                .filter(|t| t.project_id == key.project && t.archived_at.is_none())
                .max_by(|a, b| {
                    timestamp(a)
                        .cmp(&timestamp(b))
                        .then_with(|| a.id.cmp(&b.id))
                })
        });
        if let Some(thread) = latest {
            let value = serde_json::to_value(thread)?;
            if value["settledOverride"] != "settled" {
                return Ok(Destination::Thread(ThreadKey {
                    environment: key.environment,
                    thread: thread.id.clone(),
                }));
            }
        }
        self.open_draft(key).await
    }
    fn generation(&self, environment: &EnvironmentId) -> Option<Generation> {
        self.runtime
            .store()
            .snapshot()
            .environments
            .get(environment)
            .map(|e| e.session)
    }
    async fn wait_project(
        &self,
        key: &ProjectKey,
        generation: Option<Generation>,
        mut changes: crate::store::Subscription,
        timeout_ms: u64,
    ) -> Result<(), ClientError> {
        let timeout = self.platform.sleep(timeout_ms);
        futures::pin_mut!(timeout);
        loop {
            if self.generation(&key.environment) != generation {
                return Err(ClientError::Cancelled);
            }
            if self.all_projects().iter().any(|p| p.key() == *key) {
                return Ok(());
            }
            match futures::future::select(changes.next(), timeout.as_mut()).await {
                futures::future::Either::Left((Some(_), _)) => {}
                futures::future::Either::Left((None, _)) => return Err(ClientError::Disconnected),
                futures::future::Either::Right(_) => {
                    return Err(ClientError::Rejected {
                        message: "Failed to open project".into(),
                        detail: Some(
                            "It will appear in the sidebar once this client catches up.".into(),
                        ),
                    });
                }
            }
        }
    }
    pub async fn pick_folder(&self, revision: u64) -> Result<Option<Destination>, ClientError> {
        if revision != self.state.revision.get() {
            return Err(ClientError::Cancelled);
        }
        Err(PlatformError::Unsupported("directory picker".into()).into())
    }
}

pub fn new_project_folder_name(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let mut slug = String::new();
    let mut separator = false;
    for character in name
        .nfkd()
        .filter(|c| !('\u{300}'..='\u{36f}').contains(c))
        .flat_map(char::to_lowercase)
    {
        if character.is_ascii_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(character);
            separator = false;
        } else {
            separator = true;
        }
        if slug.len() >= 64 {
            slug.truncate(64);
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        return "project".into();
    }
    if matches!(slug, "con" | "prn" | "aux" | "nul")
        || ["com", "lpt"].iter().any(|prefix| {
            slug.strip_prefix(prefix).is_some_and(|suffix| {
                suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9')
            })
        })
    {
        format!("{slug}-project")
    } else {
        slug.into()
    }
}
