//! The workspace Files panel's client services: the lazy file tree (useDirectoryEntries.ts and
//! FileBrowserPanel.tsx's model), file access (state/filesystem.ts), the panel's workspace target
//! (RightPanelSurfaceContent.tsx's `files`) and file reads (projectFilesQueryState.ts).
pub mod formats;
pub mod preview;
pub mod read;
pub mod tree;

use crate::{
    ClientError,
    composition::ClientRuntime,
    platform::{Platform, StorageArea, StorageKey, StorageWrite},
    rpc::{RpcEndpoint, SessionEndpoint},
    services::filesystem::{ReadAccess, filesystem_read_access},
    types::{EnvironmentId, ThreadKey},
};
use futures::{
    FutureExt, StreamExt,
    channel::mpsc,
    future::{AbortHandle, LocalBoxFuture},
    stream::{FuturesUnordered, LocalBoxStream},
};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::{Rc, Weak},
};
use t3_contracts::{
    auth::AuthEnvironmentScope,
    base_schemas::{TrimmedNonEmptyString, TrimmedString},
    project::{ProjectEntry, ProjectListEntriesInput, ProjectSearchEntriesInput},
    rpc::{WsProjectsListEntriesRpc, WsProjectsSearchEntriesRpc},
};
use tree::{FileTreeView, Problem, Reveal, TreeModel};

pub const NOT_CONNECTED: &str = "This environment is not connected.";
pub const CANNOT_READ_HOST_FILES: &str = "This connection cannot read host files.";
pub const CANNOT_SEARCH_HOST_FILES: &str = "This connection cannot search host files.";

/// The Files panel's workspace: the thread's worktree, else its project's root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesWorkspace {
    pub cwd: String,
    pub project_name: String,
}

#[derive(Default)]
pub(crate) struct FilesState {
    trees: RefCell<BTreeMap<(EnvironmentId, String), Weak<TreeShared>>>,
    reads: read::ReadState,
}

#[derive(Clone)]
pub struct ProjectFiles<P: Platform + Clone + 'static> {
    pub(crate) runtime: ClientRuntime<P>,
    pub(crate) platform: P,
    pub(crate) state: Rc<FilesState>,
}

pub(crate) fn problem(error: &ClientError, fallback: &str) -> Problem {
    match error {
        ClientError::Rejected { message, .. } => Problem::server(message.clone()),
        _ => Problem::client(fallback),
    }
}

