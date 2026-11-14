//! CommandPalette.tsx's shared catalogue, target resolution and action dispatch.
use crate::view_logic::shell::palette_files::{PaletteSearchClient, SearchTarget};
use crate::{
    ClientError,
    composition::ClientRuntime,
    navigation::{Route, SettingsPage},
    platform::Platform,
    services::resources::{Resource, ResourcePolicy},
    types::{EnvironmentId, Generation, ProjectId, ThreadId, ThreadKey},
    view_logic::shell::{
        palette::{self, FilterInput, Group, Item},
        search, sidebar,
    },
    workspace::ProjectKey,
};
use futures::{StreamExt, stream::LocalBoxStream};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
#[path = "palette_queries.rs"]
mod queries;
type ContentCache = Option<(SearchTarget, Rc<t3_contracts::project::ProjectSearchContentsResult>)>;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HostCommand {
    FindThread,
    FindBrowser,
    CopyPrompt,
    CopyPromptContext,
    CopyReference,
    LinkPullRequest,
    LinkedPullRequests,
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
            message_targets: Rc::default(),
            content_cache: Rc::default(),
        }
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
    pub fn catalogue(&self) -> Catalogue {
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
        if self.runtime.environments().iter().any(|e| {
            e.state.phase == crate::connection::Phase::Connected
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
                "go to file open file file picker find file quick open",
                Some("filePicker.toggle"),
            ),
            (
                "action:search-project-contents",
                "Search project contents",
                "search project find in files grep content search text search",
                Some("projectSearch.toggle"),
            ),
            (
                "action:add-project",
                "Add project",
                "add project folder directory browse clone remote repository repo git url environment github gitlab gitcafe forgejo bitbucket azure",
                None,
            ),
            (
                "action:usage",
                "Open usage",
                "usage use tokens cost spend limits stats analytics",
                Some("usage.open"),
            ),
            (
                "action:settings",
                "Open settings",
                "settings preferences configuration keybindings",
                None,
            ),
        ] {
            let mut item = Item::action(value, title);
            item.search_terms = terms.split(' ').map(str::to_owned).collect();
            item.shortcut = shortcut.map(str::to_owned);
            item.keep_open = matches!(
                value,
                "action:open-file-picker" | "action:search-project-contents" | "action:add-project"
            );
            actions.push(item);
        }
        if self
            .runtime
            .environments()
            .iter()
            .any(|e| e.config.is_some())
        {
            actions.push(Item::action("action:pull-requests", "Open pull requests"));
        }
        if self.runtime.environments().iter().any(|e| {
            e.state.phase == crate::connection::Phase::Connected
                && e.config
                    .as_ref()
                    .is_some_and(|c| c.new_projects_root.is_some())
        }) {
            actions.push(Item::action("action:new-project", "New project"));
        }
        for (command, value, title) in host_items() {
            if !self.host.borrow().contains(&command) {
                continue;
            }
            if thread_command(&command) && self.key().is_none() {
                continue;
            }
            if matches!(command, HostCommand::PinProject) && self.active_project().is_none() {
                continue;
            }
            actions.push(Item::action(value, title));
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
            &|date| crate::time::epoch_ms(date).map(|d| d as i64),
        )
    }
    pub fn filter(&self, query: &str, active: Option<&[Group]>, messages: &[Item]) -> Vec<Group> {
        let catalogue = self.catalogue();
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
            threads: &catalogue.threads,
            messages,
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
            let id = self
                .runtime
                .new_threads()
                .open_project(key.clone(), None, None)
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
                    project: self.active_project(),
                }
            }
        })
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
            | HostCommand::LinkedPullRequests
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
            LinkedPullRequests,
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
