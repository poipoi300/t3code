use crate::{
    api::{ProjectionReader, ProjectionRecordSet, RecordFilter, RecordKind},
    error::{Error, Result},
    stores::{StoreDatabase, projects},
};
use std::{
    collections::HashSet,
    ffi::OsString,
    path::{Component, Path, PathBuf},
};
use t3_contracts::{orchestration_v2::*, orchestration_v2_id_table::IdOrIndex};
use t3_workspace::restore::RestoreFolder;

pub const SHARED_WORKSPACE_RESTORE_MESSAGE: &str = "File restore requires an isolated worktree. This workspace may contain changes from another thread or a side chat. Rewind the conversation without restoring files instead, or delete side chats you no longer need.";

pub fn expanded_id<T: Clone>(id: &IdOrIndex<T>) -> Result<T> {
    match id {
        IdOrIndex::Id(id) => Ok(id.clone()),
        IdOrIndex::Index(_) => Err(Error::Decode {
            entity: "checkpoint ID",
            detail: "compact IDs must be expanded before service use".into(),
        }),
    }
}

// Canonicalizing the existing ancestor keeps missing Windows paths in the same namespace.
pub fn canonicalize_missing(path: &Path) -> std::io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut ancestor = absolute.as_path();
    let mut suffix: Vec<OsString> = Vec::new();
    let mut resolved = loop {
        match std::fs::canonicalize(ancestor) {
            Ok(path) => break path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if std::fs::symlink_metadata(ancestor).is_ok() {
                    return Err(error);
                }
                let Some(name) = ancestor.file_name() else {
                    return Err(error);
                };
                suffix.push(name.to_owned());
                let Some(parent) = ancestor.parent() else {
                    return Err(error);
                };
                ancestor = parent;
            }
            Err(error) => return Err(error),
        }
    };
    for part in suffix.into_iter().rev() {
        for component in Path::new(&part).components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    resolved.pop();
                }
                _ => resolved.push(component),
            }
        }
    }
    Ok(resolved)
}

/// The current target and the future named location of a linked checkout.
pub(super) fn workspace_keys(cwd: &Path) -> std::io::Result<Vec<PathBuf>> {
    let target = canonicalize_missing(cwd)?;
    let mut keys = vec![target];
    if let Some((parent, name)) = cwd.parent().zip(cwd.file_name()) {
        keys.push(canonicalize_missing(parent)?.join(name));
        keys.sort();
        keys.dedup();
    }
    Ok(keys)
}

pub fn paths_overlap(a: &Path, b: &Path) -> bool {
    RestoreFolder::from_resolved(a.to_owned()).overlaps(&RestoreFolder::from_resolved(b.to_owned()))
}