impl<P: Platform + Clone + 'static> ProjectFiles<P> {
    pub(crate) fn new(runtime: ClientRuntime<P>, platform: P, state: Rc<FilesState>) -> Self {
        Self {
            runtime,
            platform,
            state,
        }
    }

    pub(crate) fn endpoint(&self, environment: &EnvironmentId) -> Option<SessionEndpoint> {
        self.runtime.client(environment).and_then(|c| c.session())
    }

    /// `useEnvironmentScope(AuthFilesystemWriteScope)`: whether this session may write files.
    pub fn can_write(&self, environment: &EnvironmentId) -> bool {
        self.endpoint(environment)
            .and_then(|endpoint| endpoint.grants_scope(AuthEnvironmentScope::FilesystemWrite))
            .unwrap_or(false)
    }

    /// useFilesystemReadAccess: the session's `filesystem:read` grant once it is known, else
    /// pending while connecting, else why the environment cannot be read.
    pub fn read_access(&self, environment: &EnvironmentId) -> ReadAccess {
        if let Some(granted) = self
            .endpoint(environment)
            .and_then(|endpoint| endpoint.grants_scope(AuthEnvironmentScope::FilesystemRead))
        {
            return ReadAccess {
                allowed: granted,
                pending: false,
                error: None,
            };
        }
        let phase = self
            .runtime
            .environments()
            .into_iter()
            .find(|entry| &entry.id == environment)
            .map(|entry| match entry.state.phase {
                crate::connection::Phase::Connected => "connected",
                crate::connection::Phase::Connecting => "connecting",
                crate::connection::Phase::Backoff => "reconnecting",
                _ => "disconnected",
            });
        filesystem_read_access(true, phase, None, None, None)
    }

    /// Store, connection and grant changes of `environment`, for screens that re-read access
    /// and the workspace target.
    pub fn changes(&self, environment: &EnvironmentId) -> LocalBoxStream<'static, ()> {
        futures::stream::select(
            self.runtime
                .store()
                .subscribe_environment(environment)
                .map(|_| ()),
            self.runtime.changes(),
        )
        .boxed_local()
    }

    pub fn workspace(&self, key: &ThreadKey) -> Option<FilesWorkspace> {
        let shell = self.runtime.store().shared_shell(&key.environment)?;
        let thread = shell.threads.iter().find(|t| t.id == key.thread)?;
        let project = shell.projects.iter().find(|p| p.id == thread.project_id)?;
        Some(FilesWorkspace {
            cwd: thread
                .worktree_path
                .as_ref()
                .map_or_else(|| project.workspace_root.0.clone(), |path| path.0.clone()),
            project_name: project.title.0.clone(),
        })
    }

    /// `assets.createUrl` for `resource`, resolved against the environment's HTTP origin
    /// (useAssetUrlState). Media and documents load their bytes from this signed URL.
    pub fn asset_url(
        &self,
        environment: &EnvironmentId,
        resource: t3_contracts::assets::AssetResource,
    ) -> LocalBoxFuture<'static, Result<String, Problem>> {
        let endpoint = self.endpoint(environment);
        let base = self
            .runtime
            .environments()
            .into_iter()
            .find(|entry| &entry.id == environment)
            .and_then(|entry| {
                entry
                    .routes
                    .into_iter()
                    .map(|route| route.http_base)
                    .find(|base| !base.is_empty())
            });
        async move {
            let endpoint = endpoint.ok_or_else(|| Problem::client(NOT_CONNECTED))?;
            let asset = endpoint
                .call::<t3_contracts::rpc::WsAssetsCreateUrlRpc>(
                    t3_contracts::assets::AssetCreateUrlInput { resource },
                )
                .await
                .map_err(|error| match error {
                    ClientError::Rejected { message, .. } => Problem::server(message),
                    error => Problem::server(error.to_string()),
                })?;
            let base = base.ok_or_else(|| Problem::client(NOT_CONNECTED))?;
            crate::services::assets::resolve_asset_url(&base, &asset.relative_url.0)
                .ok_or_else(|| Problem::server(asset.relative_url.0.clone()))
        }
        .boxed_local()
    }

    /// A web `localStorage` preference of the panel (explorer width and visibility, rendered
    /// modes), kept in the client's local preferences under the web's own key.
    pub fn preference(&self, key: &'static str) -> LocalBoxFuture<'static, Option<serde_json::Value>> {
        let platform = self.platform.clone();
        async move {
            let bytes = platform
                .get(StorageArea::Ordinary, StorageKey::new("local-preferences", None, key))
                .await
                .ok()
                .flatten()?;
            serde_json::from_slice(&bytes).ok()
        }
        .boxed_local()
    }
    pub fn set_preference(&self, key: &'static str, value: serde_json::Value) {
        let platform = self.platform.clone();
        let _ = self.runtime.spawn(
            async move {
                let Ok(bytes) = serde_json::to_vec(&value) else { return };
                let _ = platform
                    .write(
                        StorageArea::Ordinary,
                        vec![StorageWrite {
                            key: StorageKey::new("local-preferences", None, key),
                            value: Some(bytes),
                        }],
                    )
                    .await;
            }
            .boxed_local(),
        );
    }

    /// ChatView's `workspaceMutationId`: the token the panel refreshes on.
    pub fn workspace_mutation(&self, key: &ThreadKey) -> Option<String> {
        self.runtime.store().shared_thread(key).and_then(|p| {
            let checkpoint = p
                .checkpoints
                .iter()
                .rev()
                .find(|c| c.app_run_ordinal.is_some() && c.run_id.is_some());
            crate::thread_views::threads::detail::derive_workspace_mutation_id(
                &p.visible_turn_items,
                checkpoint.map(|c| c.captured_at.0.as_str()),
            )
            .ok()
            .flatten()
        })
    }

    /// The tree of `cwd` in `environment`, shared by every panel showing it. `host_folder` trees
    /// list a folder outside the workspace: no expand-all and no server search.
    pub fn tree(&self, environment: &EnvironmentId, cwd: &str, host_folder: bool) -> FileTree {
        let key = (environment.clone(), cwd.to_owned());
        if let Some(shared) = self.state.trees.borrow().get(&key).and_then(Weak::upgrade) {
            return FileTree { shared };
        }
        let (kick, kicks) = mpsc::unbounded();
        let shared = Rc::new(TreeShared {
            model: RefCell::new(TreeModel::new(vec![])),
            listeners: RefCell::default(),
            kick,
            task: Cell::new(None),
            search: Cell::new(None),
            refresh_search: Cell::new(false),
            host_folder,
        });
        let task = self.runtime.spawn(
            drive_tree(
                self.clone(),
                Rc::downgrade(&shared),
                environment.clone(),
                cwd.to_owned(),
                kicks,
            )
            .boxed_local(),
        );
        shared.task.set(Some(task));
        let mut trees = self.state.trees.borrow_mut();
        trees.retain(|_, tree| tree.strong_count() > 0);
        trees.insert(key, Rc::downgrade(&shared));
        FileTree { shared }
    }
}

