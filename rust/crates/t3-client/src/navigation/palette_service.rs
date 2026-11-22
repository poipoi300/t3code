//! CommandPalette.tsx's shared catalogue, target resolution and action dispatch.
use crate::view_logic::shell::palette_files::{PaletteSearchClient, SearchTarget};
use crate::{
    ClientError,
    composition::ClientRuntime,
    navigation::{
        Route, SettingsPage,
        prompt_links::{
            self, HOSTED_APP_URL, MAX_PROMPT_LENGTH, Repository, build_new_thread_link,
        },
        pull_request_links::{
            PullRequestInputResolution, ResolvedPullRequestLink, build_link_command,
            pull_request_link_project, resolve_link_pull_request_input,
        },
    },
    platform::{MimeData, Platform},
    rpc::{RpcCallError, RpcEndpoint},
    services::resources::{Resource, ResourcePolicy},
    state::pull_requests::{legacy_key, normalize_key, parsed_key},
    types::{EnvironmentId, Generation, ProjectId, ThreadId, ThreadKey},
    view_logic::{
        JsTrim,
        shell::{
            palette::{self, FilterInput, Group, Item},
            search, sidebar,
        },
    },
    workspace::ProjectKey,
};
use futures::{StreamExt, stream::LocalBoxStream};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use t3_contracts::{
    model_selection::ModelSelection,
    orchestration_v2::{OrchestrationV2GetThreadProjectionInput, OrchestrationV2ThreadShell},
    pull_request::PullRequestState,
    rpc::{
        WsOrchestrationV2DispatchCommandRpc, WsOrchestrationV2GetThreadProjectionRpc,
        WsServerRefreshProvidersPayload, WsServerRefreshProvidersRpc,
    },
    thread_pull_request::{ThreadPullRequestLink, ThreadPullRequestLinkSource},
};
#[path = "palette_queries.rs"]
mod queries;
type ContentCache = Option<(
    SearchTarget,
    Rc<t3_contracts::project::ProjectSearchContentsResult>,
)>;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HostCommand {
    FindThread,
    FindBrowser,
    CopyPrompt,
    CopyPromptContext,
    CopyReference,
    LinkPullRequest,
    ShowPullRequests,
    ShareGuest,
    RestartSession,
    Triage,
    ChangeTheme,
    ChangeAppearance,
    ThemeEditor,
    UseInAnotherApp,
    FocusNext,
    FocusPrevious,
    PinProject,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Navigate(Route),
    NavigateAddress(String),
    Files,
    Content,
    NewProject,
    Host {
        command: HostCommand,
        thread: Option<ThreadKey>,
        project: Option<ProjectKey>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteToast {
    pub title: &'static str,
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteHostFailure {
    pub title: &'static str,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadReferenceKind {
    PullRequest,
    Thread,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadReferenceCopyTarget {
    pub kind: ThreadReferenceKind,
    pub value: String,
    pub description: String,
    /// The composer-context fragment that pastes a thread back as a chip (`None` for a PR link).
    pub fragment: Option<String>,
    pub success_title: &'static str,
    pub failure_title: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinProjectTarget {
    pub display_name: String,
    pub pinned: bool,
    pub members: Vec<ProjectKey>,
}

#[derive(Clone, Debug)]
pub struct Catalogue {
    pub groups: Rc<Vec<Group>>,
    pub projects: Rc<Vec<Item>>,
    pub threads: Rc<Vec<Item>>,
    pub settings: Rc<Vec<Item>>,
}
#[derive(Clone)]
pub struct PaletteService<P: Platform + Clone + 'static> {
    runtime: ClientRuntime<P>,
    platform: P,
    route: Rc<RefCell<Route>>,
    host: Rc<RefCell<BTreeSet<HostCommand>>>,
    changed: Resource<()>,
    settings_platform: Rc<RefCell<crate::services::settings::search::SettingsSearchPlatform>>,
    hosted_app_url: Rc<RefCell<Option<String>>>,
    message_targets: Rc<RefCell<BTreeSet<ThreadKey>>>,
    content_cache: Rc<RefCell<ContentCache>>,
}
impl<P: Platform + Clone + 'static> PaletteService<P> {
    pub fn new(runtime: ClientRuntime<P>, platform: P) -> Self {
        Self {
            runtime,
            platform,
            route: Rc::new(RefCell::new(Route::Home)),
            host: Rc::default(),
            changed: Resource::new(ResourcePolicy::default(), Generation::default()),
            settings_platform: Rc::default(),
            hosted_app_url: Rc::default(),
            message_targets: Rc::default(),
            content_cache: Rc::default(),
        }
    }
    pub fn set_hosted_app_url(&self, origin: Option<String>) {
        if *self.hosted_app_url.borrow() != origin {
            *self.hosted_app_url.borrow_mut() = origin;
            self.changed.invalidate();
        }
    }
    fn is_draft(&self) -> bool {
        matches!(&*self.route.borrow(), Route::Draft(_))
    }
    pub fn set_route(&self, route: Route) {
        if *self.route.borrow() != route {
            *self.route.borrow_mut() = route;
            self.changed.invalidate();
        }
    }
    /// Actual renderer/controller support; feature policy and item titles stay in this service.
    pub fn set_host_commands(&self, commands: BTreeSet<HostCommand>) {
        *self.host.borrow_mut() = commands;
        self.changed.invalidate();
    }
    pub fn set_settings_platform(
        &self,
        platform: crate::services::settings::search::SettingsSearchPlatform,
    ) {
        *self.settings_platform.borrow_mut() = platform;
        self.changed.invalidate();
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        futures::stream::select_all(vec![
            self.runtime.store().subscribe().map(|_| ()).boxed_local(),
            self.runtime.changes(),
            self.runtime.composer_drafts().changes(),
            self.changed
                .observe(self.platform.monotonic_ms())
                .map(|_| ())
                .boxed_local(),
        ])
        .boxed_local()
    }
    fn key(&self) -> Option<ThreadKey> {
        match &*self.route.borrow() {
            Route::Thread(key) | Route::ThreadPanel(key, _) => Some(key.clone()),
            Route::Draft(id) => {
                let document = self.runtime.composer_drafts().document();
                let doc = document.borrow();
                let session = &doc["draftThreadsByThreadKey"][id];
                Some(ThreadKey {
                    environment: EnvironmentId(session["environmentId"].as_str()?.into()),
                    thread: ThreadId(session["threadId"].as_str()?.into()),
                })
            }
            _ => None,
        }
    }
    pub fn search_target(&self) -> Option<SearchTarget> {
        let key = self.key()?;
        if let Some(workspace) = self.runtime.project_files().workspace(&key) {
            return Some(SearchTarget {
                thread: key,
                cwd: workspace.cwd,
                project_name: workspace.project_name,
            });
        }
        let route = self.route.borrow();
        let Route::Draft(id) = &*route else {
            return None;
        };
        let document = self.runtime.composer_drafts().document();
        let doc = document.borrow();
        let session = &doc["draftThreadsByThreadKey"][id];
        let project = ProjectKey {
            environment: key.environment.clone(),
            project: ProjectId(session["projectId"].as_str()?.into()),
        };
        let projects = self.runtime.new_threads().projects();
        let project = projects.iter().find(|p| p.key() == project)?;
        let cwd = session["worktreePath"]
            .as_str()
            .filter(|p| !p.is_empty())
            .unwrap_or(&project.project.workspace_root.0)
            .to_owned();
        Some(SearchTarget {
            thread: key,
            cwd,
            project_name: project.project.title.0.clone(),
        })
    }
    fn active_project(&self) -> Option<ProjectKey> {
        let key = self.key()?;
        if let Some(shell) = self.runtime.store().shared_shell(&key.environment)
            && let Some(thread) = shell.threads.iter().find(|t| t.id == key.thread)
        {
            return Some(ProjectKey {
                environment: key.environment,
                project: thread.project_id.clone(),
            });
        }
        let route = self.route.borrow();
        let Route::Draft(id) = &*route else {
            return None;
        };
        let document = self.runtime.composer_drafts().document();
        let doc = document.borrow();
        Some(ProjectKey {
            environment: key.environment,
            project: ProjectId(
                doc["draftThreadsByThreadKey"][id]["projectId"]
                    .as_str()?
                    .into(),
            ),
        })
    }
    fn host_available(&self, command: &HostCommand) -> bool {
        if !self.host.borrow().contains(command) {
            return false;
        }
        let key = self.key();
        if thread_command(command) && key.is_none() {
            return false;
        }
        let server = key.as_ref().and_then(|key| self.server_thread(key));
        let config = key
            .as_ref()
            .and_then(|key| self.environment_config(&key.environment));
        match command {
            HostCommand::FindThread => {
                !self.is_draft()
                    && server.is_some()
                    && config.as_ref().is_some_and(|c| c.thread_find == Some(true))
            }
            HostCommand::RestartSession => !self.is_draft() && server.is_some(),
            HostCommand::CopyReference => self.active_thread_reference_copy_target().is_some(),
            HostCommand::LinkPullRequest | HostCommand::ShowPullRequests => {
                !self.is_draft() && server.is_some() && config.is_some()
            }
            HostCommand::ShareGuest => key.as_ref().is_some_and(|key| {
                !self.is_draft()
                    && server.is_some()
                    && crate::thread_views::thread_ops::guest_share::can_share_with_guest(
                        &self.runtime,
                        key,
                    )
            }),
            HostCommand::CopyPrompt | HostCommand::CopyPromptContext => {
                if !self.is_draft() && server.is_none() {
                    return false;
                }
                let Some(prompt) = self.active_draft_prompt() else {
                    return false;
                };
                if !matches!(command, HostCommand::CopyPromptContext) {
                    return !prompt.is_empty();
                }
                self.active_prompt_repository().is_some()
            }
            HostCommand::PinProject => self.pin_project_target().is_some(),
            _ => true,
        }
    }
    fn server_thread(&self, key: &ThreadKey) -> Option<Rc<OrchestrationV2ThreadShell>> {
        self.runtime
            .store()
            .shared_shell(&key.environment)
            .and_then(|shell| shell.threads.iter().find(|t| t.id == key.thread).cloned())
    }
    fn environment_config(
        &self,
        environment: &EnvironmentId,
    ) -> Option<Rc<t3_contracts::server::ServerConfig>> {
        self.runtime
            .store()
            .snapshot()
            .environments
            .get(environment)
            .and_then(|e| e.config.clone())
    }
    fn active_draft_key(&self) -> Option<String> {
        let drafts = self.runtime.composer_drafts();
        match &*self.route.borrow() {
            Route::Draft(id) => Some(id.clone()),
            Route::Thread(key) | Route::ThreadPanel(key, _) => Some(drafts.editor_draft_key(key)),
            _ => None,
        }
    }
    fn active_draft_prompt(&self) -> Option<String> {
        let id = self.active_draft_key()?;
        let prompt = self.runtime.composer_drafts().view_key(&id).prompt.clone();
        trimmed_linkable_prompt(&prompt)
    }
    fn active_prompt_repository(&self) -> Option<Repository> {
        let key = self.active_project()?;
        self.runtime
            .new_threads()
            .projects()
            .iter()
            .find(|p| p.key() == key)
            .and_then(|p| {
                p.project
                    .repository_identity
                    .as_ref()
                    .and_then(Option::as_ref)
            })
            .and_then(prompt_link_repository_of)
    }
    fn active_thread_reference_copy_target(&self) -> Option<ThreadReferenceCopyTarget> {
        if self.is_draft() {
            return None;
        }
        let key = self.key()?;
        let thread = self.server_thread(&key)?;
        let hosted_origin = self.hosted_app_url.borrow().clone();
        Some(reference_copy_target(
            &key,
            &thread,
            hosted_origin.as_deref(),
        ))
    }
    /// The thread's or draft's project, else the one the landing state starts threads in.
    fn contextual_project(&self) -> Option<ProjectKey> {
        self.active_project()
            .or_else(|| self.runtime.new_threads().default_project_key())
    }
    fn pin_project_target(&self) -> Option<PinProjectTarget> {
        let project = self.contextual_project()?;
        self.pin_target_for_project(&project)
    }
    fn pin_target_for_project(&self, project: &ProjectKey) -> Option<PinProjectTarget> {
        let group = self.runtime.settings().groups().into_iter().find(|group| {
            group.members.iter().any(|member| {
                member.environment_id == project.environment && member.id == project.project
            })
        })?;
        let members: Vec<ProjectKey> = group
            .members
            .iter()
            .map(|member| ProjectKey {
                environment: member.environment_id.clone(),
                project: member.id.clone(),
            })
            .collect();
        if !self
            .runtime
            .thread_actions()
            .can_organize_projects(&members)
        {
            return None;
        }
        let physical = self.runtime.new_threads().projects();
        let pinned = physical.iter().any(|p| {
            members.contains(&p.key())
                && p.project
                    .pinned_at
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_some()
        });
        Some(PinProjectTarget {
            display_name: group.display_name,
            pinned,
            members,
        })
    }
    pub fn catalogue(&self) -> Catalogue {
        let environments = self.runtime.settings().environments();
        let physical = self.runtime.new_threads().projects();
        let logical = self.runtime.settings().groups();
        let representatives = logical
            .iter()
            .filter_map(|group| {
                group.members.iter().find_map(|member| {
                    physical
                        .iter()
                        .find(|p| {
                            p.environment == member.environment_id && p.project.id == member.id
                        })
                        .cloned()
                })
            })
            .collect::<Vec<_>>();
        let projects = palette::project_items(
            &representatives,
            "project",
            |p| {
                logical
                    .iter()
                    .find(|g| {
                        g.members
                            .iter()
                            .any(|m| m.environment_id == p.environment && m.id == p.project.id)
                    })
                    .map_or(p.project.title.0.clone(), |g| g.display_name.clone())
            },
            |p| vec![p.environment.0.clone()],
        );
        let names = physical
            .iter()
            .map(|p| (p.key(), p.project.title.0.clone()))
            .collect::<BTreeMap<_, _>>();
        let snapshot = self.runtime.store().snapshot();
        let mut rows = snapshot
            .environments
            .iter()
            .flat_map(|(env, state)| {
                state.shell.snapshot.iter().flat_map(move |shell| {
                    shell
                        .threads
                        .iter()
                        .map(move |thread| sidebar::SidebarThread {
                            key: ThreadKey {
                                environment: env.clone(),
                                thread: thread.id.clone(),
                            },
                            project: ProjectKey {
                                environment: env.clone(),
                                project: thread.project_id.clone(),
                            },
                            shell: thread,
                        })
                })
            })
            .collect::<Vec<_>>();
        rows.sort_by(|a, b| {
            let subject = |row: &sidebar::SidebarThread<'_>| self.subject(row, None);
            search::recent_use(&subject(b))
                .total_cmp(&search::recent_use(&subject(a)))
                .then_with(|| b.key.thread.cmp(&a.key.thread))
                .then_with(|| b.key.environment.cmp(&a.key.environment))
        });
        let threads = palette::thread_items(&rows, self.key().as_ref(), &names, |row, title| {
            self.subject(row, title)
        });
        let mut actions = vec![];
        if let Some(current) = self
            .active_project()
            .or_else(|| representatives.first().map(|p| p.key()))
            && let Some(project) = physical.iter().find(|p| p.key() == current)
        {
            let mut item = Item::action(
                "action:new-thread",
                format!("New thread in {}", project.project.title.0),
            );
            item.project = Some(current);
            item.styled_title = true;
            item.search_title = Some(item.title.clone());
            item.search_terms = vec![
                "new thread".into(),
                "chat".into(),
                "create".into(),
                "draft".into(),
            ];
            item.shortcut = Some("chat.newLocal".into());
            actions.push(item);
        }
        if !projects.is_empty() {
            let mut item = Item::action("action:new-thread-in", "New thread in...");
            item.groups = Some(vec![Group {
                value: "new-thread-in".into(),
                label: "Projects".into(),
                items: projects.clone(),
            }]);
            actions.push(item);
        }
        if environments.iter().any(|e| {
            e.phase == crate::connection::Phase::Connected
                && e.config
                    .as_ref()
                    .is_some_and(|c| c.scratch_workspace_root.is_some())
        }) {
            let mut item = Item::action(
                "action:new-thread-without-project",
                "New thread without a project",
            );
            item.search_terms = vec![
                "new thread".into(),
                "no project".into(),
                "without project".into(),
                "none".into(),
                "chat".into(),
            ];
            actions.push(item);
        }
        for (value, title, terms, shortcut) in [
            (
                "action:open-file-picker",
                "Go to file",
                "go to file|open file|file picker|find file|quick open",
                Some("filePicker.toggle"),
            ),
            (
                "action:search-project-contents",
                "Search project contents",
                "search project|find in files|grep|content search|text search",
                Some("projectSearch.toggle"),
            ),
            (
                "action:add-project",
                "Add project",
                "add project|folder|directory|browse|clone|remote|repository|repo|git|url|environment|github|gitlab|gitcafe|forgejo|bitbucket|azure",
                None,
            ),
            (
                "action:usage",
                "Open usage",
                "usage|use|tokens|cost|spend|limits|stats|analytics",
                Some("usage.open"),
            ),
            (
                "action:settings",
                "Open settings",
                "settings|preferences|configuration|keybindings",
                None,
            ),
        ] {
            let mut item = Item::action(value, title);
            item.search_terms = terms.split('|').map(str::to_owned).collect();
            item.shortcut = shortcut.map(str::to_owned);
            item.keep_open = matches!(
                value,
                "action:open-file-picker" | "action:search-project-contents" | "action:add-project"
            );
            actions.push(item);
        }
        if environments.iter().any(|e| e.config.is_some()) {
            actions.push(Item::action("action:pull-requests", "Open pull requests"));
        }
        if environments.iter().any(|e| {
            e.phase == crate::connection::Phase::Connected
                && e.config
                    .as_ref()
                    .is_some_and(|c| c.new_projects_root.is_some())
        }) {
            actions.push(Item::action("action:new-project", "New project"));
        }
        for (command, value, title) in host_items() {
            if !self.host_available(&command) {
                continue;
            }
            let mut item = Item::action(value, title);
            match command {
                HostCommand::FindThread => {
                    item.search_terms = ["find", "search", "messages", "plans"]
                        .map(str::to_owned)
                        .into();
                    item.shortcut = Some("chat.find".into());
                }
                HostCommand::CopyPrompt => {
                    item.search_terms = ["copy", "share", "prompt", "link"]
                        .map(str::to_owned)
                        .into();
                }
                HostCommand::CopyPromptContext => {
                    item.search_terms = ["copy", "share", "prompt", "link", "repository", "model"]
                        .map(str::to_owned)
                        .into();
                }
                HostCommand::CopyReference => {
                    if let Some(target) = self.active_thread_reference_copy_target() {
                        item.title = match target.kind {
                            ThreadReferenceKind::PullRequest => "Copy PR link".into(),
                            ThreadReferenceKind::Thread => "Copy thread".into(),
                        };
                        item.description = Some(target.description);
                        item.search_terms = match target.kind {
                            ThreadReferenceKind::PullRequest => {
                                ["copy", "pull request", "link", "reference"]
                                    .map(str::to_owned)
                                    .into()
                            }
                            ThreadReferenceKind::Thread => {
                                ["copy", "thread", "thread id", "reference"]
                                    .map(str::to_owned)
                                    .into()
                            }
                        };
                        item.shortcut = Some("thread.copyReference".into());
                    }
                }
                HostCommand::LinkPullRequest => {
                    item.search_terms = ["link", "pull request", "pr", "attach", "stack"]
                        .map(str::to_owned)
                        .into();
                }
                HostCommand::ShowPullRequests => {
                    item.search_terms = ["pull requests", "linked", "stack", "prs"]
                        .map(str::to_owned)
                        .into();
                    item.disabled = self
                        .key()
                        .and_then(|key| self.server_thread(&key))
                        .is_none_or(|thread| thread_pull_requests_of(&thread).is_empty());
                }
                HostCommand::ShareGuest => {
                    item.search_terms = ["share", "guest", "invite", "link", "teammate"]
                        .map(str::to_owned)
                        .into();
                }
                HostCommand::RestartSession => {
                    item.search_terms = [
                        "restart", "reset", "reload", "agent", "session", "skills", "plugins",
                        "mcp",
                    ]
                    .map(str::to_owned)
                    .into();
                }
                HostCommand::PinProject => {
                    if let Some(pin) = self.pin_project_target() {
                        item.title = if pin.pinned {
                            "Unpin project".into()
                        } else {
                            "Pin project".into()
                        };
                        item.pinned = Some(pin.pinned);
                        item.description = Some(pin.display_name);
                        item.search_terms = ["pin", "unpin", "project", "favorite"]
                            .map(str::to_owned)
                            .into();
                    }
                }
                _ => {}
            }
            actions.push(item);
        }
        if let Some(project) = self
            .active_project()
            .or_else(|| representatives.first().map(|p| p.key()))
        {
            let mut item = Item::action("action:project-settings", "Project settings");
            item.project = Some(project);
            actions.push(item);
        }
        use crate::services::settings::search as setting;
        let settings_service = self.runtime.settings();
        let availability = setting::SettingsSearchAvailability::from_environments(
            &settings_service.environments(),
            settings_service.primary_environment().is_some(),
        );
        let available = setting::filter_available_settings_search_items(&availability);
        let settings =
            setting::settings_for_this_platform(&available, *self.settings_platform.borrow())
                .into_iter()
                .map(|setting| {
                    let mut item = Item::action(format!("setting:{}", setting.id), setting.title);
                    item.search_terms = setting.search_terms.iter().map(|s| (*s).into()).collect();
                    item.secondary = setting.secondary;
                    item
                })
                .collect();
        Catalogue {
            groups: Rc::new(palette::root_groups(
                actions,
                threads
                    .iter()
                    .take(palette::RECENT_THREAD_LIMIT)
                    .cloned()
                    .collect(),
            )),
            projects: Rc::new(projects),
            threads: Rc::new(threads),
            settings: Rc::new(settings),
        }
    }
    fn subject(
        &self,
        row: &sidebar::SidebarThread<'_>,
        title: Option<&str>,
    ) -> search::ThreadSubject {
        let label = self
            .runtime
            .environments()
            .iter()
            .find(|e| e.id == row.key.environment)
            .map(|e| e.label.clone());
        crate::thread_views::threads::search::thread_search_subject(
            &row.key,
            row.shell,
            title,
            label.as_deref(),
            &Default::default(),
            &crate::time::epoch_ms,
        )
    }
    pub fn filter(&self, query: &str, active: Option<&[Group]>, messages: &[Item]) -> Vec<Group> {
        let catalogue = self.catalogue();
        let mut threads = catalogue.threads.as_ref().clone();
        let mut listed = threads
            .iter()
            .filter_map(|item| item.thread.clone())
            .collect::<BTreeSet<_>>();
        for item in messages.iter().filter(|item| item.thread_search.is_some()) {
            if let Some(key) = &item.thread
                && listed.insert(key.clone())
            {
                threads.push(item.clone());
            }
        }
        let messages = messages
            .iter()
            .filter(|item| item.thread_search.is_none())
            .cloned()
            .collect::<Vec<_>>();
        let root = catalogue.groups.as_ref();
        let nested = palette::nested_items(
            &root
                .iter()
                .flat_map(|g| g.items.clone())
                .collect::<Vec<_>>(),
            query,
        );
        palette::filter_groups(&FilterInput {
            active: active.unwrap_or(root),
            query,
            submenu: active.is_some(),
            root: active.map(|_| root.as_slice()),
            nested: &nested,
            projects: &catalogue.projects,
            settings: &catalogue.settings,
            threads: &threads,
            messages: &messages,
            now: self.platform.now_epoch_ms() as f64,
        })
    }
    pub fn watch(
        &self,
        query: String,
    ) -> LocalBoxStream<'static, Result<Rc<Vec<Group>>, ClientError>> {
        let this = self.clone();
        let request = self.clone();
        let search = query.clone();
        let events = futures::stream::select(
            self.changes().map(|_| None),
            futures::stream::once(async move {
                Some(
                    request
                        .message_matches(if search.starts_with('>') { "" } else { &search })
                        .await,
                )
            }),
        );
        events
            .scan(Vec::new(), move |messages, event| {
                let result = match event {
                    Some(Ok(next)) => {
                        *messages = next;
                        Ok(Rc::new(this.filter(&query, None, messages)))
                    }
                    Some(Err(error)) => Err(error),
                    None => Ok(Rc::new(this.filter(&query, None, messages))),
                };
                futures::future::ready(Some(result))
            })
            .boxed_local()
    }
    pub async fn activate(&self, item: &Item) -> Result<Action, ClientError> {
        let catalogue = self.catalogue();
        let fresh = catalogue
            .groups
            .iter()
            .flat_map(|g| &g.items)
            .chain(catalogue.projects.iter())
            .chain(catalogue.threads.iter())
            .chain(catalogue.settings.iter())
            .find(|row| row.value == item.value && !row.disabled);
        if fresh.is_none()
            && let Some(key) = item
                .thread
                .as_ref()
                .filter(|key| self.message_targets.borrow().contains(key))
        {
            return Ok(Action::Navigate(Route::Thread(key.clone())));
        }
        let fresh = fresh.ok_or(ClientError::Cancelled)?;
        if let Some(id) = fresh.value.strip_prefix("setting:") {
            let setting = crate::services::settings::search::SETTINGS_SEARCH_ITEMS
                .iter()
                .find(|s| s.id == id)
                .ok_or(ClientError::Cancelled)?;
            return Ok(Action::NavigateAddress(format!(
                "{}#{}",
                setting.to,
                setting.anchor()
            )));
        }
        if let Some(key) = &fresh.thread {
            return Ok(Action::Navigate(Route::Thread(key.clone())));
        }
        if let Some(key) = &fresh.project {
            if fresh.value == "action:project-settings" {
                let group = self
                    .runtime
                    .settings()
                    .groups()
                    .into_iter()
                    .find(|g| {
                        g.members
                            .iter()
                            .any(|m| m.environment_id == key.environment && m.id == key.project)
                    })
                    .ok_or(ClientError::Cancelled)?;
                return Ok(Action::Navigate(Route::ProjectLink(group.project_key)));
            }
            let origin = match &*self.route.borrow() {
                Route::Draft(id) => Some(id.clone()),
                _ => self
                    .key()
                    .map(|key| self.runtime.composer_drafts().thread_draft_key(&key)),
            };
            let id = self
                .runtime
                .new_threads()
                .open_project(key.clone(), origin.as_deref(), None)
                .await?;
            return Ok(Action::Navigate(Route::Draft(id)));
        }
        Ok(match fresh.value.as_str() {
            "action:open-file-picker" => Action::Files,
            "action:search-project-contents" => Action::Content,
            "action:add-project" => Action::Navigate(Route::AddProject),
            "action:new-project" => Action::NewProject,
            "action:new-thread-without-project" => {
                let new = self.runtime.new_threads();
                new.start_scratch(None).await?;
                match new
                    .resolve(&crate::navigation::new_thread::Target::Home)
                    .await?
                {
                    crate::navigation::new_thread::NewThreadView::Navigate(destination) => {
                        Action::Navigate(match destination {
                            crate::navigation::new_thread::Destination::Draft(id) => {
                                Route::Draft(id)
                            }
                            crate::navigation::new_thread::Destination::Thread(key) => {
                                Route::Thread(key)
                            }
                        })
                    }
                    _ => return Err(ClientError::Cancelled),
                }
            }
            "action:settings" => Action::Navigate(Route::Settings(SettingsPage::General)),
            "action:usage" => Action::Navigate(Route::Usage),
            "action:pull-requests" => Action::Navigate(Route::PullRequests),
            value => {
                let command = host_items()
                    .into_iter()
                    .find(|(_, id, _)| *id == value)
                    .map(|(c, _, _)| c)
                    .ok_or(ClientError::Unsupported("palette submenu"))?;
                Action::Host {
                    command,
                    thread: self.key(),
                    project: self.contextual_project(),
                }
            }
        })
    }

    /// Copy either the active thread's PR link or bare hosted URL to the platform clipboard.
    pub async fn copy_thread_reference(
        &self,
        key: &ThreadKey,
    ) -> Result<PaletteToast, PaletteHostFailure> {
        let Some(thread) = self.server_thread(key) else {
            return Err(PaletteHostFailure {
                title: "Failed to copy thread",
                description: "This thread is no longer available.".into(),
            });
        };
        let hosted_origin = self.hosted_app_url.borrow().clone();
        let target = reference_copy_target(key, &thread, hosted_origin.as_deref());
        let mut flavours = vec![MimeData {
            mime: "text/plain".into(),
            bytes: target.value.clone().into_bytes(),
        }];
        if let Some(fragment) = &target.fragment {
            flavours.push(MimeData {
                mime: "text/html".into(),
                bytes: clipboard_html(&target.value, fragment).into_bytes(),
            });
        }
        if let Err(error) = self.platform.clipboard_write(flavours).await {
            return Err(PaletteHostFailure {
                title: target.failure_title,
                description: error.to_string(),
            });
        }
        Ok(PaletteToast {
            title: target.success_title,
            description: Some(target.description),
        })
    }

    /// Build and copy a shareable prompt link (with optional repository/provider/model context).
    pub async fn copy_prompt_link(
        &self,
        key: Option<&ThreadKey>,
        with_context: bool,
    ) -> Result<PaletteToast, PaletteHostFailure> {
        let link = self
            .build_prompt_link(key, with_context)
            .ok_or_else(|| PaletteHostFailure {
                title: "Could not copy the prompt link",
                description: "Prompt link is unavailable.".into(),
            })?;
        if let Err(error) = self
            .platform
            .clipboard_write(vec![MimeData {
                mime: "text/plain".into(),
                bytes: link.into_bytes(),
            }])
            .await
        {
            return Err(PaletteHostFailure {
                title: "Could not copy the prompt link",
                description: error.to_string(),
            });
        }
        Ok(PaletteToast {
            title: "Prompt link copied",
            description: None,
        })
    }

    fn build_prompt_link(&self, key: Option<&ThreadKey>, with_context: bool) -> Option<String> {
        let draft_id = match &*self.route.borrow() {
            Route::Draft(id) => Some(id.clone()),
            _ => key.map(|k| self.runtime.composer_drafts().editor_draft_key(k)),
        }?;
        let prompt = self
            .runtime
            .composer_drafts()
            .view_key(&draft_id)
            .prompt
            .clone();
        let trimmed = trimmed_linkable_prompt(&prompt)?;
        let hosted_origin = self.hosted_app_url.borrow().clone();
        if !with_context {
            return build_new_thread_link(&trimmed, None, None, None, hosted_origin.as_deref())
                .ok();
        }
        let repo = self.active_prompt_repository()?;
        let server_thread = key.and_then(|k| self.server_thread(k));
        let environment = key
            .map(|k| k.environment.clone())
            .or_else(|| self.active_project().map(|p| p.environment));
        let (selection, providers) = {
            let document = self.runtime.composer_drafts().document();
            let doc = document.borrow();
            let draft = &doc["draftsByThreadKey"][&draft_id];
            let draft_selection = draft["activeProvider"]
                .as_str()
                .filter(|p| !p.is_empty())
                .and_then(|provider| {
                    serde_json::from_value::<ModelSelection>(
                        draft["modelSelectionByProvider"][provider].clone(),
                    )
                    .ok()
                });
            let selection = draft_selection
                .or_else(|| server_thread.as_ref().map(|t| t.model_selection.clone()));
            let providers = environment
                .as_ref()
                .and_then(|env| self.environment_config(env))
                .map(|c| c.providers.0.clone())
                .unwrap_or_default();
            (selection, providers)
        };
        let provider = selection.as_ref().and_then(|sel| {
            providers
                .iter()
                .find(|candidate| candidate.instance_id == sel.instance_id)
                .map(|candidate| candidate.driver.0.clone())
        });
        let model = provider
            .as_ref()
            .and_then(|_| selection.as_ref().map(|sel| sel.model.clone()));
        build_new_thread_link(
            &trimmed,
            Some(&repo),
            provider.as_deref(),
            model.as_deref(),
            hosted_origin.as_deref(),
        )
        .ok()
    }

    /// Detach any active provider sessions, signal restart feedback, and refresh providers.
    pub async fn restart_agent_session(
        &self,
        key: &ThreadKey,
        on_restarting: impl FnOnce(PaletteToast),
    ) -> Result<(), ClientError> {
        let thread = self
            .server_thread(key)
            .ok_or_else(|| ClientError::Rejected {
                message: "This thread is no longer available.".into(),
                detail: None,
            })?;
        let endpoint = self
            .runtime
            .client(&key.environment)
            .and_then(|client| client.session())
            .ok_or(ClientError::Disconnected)?;
        let time_ms =
            |t: &t3_contracts::wire::WireDateTime| crate::time::epoch_ms(&t.0).unwrap_or(0);
        let stored_projection = self.runtime.store().shared_thread(key);
        let shell_runtime = shell_has_runtime(&thread);
        let projection = if let Some(stored) = stored_projection {
            let full = stored.source();
            let has_runtime =
                crate::thread_views::threads::execution::derive_thread_structure_runtime(
                    &full, &time_ms,
                )
                .is_some()
                    || shell_runtime;
            has_runtime.then_some(full)
        } else if shell_runtime {
            let query = OrchestrationV2GetThreadProjectionInput {
                thread_id: key.thread.clone(),
            };
            if endpoint.available::<WsOrchestrationV2GetThreadProjectionRpc>(&query) {
                Some(Rc::new(
                    endpoint
                        .command::<WsOrchestrationV2GetThreadProjectionRpc>(query)
                        .await
                        .map_err(RpcCallError::into_client_error)?,
                ))
            } else {
                None
            }
        } else {
            None
        };
        let has_runtime = projection.is_some() || shell_runtime;
        if let Some(projection) = projection.as_deref()
            && !projection.provider_sessions.is_empty()
        {
            let command_id = crate::operations::allocate_command_id(&self.platform, None)?;
            for command in crate::operations::detach_provider_sessions(&command_id, projection)? {
                if !endpoint.available::<WsOrchestrationV2DispatchCommandRpc>(&command) {
                    return Err(ClientError::PermissionDenied {
                        method: "orchestration.dispatchCommand".into(),
                    });
                }
                endpoint
                    .command::<WsOrchestrationV2DispatchCommandRpc>(command)
                    .await
                    .map_err(RpcCallError::into_client_error)?;
            }
        }
        on_restarting(PaletteToast {
            title: "Agent session will restart",
            description: Some("Your next message starts a fresh session.".into()),
        });
        let project_key = ProjectKey {
            environment: key.environment.clone(),
            project: thread.project_id.clone(),
        };
        let Some(project) = self
            .runtime
            .new_threads()
            .projects()
            .into_iter()
            .find(|candidate| candidate.key() == project_key)
        else {
            return Ok(());
        };
        let cwd = thread
            .worktree_path
            .clone()
            .unwrap_or_else(|| project.project.workspace_root.clone());
        let instance_id = if has_runtime {
            projection
                .as_ref()
                .and_then(|p| match &p.thread.provider_instance_id {
                    t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(id) => Some(id.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| thread.provider_instance_id.clone())
        } else {
            thread.model_selection.instance_id.clone()
        };
        let payload = WsServerRefreshProvidersPayload {
            instance_id: Some(Some(instance_id)),
            cwd: Some(Some(cwd)),
            fresh: Some(Some(true)),
            refresh_models: None,
            model_catalog_only: None,
            reload_secrets: None,
            workspace_snapshots_on_demand: None,
            usage_limits_command: None,
            provider_catalogs_on_demand: None,
            omit_providers: None,
            machine_catalog: None,
        };
        if !endpoint.available::<WsServerRefreshProvidersRpc>(&payload) {
            return Err(ClientError::PermissionDenied {
                method: "server.refreshProviders".into(),
            });
        }
        endpoint
            .command::<WsServerRefreshProvidersRpc>(payload)
            .await
            .map_err(RpcCallError::into_client_error)?;
        Ok(())
    }

    /// A project that can no longer be pinned (gone, or no scope) is skipped without a toast.
    pub async fn toggle_project_pin(&self, project: &ProjectKey) -> Result<(), PaletteHostFailure> {
        let Some(target) = self.pin_target_for_project(project) else {
            return Ok(());
        };
        let title = if target.pinned {
            "Failed to unpin project"
        } else {
            "Failed to pin project"
        };
        match self
            .runtime
            .thread_actions()
            .set_project_group_pinned(target.members, !target.pinned)
            .await
        {
            Err(ClientError::Cancelled) => Ok(()),
            Err(error) => Err(PaletteHostFailure {
                title,
                description: error.to_string(),
            }),
            Ok(()) => Ok(()),
        }
    }

    /// Copy the active thread's reference when one is offered (the `thread.copyReference`
    /// shortcut); `None` when the palette row would not be offered.
    pub fn copy_active_thread_reference(
        &self,
    ) -> Option<futures::future::LocalBoxFuture<'static, Result<PaletteToast, PaletteHostFailure>>>
    {
        self.active_thread_reference_copy_target()?;
        let key = self.key()?;
        let service = self.clone();
        Some(futures::FutureExt::boxed_local(async move {
            service.copy_thread_reference(&key).await
        }))
    }

    pub fn pull_request_linking_supported(&self, environment: &EnvironmentId) -> bool {
        self.environment_config(environment).is_some()
    }

    pub fn resolve_link_pull_request(
        &self,
        key: &ThreadKey,
        reference: &str,
    ) -> PullRequestInputResolution {
        let supported = self.pull_request_linking_supported(&key.environment);
        let project = self.server_thread(key).and_then(|thread| {
            let pkey = ProjectKey {
                environment: key.environment.clone(),
                project: thread.project_id.clone(),
            };
            self.runtime
                .new_threads()
                .projects()
                .into_iter()
                .find(|p| p.key() == pkey)
                .and_then(|p| {
                    pull_request_link_project(
                        p.project
                            .repository_identity
                            .as_ref()
                            .and_then(Option::as_ref),
                    )
                })
        });
        resolve_link_pull_request_input(reference, project.as_ref(), |url| {
            supported && parsed_key(url).is_some()
        })
    }

    pub async fn link_pull_request(
        &self,
        key: &ThreadKey,
        link: ResolvedPullRequestLink,
    ) -> Result<(), ClientError> {
        if !self.pull_request_linking_supported(&key.environment) {
            return Err(ClientError::Rejected {
                message: "This environment does not support linking this pull request.".into(),
                detail: None,
            });
        }
        if parsed_key(&link.url).is_none() {
            return Err(ClientError::Rejected {
                message: "The pull request is not available in this environment.".into(),
                detail: None,
            });
        }
        let endpoint = self
            .runtime
            .client(&key.environment)
            .and_then(|client| client.session())
            .ok_or(ClientError::Disconnected)?;
        let command_id = crate::operations::allocate_command_id(&self.platform, None)?;
        let command = build_link_command(command_id, key.thread.clone(), link);
        if !endpoint.available::<WsOrchestrationV2DispatchCommandRpc>(&command) {
            return Err(ClientError::PermissionDenied {
                method: "orchestration.dispatchCommand".into(),
            });
        }
        endpoint
            .command::<WsOrchestrationV2DispatchCommandRpc>(command)
            .await
            .map_err(RpcCallError::into_client_error)?;
        Ok(())
    }
}

fn trimmed_linkable_prompt(prompt: &str) -> Option<String> {
    if crate::thread_views::user_message::is_attachment_only_bootstrap_prompt(prompt) {
        return None;
    }
    let trimmed = prompt.js_trim();
    if trimmed.is_empty() || trimmed.encode_utf16().count() > MAX_PROMPT_LENGTH {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

pub(crate) fn prompt_link_repository_of(
    identity: &t3_contracts::environment::RepositoryIdentity,
) -> Option<Repository> {
    let key = identity
        .origin
        .as_ref()
        .map_or(&identity.canonical_key.0, |origin| &origin.canonical_key.0);
    key.strip_prefix("github.com/")
        .and_then(prompt_links::parse_repository)
}

fn shell_has_runtime(thread: &OrchestrationV2ThreadShell) -> bool {
    thread.latest_run_id.is_some()
        || thread.active_run_id.is_some()
        || thread.status != t3_contracts::orchestration_v2::OrchestrationV2ShellThreadStatus::Idle
        || thread.blocked_on_thread_wait == Some(Some(true))
        || thread
            .pull_requests
            .as_ref()
            .and_then(Option::as_ref)
            .is_some_and(|prs| {
                prs.iter()
                    .any(|pr| pr.watch.as_ref().and_then(Option::as_ref).is_some())
            })
}

fn thread_pull_requests_of(thread: &OrchestrationV2ThreadShell) -> Vec<ThreadPullRequestLink> {
    if let Some(links) = thread.pull_requests.as_ref().and_then(Option::as_ref) {
        return links
            .iter()
            .filter(|link| link.source != ThreadPullRequestLinkSource::StackDismissed)
            .cloned()
            .collect();
    }
    if thread.pull_requests.is_some() {
        return Vec::new();
    }
    let Some(legacy) = thread
        .linked_pull_request
        .as_ref()
        .and_then(Option::as_ref)
        .or_else(|| thread.branch_pull_request.as_ref().and_then(Option::as_ref))
    else {
        return Vec::new();
    };
    let key = legacy_key(legacy);
    vec![ThreadPullRequestLink {
        host: key.host,
        repository: key.repository,
        number: key.number,
        url: legacy.url.clone(),
        repository_web_url: None,
        source: ThreadPullRequestLinkSource::Manual,
        linked_at: t3_contracts::wire::WireDateTime("1970-01-01T00:00:00.000Z".into()),
        snapshot: None,
        stack: None,
        watch: None,
        ended_watch: None,
    }]
}

fn is_open_pull_request(link: &ThreadPullRequestLink) -> bool {
    link.snapshot
        .as_ref()
        .is_none_or(|snapshot| snapshot.state == PullRequestState::Open)
}

fn pull_request_updated_ms(link: &ThreadPullRequestLink) -> i64 {
    let timestamp = link
        .snapshot
        .as_ref()
        .and_then(|s| s.updated_at.as_ref())
        .map_or(link.linked_at.0.as_str(), |t| t.0.as_str());
    crate::time::epoch_ms(timestamp).unwrap_or(0)
}

#[derive(Clone)]
struct ChainEntry<'a> {
    index: usize,
    link: &'a ThreadPullRequestLink,
    depth: usize,
}

fn layers_from_top<'a>(mut chain: Vec<ChainEntry<'a>>) -> Vec<&'a ThreadPullRequestLink> {
    chain.sort_by(|a, b| b.depth.cmp(&a.depth).then_with(|| b.index.cmp(&a.index)));
    chain.into_iter().map(|entry| entry.link).collect()
}

fn resolve_thread_pull_request_chains(links: &[ThreadPullRequestLink]) -> Vec<Vec<ChainEntry<'_>>> {
    let keys: Vec<_> = links.iter().map(normalize_key).collect();
    let mut consumed = vec![false; links.len()];
    let mut chains: Vec<Vec<ChainEntry<'_>>> = Vec::new();

    // 1. Native stacks grouped by (host, repository, stack.id).
    let mut group_order: Vec<(String, String, String)> = Vec::new();
    let mut groups: BTreeMap<(String, String, String), Vec<usize>> = BTreeMap::new();
    for (idx, link) in links.iter().enumerate() {
        let Some(stack) = link.stack.as_ref() else {
            continue;
        };
        let key = (
            keys[idx].host.0.clone(),
            keys[idx].repository.0.clone(),
            stack.id.0.to_ascii_lowercase(),
        );
        if !groups.contains_key(&key) {
            group_order.push(key.clone());
        }
        groups.entry(key).or_default().push(idx);
    }
    for group_key in group_order {
        let indices = &groups[&group_key];
        let member_numbers: BTreeSet<u64> = indices.iter().map(|&i| links[i].number).collect();
        let best_layers = indices
            .iter()
            .filter_map(|&i| links[i].stack.as_ref().map(|s| &s.layers))
            .max_by(|a, b| {
                let cov = |layers: &&Vec<
                    t3_contracts::thread_pull_request::ThreadPullRequestStackLayer,
                >| {
                    let unique: BTreeSet<u64> = layers
                        .iter()
                        .map(|l| l.number)
                        .filter(|n| member_numbers.contains(n))
                        .collect();
                    (unique.len(), layers.len())
                };
                cov(a).cmp(&cov(b))
            });
        let mut layer_order = BTreeMap::new();
        if let Some(layers) = best_layers {
            for (pos, layer) in layers.iter().enumerate() {
                layer_order.entry(layer.number).or_insert(pos);
            }
        }
        let mut ordered = indices.clone();
        ordered.sort_by(|&a, &b| {
            let pos_a = layer_order
                .get(&links[a].number)
                .copied()
                .unwrap_or(usize::MAX);
            let pos_b = layer_order
                .get(&links[b].number)
                .copied()
                .unwrap_or(usize::MAX);
            pos_a.cmp(&pos_b).then_with(|| a.cmp(&b))
        });
        let chain = ordered
            .into_iter()
            .enumerate()
            .map(|(depth, idx)| {
                consumed[idx] = true;
                ChainEntry {
                    index: idx,
                    link: &links[idx],
                    depth,
                }
            })
            .collect();
        chains.push(chain);
    }

    // 2. Derived baseBranch -> headBranch chains over remaining links.
    let remaining: Vec<usize> = (0..links.len()).filter(|&i| !consumed[i]).collect();
    if !remaining.is_empty() {
        let mut by_head: BTreeMap<(String, String, String), Vec<usize>> = BTreeMap::new();
        for &idx in &remaining {
            if let Some(snapshot) = links[idx].snapshot.as_ref() {
                by_head
                    .entry((
                        keys[idx].host.0.clone(),
                        keys[idx].repository.0.clone(),
                        snapshot.head_branch.0.to_ascii_lowercase(),
                    ))
                    .or_default()
                    .push(idx);
            }
        }
        let unique_by_branch: BTreeMap<(String, String, String), usize> = by_head
            .into_iter()
            .filter_map(|(k, v)| (v.len() == 1).then(|| (k, v[0])))
            .collect();
        let mut parent_of = BTreeMap::new();
        let mut children_of: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for &idx in &remaining {
            if let Some(snapshot) = links[idx].snapshot.as_ref()
                && let Some(&parent) = unique_by_branch.get(&(
                    keys[idx].host.0.clone(),
                    keys[idx].repository.0.clone(),
                    snapshot.base_branch.0.to_ascii_lowercase(),
                ))
                && parent != idx
            {
                parent_of.insert(idx, parent);
                children_of.entry(parent).or_default().push(idx);
            }
        }

        fn visit_chain<'a>(
            idx: usize,
            depth: usize,
            links: &'a [ThreadPullRequestLink],
            children_of: &BTreeMap<usize, Vec<usize>>,
            visited: &mut BTreeSet<usize>,
            out: &mut Vec<ChainEntry<'a>>,
        ) {
            if !visited.insert(idx) {
                return;
            }
            out.push(ChainEntry {
                index: idx,
                link: &links[idx],
                depth,
            });
            if let Some(children) = children_of.get(&idx) {
                for &child in children {
                    visit_chain(child, depth + 1, links, children_of, visited, out);
                }
            }
        }

        let mut visited = BTreeSet::new();
        for &root in &remaining {
            if !parent_of.contains_key(&root) && !visited.contains(&root) {
                let mut chain = Vec::new();
                visit_chain(root, 0, links, &children_of, &mut visited, &mut chain);
                chains.push(chain);
            }
        }
        for &idx in &remaining {
            if !visited.contains(&idx) {
                let mut chain = Vec::new();
                visit_chain(idx, 0, links, &children_of, &mut visited, &mut chain);
                chains.push(chain);
            }
        }
    }

    chains
}

fn resolve_thread_current_pull_request_link(
    thread: &OrchestrationV2ThreadShell,
) -> Option<ThreadPullRequestLink> {
    let mut links = thread_pull_requests_of(thread);
    if links.is_empty() {
        return None;
    }
    let open: Vec<&ThreadPullRequestLink> =
        links.iter().filter(|l| is_open_pull_request(l)).collect();
    if open.len() == 1 {
        return Some(open[0].clone());
    }
    let chains = resolve_thread_pull_request_chains(&links);
    if open.len() > 1 {
        let mut open_chains: Vec<Vec<&ThreadPullRequestLink>> = chains
            .iter()
            .cloned()
            .map(|chain| {
                layers_from_top(chain)
                    .into_iter()
                    .filter(|l| is_open_pull_request(l))
                    .collect::<Vec<_>>()
            })
            .filter(|chain| !chain.is_empty())
            .collect();
        if !open_chains.is_empty() {
            if open_chains.len() > 1 {
                open_chains.sort_by(|a, b| {
                    let max_linked = |items: &Vec<&ThreadPullRequestLink>| {
                        items
                            .iter()
                            .map(|l| crate::time::epoch_ms(&l.linked_at.0).unwrap_or(0))
                            .max()
                            .unwrap_or(0)
                    };
                    max_linked(b).cmp(&max_linked(a))
                });
            }
            return Some(open_chains[0][0].clone());
        }
    }
    if chains.len() == 1 {
        return layers_from_top(chains.into_iter().next().unwrap_or_default())
            .into_iter()
            .next()
            .cloned();
    }
    links.sort_by_key(|a| std::cmp::Reverse(pull_request_updated_ms(a)));
    links.into_iter().next()
}

/// `threadContextRecord` in a version 1 `ComposerContextClipboardFragment`.
fn thread_chip_fragment(key: &ThreadKey, label: &str) -> String {
    serde_json::json!({
        "version": 1,
        "source": { "environmentId": key.environment.0 },
        "records": [{
            "version": 1,
            "kind": "thread",
            "contextId": crate::view_logic::context_references::kind_scoped_id("thread", &key.thread.0),
            "label": label,
            "environmentId": key.environment.0,
            "threadId": key.thread.0,
            "title": label,
        }],
    })
    .to_string()
}

/// `encodeComposerContextClipboardHtml` without HTML of its own: hosts carry only plain text and
/// HTML, so the fragment travels in the HTML flavour.
fn clipboard_html(text: &str, fragment: &str) -> String {
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        "<pre data-t3-context-fragment=\"{}\">{escaped}</pre>",
        crate::persistence::encode_uri_component(fragment)
    )
}

