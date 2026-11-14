//! The environment's workspace authority and the workspace services built on it.
//!
//! Port of `apps/server/src/workspace/KnownWorkspaces.ts` and the `assets.createUrl` context
//! lookups in `ws.ts`. A path is a workspace only when it resolves inside a managed worktree
//! folder, a live project's root, or a live thread's worktree that is under a managed folder
//! or listed by Git for that thread's project. Authority is environment-wide, as in
//! TypeScript: which session may call a method is decided by its scopes in the registry.
use std::sync::Arc;

#[cfg(feature = "workspace")]
mod authority {
    use crate::{adapters::persistence::Database, services::DomainContext};
    use futures::future::BoxFuture;
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
        sync::Arc,
    };
    use t3_workspace::paths::{absolute, canonicalize, within};

    pub struct EnvironmentWorkspaces {
        database: Database,
        settings: Arc<t3_config::settings::SettingsService>,
        default_worktrees: PathBuf,
        home: PathBuf,
        /// The environment's one Git root (shared caches, locks and internal-path protection),
        /// handed over once the git adapter has built it. Unset, no worktree listing grants.
        git: std::sync::OnceLock<t3_git::Git>,
        /// Bumped whenever a grant may have narrowed (`changes`).
        changes: tokio::sync::watch::Sender<u64>,
    }

    struct Project {
        id: String,
        workspace_root: String,
        favicon_path: Option<String>,
    }

    /// Missing roots keep their lexical path; other resolution failures and filesystem roots grant
    /// nothing (`resolveRoot` in KnownWorkspaces.ts).
    fn resolve_root(value: &Path) -> Option<PathBuf> {
        let lexical = absolute(value).ok()?;
        let root = match canonicalize(&lexical) {
            Ok(real) => real,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => lexical,
            Err(_) => return None,
        };
        root.parent().is_some().then_some(root)
    }

    fn active_projects(
        connection: &t3_persistence::rusqlite::Connection,
    ) -> t3_persistence::Result<Vec<Project>> {
        let mut statement = connection.prepare(
            "SELECT project_id, workspace_root, favicon_path FROM projection_projects
             WHERE deleted_at IS NULL ORDER BY created_at ASC, project_id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(Project {
                id: row.get(0)?,
                workspace_root: row.get(1)?,
                favicon_path: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    impl EnvironmentWorkspaces {
        pub fn new(context: &DomainContext<'_>) -> Arc<Self> {
            Arc::new(Self {
                database: context.database.clone(),
                settings: context.local.settings.clone(),
                default_worktrees: context.local.config.paths.worktrees(),
                home: context.account_home.clone(),
                git: std::sync::OnceLock::new(),
                changes: tokio::sync::watch::channel(0).0,
            })
        }

        /// Changes when a grant may have narrowed: a project deleted or re-rooted, a thread
        /// deleted or its worktree changed, the managed worktree folders changed. Streams that
        /// re-check the authority per value re-check on it, so an idle one ends at once.
        pub fn changes(&self) -> tokio::sync::watch::Receiver<u64> {
            self.changes.subscribe()
        }

        /// Follows the environment's committed events and settings until shutdown.
        #[cfg(feature = "orchestration")]
        pub fn follow(
            self: &Arc<Self>,
            mut events: tokio::sync::broadcast::Receiver<
                Arc<t3_contracts::application_event::ApplicationStoredEvent>,
            >,
            shutdown: tokio_util::sync::CancellationToken,
        ) {
            use t3_contracts::{
                application_event::{ApplicationProjectEvent, ApplicationStoredEvent},
                orchestration_v2::OrchestrationV2DomainEvent as Event,
            };
            use tokio::sync::broadcast::error::RecvError;
            let this = Arc::downgrade(self);
            let mut settings = self.settings.subscribe_changes();
            tokio::spawn(async move {
                let mut folders = None;
                let mut settings_open = true;
                loop {
                    let narrowed = tokio::select! {
                        () = shutdown.cancelled() => return,
                        event = events.recv() => match event {
                            Ok(event) => match event.as_ref() {
                                ApplicationStoredEvent::Project(event) => matches!(
                                    **event,
                                    ApplicationProjectEvent::Deleted(_)
                                        | ApplicationProjectEvent::MetaUpdated(_)
                                ),
                                ApplicationStoredEvent::Orchestration(event) => matches!(
                                    event.event,
                                    Event::ThreadDeleted(_) | Event::ThreadMetadataUpdated(_)
                                ),
                            },
                            // Missed events may have narrowed a grant.
                            Err(RecvError::Lagged(_)) => true,
                            Err(RecvError::Closed) => return,
                        },
                        changed = settings.recv(), if settings_open => match changed {
                            Some(settings) => {
                                let current = Some((
                                    settings.worktrees_directory,
                                    settings.previous_worktrees_directories,
                                ));
                                std::mem::replace(&mut folders, current) != folders
                            }
                            None => {
                                settings_open = false;
                                false
                            }
                        },
                    };
                    if narrowed {
                        let Some(this) = this.upgrade() else { return };
                        this.changes.send_modify(|revision| *revision += 1);
                    }
                }
            });
        }

        /// Takes the environment's Git root; checkout handles come from `root.at(cwd)`.
        pub fn use_git(&self, root: t3_git::Git) {
            let _ = self.git.set(root);
        }

        /// The folder a live project or thread owns, exactly as recorded, with its live project's
        /// root: a thread's worktree, else its project's root. `None` for an unknown or deleted
        /// record. For actions that name a folder by its owner, never by a client path.
        pub async fn owned_folder(
            &self,
            project_id: Option<&str>,
            thread_id: Option<&str>,
        ) -> Option<(PathBuf, PathBuf)> {
            let (project_id, thread_id) =
                (project_id.map(str::to_owned), thread_id.map(str::to_owned));
            let read = self
                .database
                .reader(move |connection| {
                    use t3_persistence::rusqlite::OptionalExtension;
                    let row = |row: &t3_persistence::rusqlite::Row<'_>| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, Option<String>>(1)?,
                        ))
                    };
                    Ok(match (project_id, thread_id) {
                        (None, Some(thread)) => connection
                            .query_row(
                                "SELECT json_extract(t.payload_json, '$.worktreePath'), p.workspace_root
                                 FROM orchestration_v2_projection_threads t
                                 JOIN projection_projects p
                                   ON p.project_id = t.project_id AND p.deleted_at IS NULL
                                 WHERE t.thread_id = ?1 AND t.deleted_at IS NULL",
                                [thread],
                                row,
                            )
                            .optional()?,
                        (Some(project), None) => connection
                            .query_row(
                                "SELECT NULL, workspace_root FROM projection_projects
                                 WHERE project_id = ?1 AND deleted_at IS NULL",
                                [project],
                                row,
                            )
                            .optional()?,
                        _ => None,
                    })
                })
                .await;
            match read {
                Ok(Some((worktree, Some(root)))) => Some((
                    PathBuf::from(
                        worktree
                            .filter(|path| !path.is_empty())
                            .unwrap_or_else(|| root.clone()),
                    ),
                    PathBuf::from(root),
                )),
                Ok(_) => None,
                Err(error) => {
                    tracing::warn!(%error, "cannot read the folder's owner");
                    None
                }
            }
        }

        /// `project/threadWorkspaceRoot.ts`: the thread's worktree, else its live project's root;
        /// a deleted thread, or a failed read, has none.
        pub async fn thread_workspace_root(&self, thread_id: &str) -> Option<PathBuf> {
            let thread_id = thread_id.to_owned();
            let read = self
                .database
                .reader(move |connection| {
                    use t3_persistence::rusqlite::OptionalExtension;
                    Ok(connection
                        .query_row(
                            "SELECT json_extract(t.payload_json, '$.worktreePath'), p.workspace_root
                             FROM orchestration_v2_projection_threads t
                             LEFT JOIN projection_projects p
                               ON p.project_id = t.project_id AND p.deleted_at IS NULL
                             WHERE t.thread_id = ?1 AND t.deleted_at IS NULL",
                            [thread_id],
                            |row| {
                                Ok((
                                    row.get::<_, Option<String>>(0)?,
                                    row.get::<_, Option<String>>(1)?,
                                ))
                            },
                        )
                        .optional()?)
                })
                .await;
            match read {
                Ok(Some((worktree, root))) => worktree
                    .filter(|path| !path.is_empty())
                    .or(root)
                    .map(PathBuf::from),
                Ok(None) => None,
                Err(error) => {
                    tracing::warn!(%error, "cannot read the thread's workspace");
                    None
                }
            }
        }

        /// `managedWorktreesDirectories`: the default folder, the setting and its predecessors.
        pub async fn managed_directories(&self) -> Vec<PathBuf> {
            let mut directories = vec![self.default_worktrees.clone()];
            if let Ok(settings) = self.settings.snapshot().await {
                let configured = std::iter::once(settings.worktrees_directory)
                    .chain(settings.previous_worktrees_directories);
                for setting in configured {
                    if let Some(directory) = t3_config::home::resolve_worktrees_directory(
                        &setting.0,
                        &self.default_worktrees,
                        &self.home,
                    ) && !directories.contains(&directory)
                    {
                        directories.push(directory);
                    }
                }
            }
            directories
        }

        /// Every root a workspace path may resolve inside, re-read for each check so a removed
        /// project or thread stops granting at once. A database failure grants only managed folders.
        pub async fn roots(&self) -> Vec<PathBuf> {
            self.roots_for(None).await
        }

        /// The roots containing `candidate` (a resolved path), in `KnownWorkspaces.contains`
        /// order: managed folders, then project roots, then thread worktrees, where `git worktree
        /// list` runs only for a thread whose worktree contains the candidate. `None` gathers
        /// every root, listing worktrees for each project with a thread outside managed folders.
        pub async fn roots_for(&self, candidate: Option<&Path>) -> Vec<PathBuf> {
            // Compared in the roots' form: a verbatim drive path (`\\?\C:\...`, what
            // `tokio::fs::canonicalize` returns) is refused by the host-path check, so it is
            // simplified first.
            let candidate = candidate.map(|c| {
                let plain = c
                    .to_str()
                    .and_then(|text| text.strip_prefix(r"\\?\"))
                    .filter(|rest| !rest.starts_with("UNC\\"))
                    .map_or_else(|| c.to_owned(), PathBuf::from);
                canonicalize(&plain).unwrap_or(plain)
            });
            let candidate = candidate.as_deref();
            let contains = |root: &PathBuf| candidate.is_none_or(|c| within(c, root));
            let managed = self.managed_directories().await;
            let mut roots: Vec<PathBuf> = managed
                .iter()
                .filter_map(|d| resolve_root(d))
                .filter(contains)
                .collect();
            if candidate.is_some() && !roots.is_empty() {
                return roots;
            }
            let read = self
                .database
                .reader(|connection| {
                    let projects = active_projects(connection)?;
                    let mut statement = connection.prepare(
                        "SELECT DISTINCT json_extract(payload_json, '$.worktreePath'), project_id
                         FROM orchestration_v2_projection_threads
                         WHERE deleted_at IS NULL
                           AND json_extract(payload_json, '$.worktreePath') IS NOT NULL",
                    )?;
                    let threads = statement
                        .query_map([], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })?
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok((projects, threads))
                })
                .await;
            let (projects, threads) = match read {
                Ok(read) => read,
                Err(error) => {
                    tracing::warn!(%error, "cannot read projects; only managed worktree folders are workspaces");
                    return roots;
                }
            };
            roots.extend(
                projects
                    .iter()
                    .filter_map(|p| resolve_root(Path::new(&p.workspace_root)))
                    .filter(contains),
            );
            if candidate.is_some() && !roots.is_empty() {
                return roots;
            }
            let mut listed: HashMap<String, Vec<PathBuf>> = HashMap::new();
            for (worktree, project_id) in threads {
                let path = PathBuf::from(&worktree);
                // Relative paths would resolve against the server's cwd; TypeScript skips them too.
                if !path.is_absolute() {
                    continue;
                }
                let (Some(real), Ok(lexical)) = (resolve_root(&path), absolute(&path)) else {
                    continue;
                };
                if !contains(&real) {
                    continue;
                }
                if managed.iter().any(|directory| within(&lexical, directory)) {
                    roots.push(real);
                    if candidate.is_some() {
                        return roots;
                    }
                    continue;
                }
                let Some(project) = projects.iter().find(|p| p.id == project_id) else {
                    continue;
                };
                if !listed.contains_key(&project_id) {
                    let Some(root) = self.git.get() else {
                        continue;
                    };
                    let worktrees = root
                        .at(&project.workspace_root)
                        .list_worktrees()
                        .await
                        .map(|worktrees| {
                            worktrees
                                .into_iter()
                                .filter(|w| w.prunable.is_none())
                                .filter_map(|w| resolve_root(&w.path))
                                .collect()
                        })
                        .unwrap_or_default();
                    listed.insert(project_id.clone(), worktrees);
                }
                if listed
                    .get(&project_id)
                    .is_some_and(|paths| paths.contains(&real))
                {
                    roots.push(real);
                    if candidate.is_some() {
                        return roots;
                    }
                }
            }
            roots
        }

        async fn project_by_root(&self, root: String) -> t3_persistence::Result<Option<Project>> {
            self.database
                .reader(move |connection| {
                    Ok(active_projects(connection)?
                        .into_iter()
                        .find(|p| p.workspace_root == root))
                })
                .await
        }

        /// A live thread's worktree, or its live project's root (`assets.createUrl` in ws.ts).
        async fn thread_workspace(
            &self,
            thread_id: String,
        ) -> t3_persistence::Result<Option<PathBuf>> {
            self.database
                .reader(move |connection| {
                    let thread = connection
                        .query_row(
                            "SELECT project_id, json_extract(payload_json, '$.worktreePath')
                             FROM orchestration_v2_projection_threads
                             WHERE thread_id = ?1 AND deleted_at IS NULL",
                            [&thread_id],
                            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                        )
                        .map(Some)
                        .or_else(|error| match error {
                            t3_persistence::rusqlite::Error::QueryReturnedNoRows => Ok(None),
                            error => Err(error),
                        })?;
                    let Some((project_id, worktree)) = thread else {
                        return Ok(None);
                    };
                    let project = active_projects(connection)?
                        .into_iter()
                        .find(|p| p.id == project_id);
                    Ok(project.map(|p| PathBuf::from(worktree.unwrap_or(p.workspace_root))))
                })
                .await
        }
    }

    #[cfg(feature = "git")]
    impl crate::adapters::git::WorkspaceAuthority for EnvironmentWorkspaces {
        fn changes(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
            Some(EnvironmentWorkspaces::changes(self))
        }
        fn roots<'a>(
            &'a self,
            _session: &'a t3_contracts::base_schemas::AuthSessionId,
        ) -> BoxFuture<'a, Result<Vec<PathBuf>, t3_contracts::auth::EnvironmentAuthorizationError>>
        {
            Box::pin(async move { Ok(EnvironmentWorkspaces::roots(self).await) })
        }
        fn roots_for<'a>(
            &'a self,
            _session: &'a t3_contracts::base_schemas::AuthSessionId,
            candidate: &'a Path,
        ) -> BoxFuture<'a, Result<Vec<PathBuf>, t3_contracts::auth::EnvironmentAuthorizationError>>
        {
            Box::pin(
                async move { Ok(EnvironmentWorkspaces::roots_for(self, Some(candidate)).await) },
            )
        }
    }

    mod workspace {
        use super::*;
        use crate::adapters::workspace::{
            AssetAccess, AssetAuthority, AssetSigner, AssetWorkspaceContext, AttachmentStore,
            DocumentText, ExternalAssets, FaviconResolver, InternalPaths, KnownWorkspaces,
            SearchIndexes, WorkspaceAdapter, WorkspaceEntries, WorkspaceFileSystem, WorkspaceRoots,
        };
        use t3_contracts::assets::AssetResource;

        fn invalid(error: impl std::fmt::Display) -> t3_workspace::Error {
            t3_workspace::Error::Invalid(error.to_string())
        }

        impl AssetAuthority for EnvironmentWorkspaces {
            fn resolve<'a>(
                &'a self,
                resource: &'a AssetResource,
            ) -> BoxFuture<'a, t3_workspace::Result<Option<AssetWorkspaceContext>>> {
                Box::pin(async move {
                    match resource {
                        AssetResource::ProjectFavicon { cwd, .. } => Ok(self
                            .project_by_root(cwd.0.clone())
                            .await
                            .map_err(invalid)?
                            .map(|project| AssetWorkspaceContext {
                                workspace_root: Some(PathBuf::from(project.workspace_root)),
                                project_favicon_path: project.favicon_path,
                                ..Default::default()
                            })),
                        AssetResource::WorkspaceFile { thread_id, .. }
                        | AssetResource::MediaFile { thread_id, .. } => Ok(self
                            .thread_workspace(thread_id.0.clone())
                            .await
                            .map_err(invalid)?
                            .map(|root| AssetWorkspaceContext {
                                workspace_root: Some(root),
                                ..Default::default()
                            })),
                        _ => Ok(None),
                    }
                })
            }
        }

        /// Tool images, native app icons and source-control media need application services that
        /// are not ported; they resolve to nothing rather than to a guess.
        struct NoExternalAssets;
        impl ExternalAssets for NoExternalAssets {
            fn tool_image<'a>(
                &'a self,
                _: &'a str,
                _: &'a str,
                _: u64,
            ) -> BoxFuture<'a, t3_workspace::Result<Option<(String, Vec<u8>)>>> {
                Box::pin(async { Ok(None) })
            }
            fn native_icon<'a>(
                &'a self,
                _: &'a t3_contracts::assets::NativeAppIconReference,
            ) -> BoxFuture<'a, t3_workspace::Result<Option<PathBuf>>> {
                Box::pin(async { Ok(None) })
            }
            fn validate_source_control<'a>(
                &'a self,
                _: &'a Path,
                _: &'a str,
                _: Option<&'a str>,
            ) -> BoxFuture<
                'a,
                t3_workspace::Result<Option<crate::adapters::workspace::SourceControlMedia>>,
            > {
                Box::pin(async { Ok(None) })
            }
        }
        /// Document text extraction is not ported; attachments are stored without it.
        struct NoDocumentText;
        impl DocumentText for NoDocumentText {
            fn ensure<'a>(&'a self, _: &'a Path) -> BoxFuture<'a, ()> {
                Box::pin(async {})
            }
        }

        /// The workspace, asset and attachment services over this environment's authority.
        pub async fn workspace_adapter(
            context: &DomainContext<'_>,
            authority: Arc<EnvironmentWorkspaces>,
        ) -> crate::Result<Arc<WorkspaceAdapter>> {
            let service = |error: &dyn std::fmt::Display| crate::Error::Service(error.to_string());
            let paths = &context.local.config.paths;
            let internal = InternalPaths {
                state_dir: paths.state_dir.clone(),
                secrets_dir: paths.secrets(),
            };
            let indexes = Arc::new(SearchIndexes::new(internal.clone()));
            let roots = authority.clone();
            let known: Arc<dyn WorkspaceRoots> = Arc::new(move || {
                let roots = roots.clone();
                async move { KnownWorkspaces::new(roots.roots().await) }
            });
            let secrets = t3_auth::ServerSecretStore::open(paths.secrets())
                .await
                .map_err(|e| service(&e))?;
            let key = secrets
                .get_or_create_random("asset-access-signing-key", 32)
                .await
                .map_err(|e| service(&e))?;
            let signer = AssetSigner::new(key).map_err(|e| service(&e))?;
            let home = context.account_home.clone();
            Ok(Arc::new(WorkspaceAdapter {
                filesystem: WorkspaceFileSystem {
                    home: home.clone(),
                    internal: internal.clone(),
                    known: known.clone(),
                    indexes: indexes.clone(),
                },
                entries: WorkspaceEntries {
                    home: home.clone(),
                    indexes,
                    known,
                },
                assets: AssetAccess {
                    signer: signer.clone(),
                    attachments_dir: paths.attachments(),
                    home: home.clone(),
                    internal,
                    favicons: Arc::new(FaviconResolver::new(home)),
                    external: Arc::new(NoExternalAssets),
                },
                attachments: AttachmentStore::new(
                    paths.attachments(),
                    signer,
                    Arc::new(NoDocumentText),
                ),
                authority,
            }))
        }
    }
    pub use workspace::workspace_adapter;
}
#[cfg(feature = "workspace")]
pub use authority::{EnvironmentWorkspaces, workspace_adapter};

/// Terminal attach, observe and resize run for one socket (`viewer_id`) and stop sending output
/// until the client ACKs (the per-call window), both supplied by the WebSocket transport.
#[cfg(feature = "terminal")]
pub fn terminal_transport() -> Arc<crate::handlers::workspace_terminal::TerminalTransport> {
    use crate::{
        adapters::terminal::TerminalOutputWindow,
        handlers::workspace_terminal::{TerminalRequest, TerminalTransport},
    };
    Arc::new(TerminalTransport {
        resolve: Arc::new(|context| {
            let socket = context.call.socket_id()?;
            let window = Arc::new(TerminalOutputWindow::default());
            let acked = window.clone();
            if !context.call.on_ack(move || acked.ack()) {
                return None;
            }
            Some(TerminalRequest {
                viewer_id: format!("socket-{socket}"),
                output_window: window,
            })
        }),
    })
}