struct TreeShared {
    model: RefCell<TreeModel>,
    listeners: RefCell<Vec<mpsc::UnboundedSender<()>>>,
    kick: mpsc::UnboundedSender<()>,
    task: Cell<Option<AbortHandle>>,
    search: Cell<Option<u64>>,
    refresh_search: Cell<bool>,
    host_folder: bool,
}
impl Drop for TreeShared {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
impl TreeShared {
    fn notify(&self) {
        self.listeners
            .borrow_mut()
            .retain(|listener| listener.unbounded_send(()).is_ok());
    }
    fn changed(&self) {
        let _ = self.kick.unbounded_send(());
        self.notify();
    }
}

/// A screen's hold on a tree; the last one dropped cancels its reads.
#[derive(Clone)]
pub struct FileTree {
    shared: Rc<TreeShared>,
}

impl FileTree {
    pub fn view(&self) -> FileTreeView {
        self.shared.model.borrow().view()
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        let (sender, receiver) = mpsc::unbounded();
        self.shared.listeners.borrow_mut().push(sender);
        receiver.boxed_local()
    }
    pub fn host_folder(&self) -> bool {
        self.shared.host_folder
    }
    pub fn set_expanded(&self, ids: &[impl AsRef<str>]) {
        self.shared.model.borrow_mut().set_expanded(ids);
        self.shared.changed();
    }
    pub fn toggle_all(&self) {
        self.shared.model.borrow_mut().toggle_all();
        self.shared.changed();
    }
    /// The refresh button: folders, and the search when one is open.
    pub fn refresh(&self) {
        let searching = {
            let mut model = self.shared.model.borrow_mut();
            model.refresh();
            !model.query().trim().is_empty()
        };
        if searching {
            self.shared.refresh_search.set(true);
        }
        self.shared.changed();
    }
    pub fn set_query(&self, query: String) {
        let generation = self
            .shared
            .model
            .borrow_mut()
            .set_query(query, !self.shared.host_folder);
        self.shared.search.set(generation);
        self.shared.changed();
    }
    pub fn query(&self) -> String {
        self.shared.model.borrow().query().to_owned()
    }
    /// A selection made in the tree. Returns whether it is a file the panel opens.
    pub fn select(&self, path: &str) -> bool {
        let mut model = self.shared.model.borrow_mut();
        let file = model.is_file(path);
        if file {
            model.select_from_tree(path);
        }
        file
    }
    pub fn load_ancestors(&self, path: &str) {
        self.shared.model.borrow_mut().load_ancestors(path);
        self.shared.changed();
    }
    pub fn reveal(&self, path: Option<&str>, reveal_id: u64) -> Option<Reveal> {
        let reveal = self.shared.model.borrow_mut().reveal(path, reveal_id);
        if reveal.is_some() {
            self.shared.search.set(None);
            self.shared.changed();
        }
        reveal
    }
    pub fn observe_mutation(&self, mutation: Option<&str>) {
        if self.shared.model.borrow_mut().observe_mutation(mutation) {
            self.shared.refresh_search.set(true);
        }
        self.shared.changed();
    }
    /// Reads started since the tree was created, for tests that count work.
    pub fn lists_started(&self) -> u64 {
        self.shared.model.borrow().lists_started
    }
}

enum Done {
    List(String, Result<Vec<ProjectEntry>, Problem>),
    Debounce(u64),
    Search(u64, Result<(Vec<ProjectEntry>, bool), Problem>),
    Idle,
}

fn storage_key() -> StorageKey {
    StorageKey::new(
        "local-preferences",
        None,
        tree::EXPANDED_DIRECTORIES_STORAGE_KEY,
    )
}

async fn drive_tree<P: Platform + Clone + 'static>(
    files: ProjectFiles<P>,
    weak: Weak<TreeShared>,
    environment: EnvironmentId,
    cwd: String,
    mut kicks: mpsc::UnboundedReceiver<()>,
) {
    let root = format!("{}:{cwd}", environment.0);
    let stored = files
        .platform
        .get(StorageArea::Ordinary, storage_key())
        .await
        .ok()
        .flatten();
    let restored = tree::read_expanded_directories(stored.as_deref(), &root);
    let Some(shared) = weak.upgrade() else {
        return;
    };
    shared.model.borrow_mut().restore(restored);
    let mut persisted = shared.model.borrow().expansion_revision;
    drop(shared);
    let mut sessions = files.runtime.store().subscribe_environment(&environment).fuse();
    let mut connected = files.endpoint(&environment).is_some();
    let mut session = session_generation(&files, &environment);
    let mut calls: FuturesUnordered<LocalBoxFuture<'static, Done>> = FuturesUnordered::new();
    loop {
        let Some(shared) = weak.upgrade() else {
            return;
        };
        let endpoint = files.endpoint(&environment);
        loop {
            let next = shared.model.borrow_mut().next_start();
            let Some(directory) = next else {
                break;
            };
            calls.push(list(endpoint.clone(), cwd.clone(), directory).boxed_local());
        }
        if shared.refresh_search.take() {
            let current = shared.model.borrow().search_generation();
            calls.push(search(&files, &environment, endpoint.clone(), &cwd, &shared, current));
        }
        if let Some(search_generation) = shared.search.take() {
            let platform = files.platform.clone();
            calls.push(
                async move {
                    platform.sleep(tree::SEARCH_DEBOUNCE_MS).await;
                    Done::Debounce(search_generation)
                }
                .boxed_local(),
            );
        }
        let revision = shared.model.borrow().expansion_revision;
        if revision != persisted {
            persisted = revision;
            let paths = shared.model.borrow().persisted_expansion();
            let platform = files.platform.clone();
            let root = root.clone();
            calls.push(
                async move {
                    let stored = platform
                        .get(StorageArea::Ordinary, storage_key())
                        .await
                        .ok()
                        .flatten();
                    let value = tree::write_expanded_directories(stored.as_deref(), &root, paths);
                    let _ = platform
                        .write(
                            StorageArea::Ordinary,
                            vec![StorageWrite {
                                key: storage_key(),
                                value: Some(value),
                            }],
                        )
                        .await;
                    Done::Idle
                }
                .boxed_local(),
            );
        }
        shared.notify();
        drop(shared);
        futures::select! {
            kick = kicks.next() => {
                if kick.is_none() {
                    return;
                }
            }
            done = calls.select_next_some() => {
                let Some(shared) = weak.upgrade() else { return };
                match done {
                    Done::List(directory, result) => {
                        shared.model.borrow_mut().complete(&directory, result);
                    }
                    Done::Debounce(search_generation) => {
                        if shared.model.borrow().search_input(search_generation).is_some() {
                            let endpoint = files.endpoint(&environment);
                            calls.push(search(&files, &environment, endpoint, &cwd, &shared, search_generation));
                        }
                    }
                    Done::Search(search_generation, result) => {
                        shared.model.borrow_mut().complete_search(search_generation, result);
                    }
                    Done::Idle => {}
                }
            }
            _ = sessions.next() => {
                let next = session_generation(&files, &environment);
                let reachable = files.endpoint(&environment).is_some();
                if next != session || reachable != connected {
                    session = next;
                    connected = reachable;
                    if let Some(shared) = weak.upgrade() {
                        let searching = {
                            let mut model = shared.model.borrow_mut();
                            model.retry_errors();
                            !model.query().trim().is_empty()
                        };
                        if searching && !shared.host_folder {
                            shared.refresh_search.set(true);
                        }
                    }
                }
            }
        }
    }
}