pub fn is_managed_worktree_path(path: &Path, roots: &[PathBuf]) -> Result<bool> {
    if !path.is_absolute()
        || std::path::absolute(path)?.as_os_str() != path.as_os_str()
        || path
            .components()
            .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
    {
        return Ok(false);
    }
    let candidate = canonicalize_missing(path)?;
    for root in roots {
        let root = canonicalize_missing(root)?;
        if root.parent().is_none() {
            continue;
        }
        if let Ok(relative) = candidate.strip_prefix(root) {
            let parts: Vec<_> = relative.components().collect();
            if parts.len() == 2 && parts.iter().all(|c| matches!(c, Component::Normal(_))) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub async fn is_checkpoint_restore_isolated(
    thread: &OrchestrationV2AppThread,
    scope: &OrchestrationV2CheckpointScope,
    projections: &dyn ProjectionReader,
    database: &StoreDatabase,
) -> Result<bool> {
    let Some(worktree) = &thread.worktree_path else {
        return Ok(false);
    };
    let cwd = canonicalize_missing(Path::new(scope.cwd.0.as_str()))?;
    if cwd != canonicalize_missing(Path::new(worktree.0.as_str()))? {
        return Ok(false);
    }
    let current_id = expanded_id(&thread.id)?;
    let shell = projections.shell_snapshot(None, None).await?;
    let mut checked = HashSet::new();
    for other in shell.threads.into_iter().chain(shell.archived_threads) {
        if other.id == current_id || other.deleted_at.is_some() {
            continue;
        }
        if other.lineage.side_chat == Some(Some(true))
            && other.active_run_id.is_none()
            && other.worktree_path.as_ref() == Some(worktree)
        {
            let records = projections
                .records(
                    other.id.clone(),
                    vec![RecordKind::TurnItems],
                    RecordFilter {
                        turn_item_types: Some(vec![
                            "command_execution".into(),
                            "file_change".into(),
                            "dynamic_tool".into(),
                        ]),
                        ..Default::default()
                    },
                )
                .await?;
            if records
                .records
                .iter()
                .any(|set| matches!(set, ProjectionRecordSet::TurnItems(items) if items.is_empty()))
            {
                continue;
            }
        }
        let records = projections
            .records(
                other.id.clone(),
                vec![RecordKind::CheckpointScopes, RecordKind::ProviderSessions],
                RecordFilter::default(),
            )
            .await?;
        let mut paths: Vec<String> = other.worktree_path.iter().map(|p| p.0.clone()).collect();
        for set in records.records {
            match set {
                ProjectionRecordSet::CheckpointScopes(scopes) => {
                    paths.extend(scopes.into_iter().map(|s| s.cwd.0))
                }
                ProjectionRecordSet::ProviderSessions(sessions) => paths.extend(
                    sessions
                        .into_iter()
                        .filter(|s| {
                            s.status != ProviderSessionStatus::Stopped
                                && !s
                                    .capabilities
                                    .sessions
                                    .supports_multiple_provider_threads_per_session
                        })
                        .map(|s| s.cwd.0),
                ),
                _ => {}
            }
        }
        if other.worktree_path.is_none() {
            let project = database
                .read(move |db| projects::get(db, &other.project_id, true))
                .await?;
            let Some(project) = project else {
                return Ok(false);
            };
            paths.push(project.workspace_root);
        }
        for path in paths {
            if !checked.insert(path.clone()) {
                continue;
            }
            // A removed checkout retains its ownership until the owning thread or scope is removed.
            let other_cwd = canonicalize_missing(Path::new(&path))?;
            if paths_overlap(&cwd, &other_cwd) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

pub fn ensure_checkpoint_paths_safe(root: &Path, paths: &[PathBuf]) -> Result<()> {
    let mut checked = HashSet::new();
    for relative in std::iter::once(Path::new("")).chain(paths.iter().map(PathBuf::as_path)) {
        let mut path = root.to_path_buf();
        let mut candidates = vec![path.clone()];
        for component in relative.components() {
            if !matches!(component, Component::Normal(_) | Component::CurDir) {
                return Err(Error::Service {
                    operation: "checkpoint.restore-safety",
                    detail: "Git returned a path outside the checkpoint scope".into(),
                });
            }
            path.push(component);
            candidates.push(path.clone());
        }
        for path in candidates {
            if !checked.insert(path.clone()) {
                continue;
            }
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => return Err(error.into()),
            };
            #[cfg(windows)]
            let linked = {
                use std::os::windows::fs::MetadataExt;
                metadata.file_attributes() & 0x400 != 0
            };
            #[cfg(not(windows))]
            let linked = metadata.file_type().is_symlink();
            if linked {
                return Err(Error::Service {
                    operation: "checkpoint.restore-safety",
                    detail: format!(
                        "Refusing checkpoint filesystem access through a link or reparse point: {}",
                        path.display()
                    ),
                });
            }
            if !metadata.is_dir() {
                break;
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReservationKind {
    Restore,
    TurnStart,
}

#[derive(Default)]
struct Reservations {
    next: u64,
    held: Vec<(u64, RestoreFolder, ReservationKind)>,
    waiting: Vec<(u64, RestoreFolder)>,
}

pub struct WorkspaceReservations {
    state: std::sync::Mutex<Reservations>,
    released: tokio::sync::watch::Sender<u64>,
    waiters: tokio::sync::watch::Sender<u64>,
}

impl Default for WorkspaceReservations {
    fn default() -> Self {
        Self {
            state: std::sync::Mutex::new(Reservations::default()),
            released: tokio::sync::watch::channel(0).0,
            waiters: tokio::sync::watch::channel(0).0,
        }
    }
}

/// Marks one blocked `reserve` call while it waits.
struct Waiting<'a> {
    owner: &'a WorkspaceReservations,
    id: u64,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        let mut state = self.owner.state.lock().unwrap_or_else(|e| e.into_inner());
        state.waiting.retain(|(id, _)| *id != self.id);
        self.owner
            .waiters
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }
}

pub struct WorkspaceReservation {
    owner: std::sync::Arc<WorkspaceReservations>,
    id: u64,
}

impl WorkspaceReservations {
    pub fn global() -> std::sync::Arc<Self> {
        static RESERVATIONS: std::sync::OnceLock<std::sync::Arc<WorkspaceReservations>> =
            std::sync::OnceLock::new();
        RESERVATIONS
            .get_or_init(|| std::sync::Arc::new(Self::default()))
            .clone()
    }

    /// Maintenance skips an occupied checkout instead of queuing a deletion behind its writer.
    pub fn try_reserve_cleanup(
        self: &std::sync::Arc<Self>,
        cwd: &Path,
    ) -> Result<Option<WorkspaceReservation>> {
        let keys = workspace_keys(cwd)?
            .into_iter()
            .map(RestoreFolder::from_resolved)
            .collect::<Vec<_>>();
        self.claim(
            &mut self.state.lock().unwrap_or_else(|e| e.into_inner()),
            &keys,
            ReservationKind::Restore,
        )
    }

    fn claim(
        self: &std::sync::Arc<Self>,
        state: &mut Reservations,
        keys: &[RestoreFolder],
        kind: ReservationKind,
    ) -> Result<Option<WorkspaceReservation>> {
        if state.held.iter().any(|(_, path, held_kind)| {
            (kind == ReservationKind::Restore || *held_kind == ReservationKind::Restore)
                && keys.iter().any(|key| path.overlaps(key))
        }) {
            return Ok(None);
        }
        let id = state.next;
        state.next = state.next.checked_add(1).ok_or_else(|| Error::Service {
            operation: "workspace.reserve",
            detail: "Reservation identity exhausted".into(),
        })?;
        state
            .held
            .extend(keys.iter().cloned().map(|key| (id, key, kind)));
        Ok(Some(WorkspaceReservation {
            owner: self.clone(),
            id,
        }))
    }

    pub async fn reserve(
        self: &std::sync::Arc<Self>,
        cwd: &Path,
        kind: ReservationKind,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> Result<WorkspaceReservation> {
        let mut released = self.released.subscribe();
        let mut waiting = None;
        loop {
            if cancellation.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let keys: Vec<_> = workspace_keys(cwd)?
                .into_iter()
                .map(RestoreFolder::from_resolved)
                .collect();
            {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(guard) = self.claim(&mut state, &keys, kind)? {
                    return Ok(guard);
                }
                if waiting.is_none() {
                    let id = state.next;
                    state.next = state.next.checked_add(1).ok_or_else(|| Error::Service {
                        operation: "workspace.reserve",
                        detail: "Reservation identity exhausted".into(),
                    })?;
                    state
                        .waiting
                        .extend(keys.iter().cloned().map(|key| (id, key)));
                    waiting = Some(Waiting { owner: self, id });
                    self.waiters
                        .send_modify(|generation| *generation = generation.wrapping_add(1));
                }
            }
            tokio::select! {
                _ = cancellation.cancelled() => return Err(Error::Cancelled),
                result = released.changed() => { result.map_err(|error| Error::Service { operation: "workspace.reserve", detail: error.to_string() })?; },
            }
            drop(waiting.take());
        }
    }
}

impl WorkspaceReservations {
    /// Resolves once a reservation of a folder overlapping `cwd` is waiting: a test's proof that
    /// a turn start is held off rather than not yet reached.
    #[cfg(any(test, feature = "testkit"))]
    pub async fn waiting_on(&self, cwd: &Path) -> Result<()> {
        let canonical = RestoreFolder::from_resolved(canonicalize_missing(cwd)?);
        let mut changed = self.waiters.subscribe();
        loop {
            if self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .waiting
                .iter()
                .any(|(_, path)| path.overlaps(&canonical))
            {
                return Ok(());
            }
            changed.changed().await.map_err(|error| Error::Service {
                operation: "workspace.reserve",
                detail: error.to_string(),
            })?;
        }
    }
}

impl Drop for WorkspaceReservation {
    fn drop(&mut self) {
        let mut state = self.owner.state.lock().unwrap_or_else(|e| e.into_inner());
        state.held.retain(|(id, _, _)| *id != self.id);
        self.owner
            .released
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }
}

/// Shared by provider startup, worktree cleanup and checkpoint rollback.
#[derive(Clone)]
pub struct WorkspaceCoordination {
    reservations: std::sync::Arc<WorkspaceReservations>,
    leases: std::sync::Arc<super::checkpoint_service::WorkspaceLocks>,
}

impl WorkspaceCoordination {
    pub fn new(
        reservations: std::sync::Arc<WorkspaceReservations>,
        leases: std::sync::Arc<super::checkpoint_service::WorkspaceLocks>,
    ) -> Self {
        Self {
            reservations,
            leases,
        }
    }

    pub fn global() -> Self {
        Self::new(
            WorkspaceReservations::global(),
            super::checkpoint_service::WorkspaceLocks::workspace_leases(),
        )
    }

    /// The future performs startup and returns its execution handle; never await turn settlement here.
    pub async fn start<T>(
        &self,
        cwd: Option<&Path>,
        worktree: Option<&Path>,
        cancellation: &tokio_util::sync::CancellationToken,
        startup: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        let _reservation = match cwd.or(worktree) {
            Some(cwd) => Some(
                self.reservations
                    .reserve(cwd, ReservationKind::TurnStart, cancellation)
                    .await?,
            ),
            None => None,
        };
        let _lease = match worktree {
            Some(cwd) => Some(tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(Error::Cancelled),
                lease = self.leases.acquire_for_recovery(cwd) => lease?,
            }),
            None => None,
        };
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        startup.await
    }

    pub async fn cleanup<T>(
        &self,
        cwd: &Path,
        cancellation: &tokio_util::sync::CancellationToken,
        cleanup: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        let _lease = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(Error::Cancelled),
            lease = self.leases.acquire(cwd) => lease?,
        };
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        cleanup.await
    }
}

pub struct CoordinatedWorkspaces {
    backend: std::sync::Arc<dyn crate::api::WorkspaceBackend>,
    coordination: WorkspaceCoordination,
}

impl CoordinatedWorkspaces {
    pub fn new(
        backend: std::sync::Arc<dyn crate::api::WorkspaceBackend>,
        coordination: WorkspaceCoordination,
    ) -> Self {
        Self {
            backend,
            coordination,
        }
    }
}

#[async_trait::async_trait]
impl crate::api::WorkspaceBackend for CoordinatedWorkspaces {
    async fn prepare(
        &self,
        project: crate::api::ProjectId,
        thread: crate::api::ThreadId,
        strategy: OrchestrationV2ThreadLaunchWorkspaceStrategy,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<crate::api::Workspace> {
        self.backend
            .prepare(project, thread, strategy, cancellation)
            .await
    }
    async fn remove(
        &self,
        workspace: crate::api::Workspace,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        let cwd = workspace
            .worktree_path
            .as_ref()
            .unwrap_or(&workspace.cwd)
            .clone();
        self.coordination
            .cleanup(
                &cwd,
                &cancellation,
                self.backend.remove(workspace, cancellation.clone()),
            )
            .await
    }
    async fn current_branch(&self, cwd: PathBuf) -> Result<Option<String>> {
        self.backend.current_branch(cwd).await
    }
    async fn repository_identity(
        &self,
        cwd: PathBuf,
    ) -> Result<Option<t3_contracts::environment::RepositoryIdentity>> {
        self.backend.repository_identity(cwd).await
    }
    async fn default_branch(&self, cwd: PathBuf) -> Result<Option<String>> {
        self.backend.default_branch(cwd).await
    }
    async fn checked_out_pull_request(
        &self,
        cwd: PathBuf,
        branch: String,
        url: String,
    ) -> Result<bool> {
        self.backend
            .checked_out_pull_request(cwd, branch, url)
            .await
    }
}

pub struct CoordinatedTurnStart {
    inner: crate::ports::runs_ingestion::provider_turn_start_service::ProviderTurnStartService,
    projections: std::sync::Arc<dyn ProjectionReader>,
    database: StoreDatabase,
    coordination: WorkspaceCoordination,
}

impl CoordinatedTurnStart {
    pub fn new(
        inner: crate::ports::runs_ingestion::provider_turn_start_service::ProviderTurnStartService,
        projections: std::sync::Arc<dyn ProjectionReader>,
        database: StoreDatabase,
        coordination: WorkspaceCoordination,
    ) -> Self {
        Self {
            inner,
            projections,
            database,
            coordination,
        }
    }

    /// Provider startup under the workspace reservation; the policy is resolved, and a ceiling
    /// refusal settled, by the turn start itself.
    pub async fn start_resolving(
        &self,
        thread_id: crate::api::ThreadId,
        run_id: crate::api::RunId,
        policy: &dyn crate::api::RuntimePolicyResolver,
        will_retry: bool,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<Option<crate::ports::runs_ingestion::run_execution_service::RunExecutionHandle>>
    {
        loop {
            let thread = self
                .projections
                .thread(thread_id.clone())
                .await?
                .ok_or_else(|| Error::ThreadNotFound(thread_id.clone()))?;
            let worktree = thread.worktree_path.map(|path| PathBuf::from(path.0));
            let cwd = match &worktree {
                Some(path) => Some(path.clone()),
                None => {
                    let project_id = expanded_id(&thread.project_id)?;
                    self.database
                        .read(move |db| projects::get(db, &project_id, true))
                        .await?
                        .map(|project| PathBuf::from(project.workspace_root))
                }
            };
            let result = self
                .coordination
                .start(cwd.as_deref(), worktree.as_deref(), &cancellation, async {
                    let context = self
                        .projections
                        .turn_start_context(thread_id.clone(), run_id.clone())
                        .await?;
                    if context
                        .projection
                        .thread
                        .worktree_path
                        .as_ref()
                        .map(|p| Path::new(&p.0))
                        != worktree.as_deref()
                        || context.projection.thread.project_id != thread.project_id
                    {
                        return Ok(None);
                    }
                    self.inner
                        .start_from(
                            (thread_id.clone(), run_id.clone()),
                            context,
                            policy,
                            will_retry,
                            cancellation.clone(),
                            None,
                        )
                        .await
                        .map(Some)
                })
                .await?;
            if let Some(result) = result {
                return Ok(result);
            }
        }
    }
}
