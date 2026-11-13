use super::super::{
    ProjectKey,
    projects::EnvironmentProject,
    vcs::{CheckoutKey, VcsCommandScheduler},
};
use crate::{
    ClientError, ClientStore,
    platform::Platform,
    rpc::{RpcCallError, RpcEndpoint},
    services::{
        filesystem::{FileMetadataService, workspace_file_path},
        resources::{Resource, ResourcePolicy, ResourceRegistry},
    },
    types::{EnvironmentId, Generation},
};
use futures::{
    FutureExt, StreamExt,
    future::{Either, select},
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use t3_contracts::{filesystem::FilesystemEntryKind, project::*, rpc::*};

#[derive(Debug, Clone)]
pub struct OptimisticProjectFile {
    pub data: ProjectReadFileResult,
    pub confirmed_against: Option<Rc<ProjectReadFileResult>>,
}
impl PartialEq for OptimisticProjectFile {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
            && match (&self.confirmed_against, &other.confirmed_against) {
                (None, None) => true,
                (Some(left), Some(right)) => Rc::ptr_eq(left, right),
                _ => false,
            }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProjectFileTarget {
    pub environment: EnvironmentId,
    pub cwd: String,
    pub relative_path: String,
}
#[derive(Debug, thiserror::Error)]
pub enum ProjectCommandError {
    #[error(transparent)]
    Rpc(#[from] RpcCallError),
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("The folder for threads without a project has not reached this device yet. Try again.")]
    ScratchProjectNotLoaded { project: crate::types::ProjectId },
}
#[derive(Clone, Default)]
pub struct ProjectCommands {
    pub metadata: FileMetadataService,
    pub queries: Rc<RefCell<ResourceRegistry>>,
    optimistic_files:
        Rc<RefCell<BTreeMap<ProjectFileTarget, Resource<Option<OptimisticProjectFile>>>>>,
    projects: VcsCommandScheduler,
    files: VcsCommandScheduler,
    sessions: Rc<RefCell<BTreeMap<EnvironmentId, Generation>>>,
}
impl ProjectCommands {
    pub fn optimistic_file(
        &self,
        target: &ProjectFileTarget,
    ) -> Resource<Option<OptimisticProjectFile>> {
        self.optimistic_files
            .borrow_mut()
            .entry(target.clone())
            .or_insert_with(|| Resource::new(ResourcePolicy::default(), Generation::default()))
            .clone()
    }
    pub fn set_optimistic_file(
        &self,
        target: &ProjectFileTarget,
        value: Option<OptimisticProjectFile>,
    ) {
        let resource = self.optimistic_file(target);
        let token = resource.begin();
        resource.finish(token, value, 0, false);
    }
    pub fn begin_session(&self, environment: EnvironmentId, generation: Generation) {
        self.sessions
            .borrow_mut()
            .insert(environment.clone(), generation);
        self.metadata.begin_session(environment.clone(), generation);
        self.queries
            .borrow_mut()
            .replace_session(&environment, generation);
    }
    fn generation(&self, environment: &EnvironmentId) -> Generation {
        self.sessions
            .borrow()
            .get(environment)
            .copied()
            .unwrap_or_default()
    }
    pub fn query<M: t3_contracts::rpc::RpcMethod>(
        &self,
        environment: EnvironmentId,
        input: &M::Payload,
    ) -> Result<Resource<M::Success>, ClientError>
    where
        M::Success: 'static,
    {
        let stale_ms = if matches!(
            M::TAG,
            "projects.searchEntries" | "projects.listIgnoredFiles"
        ) {
            15_000
        } else {
            30_000
        };
        self.queries.borrow_mut().query::<M>(
            environment,
            input,
            ResourcePolicy {
                stale_ms,
                ..ResourcePolicy::default()
            },
        )
    }
    pub async fn mutate(
        &self,
        environment: EnvironmentId,
        endpoint: &impl RpcEndpoint,
        mutation: ProjectMutation,
    ) -> Result<Project, RpcCallError> {
        super::source_control::check_destination(endpoint, &environment)?;
        let project = match &mutation {
            ProjectMutation::Create(m) => &m.project_id,
            ProjectMutation::Update(m) => &m.project_id,
            ProjectMutation::Delete(m) => &m.project_id,
        };
        let _lease = self
            .projects
            .acquire(CheckoutKey {
                environment,
                cwd: project.0.clone(),
            })
            .await;
        endpoint.command::<WsProjectsMutateRpc>(mutation).await
    }
    pub async fn create(
        &self,
        environment: EnvironmentId,
        endpoint: &impl RpcEndpoint,
        input: ProjectMutationMember1,
    ) -> Result<Project, RpcCallError> {
        self.mutate(environment, endpoint, ProjectMutation::Create(input))
            .await
    }
    pub async fn update(
        &self,
        environment: EnvironmentId,
        endpoint: &impl RpcEndpoint,
        input: ProjectMutationMember2,
    ) -> Result<Project, RpcCallError> {
        self.mutate(environment, endpoint, ProjectMutation::Update(input))
            .await
    }
    pub async fn delete(
        &self,
        environment: EnvironmentId,
        endpoint: &impl RpcEndpoint,
        input: ProjectMutationMember3,
    ) -> Result<Project, RpcCallError> {
        self.mutate(environment, endpoint, ProjectMutation::Delete(input))
            .await
    }
    pub async fn create_new(
        &self,
        environment: EnvironmentId,
        endpoint: &impl RpcEndpoint,
        input: ProjectCreateNewInput,
    ) -> Result<ProjectCreateNewResult, RpcCallError> {
        super::source_control::check_destination(endpoint, &environment)?;
        let _lease = self
            .projects
            .acquire(CheckoutKey {
                environment,
                cwd: String::new(),
            })
            .await;
        endpoint.command::<WsProjectsCreateNewRpc>(input).await
    }
    pub async fn create_from_thread(
        &self,
        environment: EnvironmentId,
        endpoint: &impl RpcEndpoint,
        input: ProjectCreateFromThreadInput,
    ) -> Result<ProjectCreateFromThreadResult, RpcCallError> {
        super::source_control::check_destination(endpoint, &environment)?;
        let _lease = self
            .projects
            .acquire(CheckoutKey {
                environment,
                cwd: String::new(),
            })
            .await;
        endpoint
            .command::<WsProjectsCreateFromThreadRpc>(input)
            .await
    }
    pub async fn open_scratch(
        &self,
        environment: EnvironmentId,
        endpoint: &impl RpcEndpoint,
        platform: &(impl Platform + ?Sized),
        store: &ClientStore,
        scratch_root: impl Fn() -> Option<String>,
    ) -> Result<EnvironmentProject, ProjectCommandError> {
        super::source_control::check_destination(endpoint, &environment)?;
        let _lease = self
            .projects
            .acquire(CheckoutKey {
                environment: environment.clone(),
                cwd: String::new(),
            })
            .await;
        let generation = store
            .snapshot()
            .environments
            .get(&environment)
            .map(|e| e.session);
        let mut changes = store.subscribe();
        let result = endpoint
            .command::<WsProjectsEnsureScratchRpc>(Empty {})
            .await?;
        let timeout = platform.sleep(10_000).fuse();
        futures::pin_mut!(timeout);
        loop {
            if store
                .snapshot()
                .environments
                .get(&environment)
                .map(|e| e.session)
                != generation
            {
                return Err(ClientError::Cancelled.into());
            }
            if let Some(shell) = store.shared_shell(&environment)
                && let Some(project) = shell.projects.iter().find(|p| p.id == result.project_id)
            {
                let root = scratch_root();
                return Ok(EnvironmentProject {
                    environment,
                    project: project.as_ref().clone(),
                    is_scratch: root.as_deref().is_some_and(|root| {
                        super::paths::find_project_by_path(
                            std::slice::from_ref(project),
                            root,
                            |p| &p.workspace_root.0,
                        )
                        .is_some()
                    }),
                });
            }
            match select(changes.next(), timeout.as_mut()).await {
                Either::Left((Some(_), _)) => (),
                Either::Left((None, _)) => return Err(ClientError::Cancelled.into()),
                Either::Right(_) => {
                    return Err(ProjectCommandError::ScratchProjectNotLoaded {
                        project: result.project_id,
                    });
                }
            }
        }
    }
    pub async fn read_file(
        &self,
        environment: &EnvironmentId,
        endpoint: &impl RpcEndpoint,
        platform: &(impl Platform + ?Sized),
        input: ProjectReadFileInput,
    ) -> Result<ProjectReadFileResult, ClientError> {
        super::source_control::check_destination(endpoint, environment)
            .map_err(RpcCallError::into_client_error)?;
        let token = self.metadata.seed_token(environment);
        let cwd = input.cwd.0.clone();
        let result = endpoint.call::<WsProjectsReadFileRpc>(input).await?;
        if result.exists != Some(Some(false))
            && let Some(token) = token
        {
            self.metadata.remember_file(
                environment,
                token,
                &cwd,
                &result.relative_path,
                result.byte_length,
                platform.monotonic_ms(),
            );
        }
        Ok(result)
    }
    pub async fn list_entries(
        &self,
        environment: &EnvironmentId,
        endpoint: &impl RpcEndpoint,
        platform: &(impl Platform + ?Sized),
        input: ProjectListEntriesInput,
    ) -> Result<ProjectListEntriesResult, ClientError> {
        super::source_control::check_destination(endpoint, environment)
            .map_err(RpcCallError::into_client_error)?;
        let token = self.metadata.seed_token(environment);
        let cwd = input.cwd.0.clone();
        let result = endpoint.call::<WsProjectsListEntriesRpc>(input).await?;
        if let Some(token) = token {
            self.remember_entries(
                environment,
                token,
                &cwd,
                &result.entries,
                platform.monotonic_ms(),
            );
        }
        Ok(result)
    }
    pub async fn search_entries(
        &self,
        environment: &EnvironmentId,
        endpoint: &impl RpcEndpoint,
        platform: &(impl Platform + ?Sized),
        input: ProjectSearchEntriesInput,
    ) -> Result<ProjectSearchEntriesResult, ClientError> {
        super::source_control::check_destination(endpoint, environment)
            .map_err(RpcCallError::into_client_error)?;
        let token = self.metadata.seed_token(environment);
        let cwd = input.cwd.0.clone();
        let result = endpoint.call::<WsProjectsSearchEntriesRpc>(input).await?;
        if let Some(token) = token {
            self.remember_entries(
                environment,
                token,
                &cwd,
                &result.entries,
                platform.monotonic_ms(),
            );
        }
        Ok(result)
    }
    fn remember_entries(
        &self,
        environment: &EnvironmentId,
        token: crate::services::filesystem::MetadataSeed,
        cwd: &str,
        entries: &[ProjectEntry],
        now: u64,
    ) {
        self.metadata.remember_entries(
            environment,
            token,
            cwd,
            entries.iter().map(|entry| {
                (
                    entry.path.0.clone(),
                    match entry.kind {
                        ProjectEntryKind::File => FilesystemEntryKind::File,
                        ProjectEntryKind::Directory => FilesystemEntryKind::Directory,
                    },
                )
            }),
            now,
        );
    }
    pub async fn write_file(
        &self,
        environment: EnvironmentId,
        endpoint: &impl RpcEndpoint,
        platform: &(impl Platform + ?Sized),
        input: ProjectWriteFileInput,
    ) -> Result<ProjectWriteFileResult, RpcCallError> {
        super::source_control::check_destination(endpoint, &environment)?;
        let key = serde_json::to_string(&(&input.cwd, &input.relative_path))
            .map_err(|e| RpcCallError::NotSent(e.into()))?;
        let _lease = self
            .files
            .acquire(CheckoutKey {
                environment: environment.clone(),
                cwd: key,
            })
            .await;
        let generation = self.generation(&environment);
        let cwd = input.cwd.0.clone();
        let path = input.relative_path.clone();
        let result = endpoint.command::<WsProjectsWriteFileRpc>(input).await?;
        if self.generation(&environment) != generation {
            return Ok(result);
        }
        let paths: BTreeSet<_> = [&path, &result.relative_path]
            .into_iter()
            .map(|path| workspace_file_path(&cwd, path))
            .collect();
        for path in paths {
            self.metadata.invalidate(&environment, &path);
            self.metadata
                .enqueue(environment.clone(), &path, None, platform.monotonic_ms());
        }
        let _ = self
            .metadata
            .fetch_pending(&environment, endpoint, platform)
            .await;
        Ok(result)
    }
    pub async fn invalidate_file_metadata(
        &self,
        environment: &EnvironmentId,
        endpoint: &impl RpcEndpoint,
        platform: &(impl Platform + ?Sized),
        paths: impl IntoIterator<Item = String>,
    ) -> Result<(), ClientError> {
        super::source_control::check_destination(endpoint, environment)
            .map_err(RpcCallError::into_client_error)?;
        for path in paths.into_iter().collect::<BTreeSet<_>>() {
            self.metadata.invalidate(environment, &path);
            self.metadata
                .enqueue(environment.clone(), &path, None, platform.monotonic_ms());
        }
        self.metadata
            .fetch_pending(environment, endpoint, platform)
            .await
    }
    pub async fn refresh_ignored_files(
        &self,
        environment: EnvironmentId,
        endpoint: &impl RpcEndpoint,
        platform: &(impl Platform + ?Sized),
        input: ProjectListIgnoredFilesInput,
    ) -> Result<(), ClientError> {
        super::source_control::check_destination(endpoint, &environment)
            .map_err(RpcCallError::into_client_error)?;
        self.query::<WsProjectsListIgnoredFilesRpc>(environment, &input)?
            .execute::<WsProjectsListIgnoredFilesRpc, _, _>(endpoint, platform, input)
            .await
    }
    pub fn remove_environment(&self, environment: &EnvironmentId) {
        self.begin_session(
            environment.clone(),
            Generation(self.generation(environment).0 + 1),
        );
        self.queries.borrow_mut().remove_environment(environment);
        self.metadata.remove_environment(environment);
        for (key, resource) in self.optimistic_files.borrow().iter() {
            if &key.environment == environment {
                let token = resource.begin();
                resource.finish(token, None, 0, false);
            }
        }
        self.optimistic_files
            .borrow_mut()
            .retain(|key, _| &key.environment != environment);
    }
}

macro_rules! file_queries {
    ($($name:ident: $method:ident => $read:ident),* $(,)?) => { impl ProjectCommands { $(
        pub async fn $name(&self, environment: EnvironmentId, endpoint: &impl RpcEndpoint, platform: &(impl Platform + ?Sized), input: <$method as t3_contracts::rpc::RpcMethod>::Payload) -> Result<(), ClientError> {
            let resource = self.query::<$method>(environment.clone(), &input)?;
            resource.run(platform, |_| self.$read(&environment, endpoint, platform, input)).await
        }
    )* } };
}
file_queries! {
    refresh_read_file: WsProjectsReadFileRpc => read_file,
    refresh_list_entries: WsProjectsListEntriesRpc => list_entries,
    refresh_search_entries: WsProjectsSearchEntriesRpc => search_entries,
}

pub fn project_key(environment: EnvironmentId, project: crate::types::ProjectId) -> ProjectKey {
    ProjectKey {
        environment,
        project,
    }
}