fn session_generation<P: Platform + Clone + 'static>(
    files: &ProjectFiles<P>,
    environment: &EnvironmentId,
) -> Option<u64> {
    files
        .runtime
        .store()
        .snapshot()
        .environments
        .get(environment)
        .map(|e| e.session.0)
}

async fn list(endpoint: Option<SessionEndpoint>, cwd: String, directory: String) -> Done {
    let result = match endpoint {
        None => Err(Problem::client(NOT_CONNECTED)),
        Some(endpoint) => endpoint
            .call::<WsProjectsListEntriesRpc>(ProjectListEntriesInput {
                cwd: TrimmedNonEmptyString(cwd),
                directory_path: Some(Some(TrimmedString(directory.clone()))),
            })
            .await
            .map(|result| result.entries)
            .map_err(|error| problem(&error, tree::UNABLE_TO_LOAD_FOLDER)),
    };
    Done::List(directory, result)
}

fn search<P: Platform + Clone + 'static>(
    files: &ProjectFiles<P>,
    environment: &EnvironmentId,
    endpoint: Option<SessionEndpoint>,
    cwd: &str,
    shared: &TreeShared,
    generation: u64,
) -> LocalBoxFuture<'static, Done> {
    let access = files.read_access(environment);
    let query = shared.model.borrow().search_input(generation);
    let cwd = cwd.to_owned();
    async move {
        let Some(query) = query else {
            return Done::Idle;
        };
        if !access.allowed {
            if access.pending {
                return Done::Idle;
            }
            let problem = access
                .error
                .map_or_else(|| Problem::client(CANNOT_SEARCH_HOST_FILES), |text| Problem::client(&text));
            return Done::Search(generation, Err(problem));
        }
        let result = match endpoint {
            None => Err(Problem::client(NOT_CONNECTED)),
            Some(endpoint) => endpoint
                .call::<WsProjectsSearchEntriesRpc>(ProjectSearchEntriesInput {
                    cwd: TrimmedNonEmptyString(cwd),
                    query: TrimmedString(query),
                    limit: tree::SEARCH_LIMIT.into(),
                    kind: None,
                    image_only: None,
                })
                .await
                .map(|result| (result.entries, result.truncated))
                .map_err(|error| match error {
                    ClientError::Rejected { message, .. } => Problem::server(message),
                    error => Problem::server(error.to_string()),
                }),
        };
        Done::Search(generation, result)
    }
    .boxed_local()
}
