#![allow(dead_code)]
use async_trait::async_trait;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_contracts::{project::*, project_clone::*};
use t3_orchestration::{
    ports::{
        decisions::thread_command_executor::KeyedLanes,
        sessions_recovery::legacy::legacy_v1_thread_importer::LegacyV1ThreadImporter,
    },
    projections::store::ProjectionStore,
    runtime::{commit::EventSink, worker::OutboxSignals},
    stores::StoreDatabase,
    testing::{ManualClock, SequenceIds},
};
use t3_project::{
    clones::*, enrichment::ProjectEnrichmentService, identity::RepositoryIdentityResolver,
    managed::ManagedFolderIo, *,
};

pub struct NoClones;
#[async_trait]
impl CloneRepository for NoClones {
    async fn prepare(&self, _: &ProjectCloneStartInput) -> Result<PreparedClone> {
        Err(Error::Unavailable("test has no clone service"))
    }
    async fn clone_repository(
        &self,
        _: &ProjectCloneStartInput,
        _: &PreparedClone,
        _: Arc<dyn Fn(CloneProgress) + Send + Sync>,
        _: t3_process::CancellationToken,
    ) -> Result<()> {
        Err(Error::Unavailable("test has no clone service"))
    }
    async fn discard(&self, _: &str) -> Result<()> {
        Err(Error::Unavailable("test has no clone service"))
    }
}
pub struct Fixture {
    pub temp: tempfile::TempDir,
    pub service: ProjectService,
    pub database: StoreDatabase,
    pub sink: EventSink,
    pub projections: Arc<ProjectionStore>,
    pub clock: Arc<ManualClock>,
    pub clones: ProjectCloneTracker,
}
impl Fixture {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().expect("temp fixture");
        let database = StoreDatabase(
            t3_persistence::Database::open(temp.path().join("statev2.sqlite")).expect("database"),
        );
        let clock = Arc::new(ManualClock::new("2026-09-04T12:00:00.000Z"));
        let ids = Arc::new(SequenceIds::new("project-test"));
        let projections = Arc::new(ProjectionStore::new(database.clone()));
        let sink = EventSink::new(
            database.clone(),
            projections.clone(),
            clock.clone(),
            OutboxSignals::default(),
        );
        let legacy = Arc::new(LegacyV1ThreadImporter::new(
            database.clone(),
            Arc::new(sink.clone()),
            clock.clone(),
            ids.clone(),
        ));
        let clones = ProjectCloneTracker::new(Arc::new(NoClones), clock.clone());
        let enrichment = ProjectEnrichmentService::new(
            RepositoryIdentityResolver::default().with_environment(git_environment(temp.path())),
            Arc::new(t3_workspace::project::FaviconResolver::new(
                temp.path().into(),
            )),
        );
        let settings = t3_config::settings::SettingsService::new(temp.path().join("settings.json"));
        let service = ProjectService::new_with_git(
            ProjectServices {
                database: database.clone(),
                sink: sink.clone(),
                projections: projections.clone(),
                legacy,
                thread_lanes: Arc::new(KeyedLanes::default()),
                clock: clock.clone(),
                ids,
                settings,
                enrichment,
                clones: clones.clone(),
                home: temp.path().into(),
            },
            t3_git::Git::new(temp.path()).with_environment(git_environment(temp.path())),
        );
        Self {
            temp,
            service,
            database,
            sink,
            projections,
            clock,
            clones,
        }
    }
    pub fn root(&self, name: &str) -> PathBuf {
        let p = self.temp.path().join(name);
        std::fs::create_dir_all(&p).expect("fixture directory");
        p
    }
    pub fn create(&self, id: &str, command: &str, root: &Path) -> ProjectMutationMember1 {
        serde_json::from_value(serde_json::json!({"type":"project.create","commandId":command,"projectId":id,"title":id,"workspaceRoot":root})).expect("create input")
    }
    pub fn delete(&self, id: &str, command: &str, force: bool) -> ProjectMutationMember3 {
        serde_json::from_value(serde_json::json!({"type":"project.delete","commandId":command,"projectId":id,"force":force})).expect("delete input")
    }
    pub async fn shutdown(&self) {
        self.service.shutdown().await;
        self.service.enrichment().shutdown().await;
        self.clones.shutdown().await;
        self.sink.drain().await;
    }
}
pub struct FixtureIo;
fn io_error(e: std::io::Error) -> Error {
    Error::Operation {
        operation: "fixture io",
        detail: e.to_string(),
    }
}
#[async_trait]
impl ManagedFolderIo for FixtureIo {
    async fn create_directory(&self, path: &Path, recursive: bool) -> Result<bool> {
        if recursive {
            std::fs::create_dir_all(path).map_err(io_error)?;
            return Ok(true);
        }
        match std::fs::create_dir(path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(io_error(e)),
        }
    }
    async fn write_file(&self, path: &Path, contents: &str) -> Result<()> {
        std::fs::write(path, contents).map_err(io_error)
    }
    async fn remove_directory(&self, path: &Path) -> Result<()> {
        let metadata = std::fs::symlink_metadata(path).map_err(io_error)?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(Error::Invalid("Refusing reparse point".into()));
            }
        }
        if metadata.file_type().is_symlink() {
            return Err(Error::Invalid("Refusing symlink".into()));
        }
        std::fs::remove_dir_all(path).map_err(io_error)
    }
}
pub fn git_environment(root: &Path) -> t3_process::Environment {
    [
        (
            "GIT_CONFIG_GLOBAL",
            root.join("empty-git-config").into_os_string(),
        ),
        ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ("GIT_AUTHOR_NAME", "Fixture".into()),
        ("GIT_AUTHOR_EMAIL", "fixture@example.invalid".into()),
        ("GIT_COMMITTER_NAME", "Fixture".into()),
        ("GIT_COMMITTER_EMAIL", "fixture@example.invalid".into()),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), Some(v)))
    .collect()
}