fn reference_copy_target(
    key: &ThreadKey,
    thread: &OrchestrationV2ThreadShell,
    hosted_app_url: Option<&str>,
) -> ThreadReferenceCopyTarget {
    if let Some(link) = resolve_thread_current_pull_request_link(thread) {
        let url = link.url.0;
        return ThreadReferenceCopyTarget {
            kind: ThreadReferenceKind::PullRequest,
            value: url.clone(),
            description: url,
            fragment: None,
            success_title: "PR link copied",
            failure_title: "Failed to copy PR link",
        };
    }
    let origin = hosted_app_url
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .unwrap_or(HOSTED_APP_URL);
    let url = crate::view_logic::session::markdown_link_presentation::format_hosted_thread_link(
        key, "", origin,
    )
    .or_else(|| {
        crate::view_logic::session::markdown_link_presentation::format_hosted_thread_link(
            key,
            "",
            HOSTED_APP_URL,
        )
    })
    .unwrap_or_default();
    let label = crate::thread_views::user_message::sanitize_label(&thread.title, "thread");
    let fragment = thread_chip_fragment(key, &label);
    ThreadReferenceCopyTarget {
        kind: ThreadReferenceKind::Thread,
        value: url,
        description: label,
        fragment: Some(fragment),
        success_title: "Thread copied",
        failure_title: "Failed to copy thread",
    }
}
fn thread_command(command: &HostCommand) -> bool {
    matches!(
        command,
        HostCommand::FindThread
            | HostCommand::CopyPrompt
            | HostCommand::CopyPromptContext
            | HostCommand::CopyReference
            | HostCommand::LinkPullRequest
            | HostCommand::ShowPullRequests
            | HostCommand::ShareGuest
            | HostCommand::RestartSession
    )
}
fn host_items() -> Vec<(HostCommand, &'static str, &'static str)> {
    use HostCommand::*;
    vec![
        (Triage, "action:triage", "Triage an issue"),
        (FindThread, "find-current-thread", "Find in current thread"),
        (
            FindBrowser,
            "find-in-browser-page",
            "Find on page in browser",
        ),
        (CopyPrompt, "action:copy-prompt-link", "Copy prompt link"),
        (
            CopyPromptContext,
            "action:copy-prompt-link-with-context",
            "Copy prompt link with repo and model",
        ),
        (CopyReference, "action:copy-thread-reference", "Copy thread"),
        (
            LinkPullRequest,
            "action:link-pull-request",
            "Link pull request to thread",
        ),
        (
            ShowPullRequests,
            "action:open-thread-pull-requests",
            "Show linked pull requests",
        ),
        (
            ShareGuest,
            "action:share-thread-with-guest",
            "Share with guest",
        ),
        (
            RestartSession,
            "action:restart-agent-session",
            "Restart agent session",
        ),
        (ChangeTheme, "action:change-theme", "Change theme"),
        (
            ChangeAppearance,
            "action:change-appearance",
            "Change appearance",
        ),
        (ThemeEditor, "action:theme-editor", "Toggle theme editor"),
        (
            UseInAnotherApp,
            "action:use-in-another-app",
            "Use T3 Code in another app (MCP)",
        ),
        (
            FocusNext,
            "action:workspace-focus:1",
            "Focus Next Workspace Area",
        ),
        (
            FocusPrevious,
            "action:workspace-focus:-1",
            "Focus Previous Workspace Area",
        ),
        (PinProject, "action:project-pin", "Pin project"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn shell_with_json(value: serde_json::Value) -> OrchestrationV2ThreadShell {
        serde_json::from_value(value).expect("valid thread shell fixture")
    }

    fn base_thread_json() -> serde_json::Value {
        crate::testing::environment::shell(&[("thr.1", "Alpha")])["threads"][0].clone()
    }

    fn pr_link_json(
        number: u64,
        state: &str,
        head: &str,
        base: &str,
        linked_at: &str,
        updated_at: &str,
        source: &str,
    ) -> serde_json::Value {
        json!({
            "host": "github.com",
            "repository": "acme/web",
            "number": number,
            "url": format!("https://github.com/acme/web/pull/{number}"),
            "source": source,
            "linkedAt": linked_at,
            "snapshot": {
                "state": state,
                "title": format!("PR {number}"),
                "headBranch": head,
                "baseBranch": base,
                "isDraft": false,
                "updatedAt": updated_at,
                "syncedAt": updated_at
            },
            "stack": null
        })
    }

    #[test]
    fn current_pull_request_respects_stack_order_dismissed_and_legacy_fallback() {
        let mut stacked = base_thread_json();
        stacked["pullRequests"] = json!([
            pr_link_json(
                10,
                "open",
                "feat/1",
                "main",
                "2026-04-01T00:00:00.000Z",
                "2026-04-01T01:00:00.000Z",
                "manual"
            ),
            pr_link_json(
                11,
                "open",
                "feat/2",
                "feat/1",
                "2026-04-01T00:00:00.000Z",
                "2026-04-01T00:30:00.000Z",
                "stack"
            ),
            pr_link_json(
                12,
                "open",
                "feat/3",
                "feat/2",
                "2026-04-01T00:00:00.000Z",
                "2026-04-01T00:05:00.000Z",
                "stack-dismissed"
            )
        ]);
        let resolved = resolve_thread_current_pull_request_link(&shell_with_json(stacked))
            .expect("top non-dismissed open stack PR");
        assert_eq!(resolved.number, 11);

        let mut single_open = base_thread_json();
        single_open["pullRequests"] = json!([
            pr_link_json(
                20,
                "merged",
                "feat/a",
                "main",
                "2026-04-01T00:00:00.000Z",
                "2026-04-02T00:00:00.000Z",
                "manual"
            ),
            pr_link_json(
                21,
                "open",
                "feat/b",
                "main",
                "2026-04-01T00:00:00.000Z",
                "2026-04-01T00:00:00.000Z",
                "manual"
            )
        ]);
        assert_eq!(
            resolve_thread_current_pull_request_link(&shell_with_json(single_open))
                .expect("single open PR")
                .number,
            21
        );

        let mut legacy = base_thread_json();
        legacy["branchPullRequest"] = json!({
            "projectId": "proj",
            "repository": "acme/web",
            "number": 33,
            "url": "https://github.com/acme/web/pull/33"
        });
        assert_eq!(
            resolve_thread_current_pull_request_link(&shell_with_json(legacy))
                .expect("branchPullRequest fallback")
                .number,
            33
        );
    }

    #[test]
    fn reference_copy_target_escapes_dots_and_parens_and_uses_configured_origin() {
        let thread = shell_with_json(base_thread_json());
        let key = ThreadKey {
            environment: EnvironmentId("env.1".into()),
            thread: ThreadId("thr.(a)".into()),
        };
        let target = reference_copy_target(&key, &thread, Some("https://web.internal.example/app"));
        assert_eq!(target.kind, ThreadReferenceKind::Thread);
        assert_eq!(
            target.value,
            "https://web.internal.example/threads/env%2E1/thr%2E%28a%29"
        );
    }

    #[test]
    fn thread_reference_carries_a_sanitised_label_and_a_chip_fragment_and_html_escapes_the_link() {
        let mut json = base_thread_json();
        json["title"] = serde_json::json!("[wip]\n  foo\\bar ");
        let thread = shell_with_json(json);
        let key = ThreadKey {
            environment: EnvironmentId("env".into()),
            thread: ThreadId("thr".into()),
        };
        let target = reference_copy_target(&key, &thread, None);
        assert_eq!(target.description, "wip foo bar");
        let fragment: serde_json::Value =
            serde_json::from_str(&target.fragment.expect("a thread copy carries a fragment"))
                .unwrap();
        assert_eq!(fragment["version"], 1);
        assert_eq!(fragment["source"]["environmentId"], "env");
        assert_eq!(
            fragment["records"][0],
            serde_json::json!({
                "version": 1, "kind": "thread", "contextId": "thread_thr",
                "label": "wip foo bar", "environmentId": "env", "threadId": "thr",
                "title": "wip foo bar",
            })
        );
        let html = clipboard_html("https://x.test/a?b=1&c=<2>", "{\"a\":1}");
        assert_eq!(
            html,
            "<pre data-t3-context-fragment=\"%7B%22a%22%3A1%7D\">https://x.test/a?b=1&amp;c=&lt;2&gt;</pre>"
        );
    }
}
