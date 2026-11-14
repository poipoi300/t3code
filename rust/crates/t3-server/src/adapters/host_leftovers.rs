//! Host services that span several domains (`ws.ts`): `shell.openInEditor`
//! (`process/externalLauncher.ts`), `server.checkSecretSource` and `secrets.answerRequest`
//! (`secrets/`), and `peerLinks.*` (`peer/PeerLinks.ts`, `peer/PeerLinkRequests.ts`). Each part
//! starts only when the domains it needs are present; a missing part leaves its methods unserved.
use std::sync::Arc;

/// What a host injects at the process edge. Tests name the editors installed and record the
/// launches instead of starting editors; production discovers editors on the machine.
#[derive(Default, Clone)]
pub struct HostLeftoversOptions {
    #[cfg(feature = "workspace")]
    pub editors: Option<EditorEdge>,
    /// Filled with the started adapter; tests keep a clone to create the cards an agent's MCP
    /// tools would (`secret_request.record`, `PeerLinkRequests::request`).
    pub started: Option<Arc<std::sync::OnceLock<Arc<HostLeftoversAdapter>>>>,
}

pub struct HostLeftoversAdapter {
    #[cfg(feature = "workspace")]
    pub editors: Option<Arc<editors::Editors>>,
    #[cfg(feature = "providers")]
    pub secrets: Option<Arc<secrets::Secrets>>,
    #[cfg(all(feature = "cloud", feature = "project"))]
    pub peers: Option<Arc<peers::Peers>>,
    #[cfg(feature = "project")]
    pub sessions: Option<Arc<agent_sessions::AgentSessions>>,
}

impl HostLeftoversAdapter {
    #[cfg(feature = "local-services")]
    pub async fn start(
        context: &crate::services::DomainContext<'_>,
        domains: &crate::services::Domains,
        options: &HostLeftoversOptions,
    ) -> crate::Result<Arc<Self>> {
        let _ = (context, domains);
        let adapter = Arc::new(Self {
            #[cfg(feature = "workspace")]
            editors: editors::Editors::start(context, domains, options.editors.clone())
                .map(Arc::new),
            #[cfg(feature = "providers")]
            secrets: secrets::Secrets::start(context, domains)
                .await?
                .map(Arc::new),
            #[cfg(all(feature = "cloud", feature = "project"))]
            peers: peers::Peers::start(context, domains).await?.map(Arc::new),
            #[cfg(feature = "project")]
            sessions: agent_sessions::AgentSessions::start(context, domains).map(Arc::new),
        });
        if let Some(slot) = &options.started {
            let _ = slot.set(adapter.clone());
        }
        Ok(adapter)
    }
}

impl crate::services::DomainRuntime for HostLeftoversAdapter {
    fn name(&self) -> &'static str {
        "host-leftovers"
    }
    /// Stops the secret-request expiry sweep and the peer-link event forwarder.
    fn shutdown(&self) -> futures::future::BoxFuture<'_, crate::Result<()>> {
        Box::pin(async move {
            #[cfg(feature = "providers")]
            if let Some(secrets) = &self.secrets {
                secrets.stop();
            }
            #[cfg(all(feature = "cloud", feature = "project"))]
            if let Some(peers) = &self.peers {
                peers.stop().await?;
            }
            Ok(())
        })
    }
}

#[cfg(feature = "workspace")]
pub use editors::EditorEdge;

/// `shell.openInEditor` over t3-host's launcher: editors and targets come from the server's own
/// discovery and workspace roots, launches are detached processes with discrete arguments.
#[cfg(feature = "workspace")]
pub mod editors {
    use async_trait::async_trait;
    use std::{
        path::{Component, Path, PathBuf, Prefix},
        sync::Arc,
    };
    use t3_contracts::editor::{
        ExternalLauncherError, ExternalLauncherUnsupportedTargetError, LaunchEditorInput,
    };
    use t3_host::editor::{
        DetachedEditorLaunch, DiscoveredEditor, DiscoveredEditors, EditorCommand, EditorDiscovery,
        EditorRoots, ExternalLauncher, SystemEditorDiscovery,
    };

    /// The process edge a test replaces: the editors installed and the launcher.
    #[derive(Clone)]
    pub struct EditorEdge {
        pub editors: Vec<DiscoveredEditor>,
        pub launch: Arc<dyn DetachedEditorLaunch>,
    }

    pub struct Editors {
        pub launcher: Arc<ExternalLauncher>,
    }

    impl Editors {
        #[cfg(feature = "local-services")]
        pub fn start(
            context: &crate::services::DomainContext<'_>,
            domains: &crate::services::Domains,
            edge: Option<EditorEdge>,
        ) -> Option<Self> {
            let workspaces = domains.get::<crate::workspaces::EnvironmentWorkspaces>()?;
            let roots: Arc<dyn EditorRoots> = Arc::new(Roots {
                workspaces,
                // Scratch threads' folders (`t3_project::managed::scratch_folder_root`).
                scratch: context.local.config.paths.base_dir.join("scratch"),
            });
            let discovery: Arc<dyn EditorDiscovery> = match edge {
                Some(edge) => Arc::new(Fixed {
                    roots,
                    edge,
                    home: context.account_home.clone(),
                }),
                None => Arc::new(SystemEditorDiscovery::new(roots, Arc::new(DetachedLaunch))),
            };
            Some(Self {
                launcher: Arc::new(ExternalLauncher::new(discovery)),
            })
        }

        /// TypeScript accepts any path here. A UNC, device or verbatim path would make Windows
        /// open (and authenticate to) a remote host before any check, so such a target is refused
        /// before discovery or the filesystem sees it.
        pub async fn launch(
            &self,
            input: &LaunchEditorInput,
        ) -> Result<(), Box<ExternalLauncherError>> {
            let local = local_path(&input.cwd.0)
                && input
                    .project_root
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_none_or(|root| local_path(&root.0));
            if !local {
                return Err(Box::new(ExternalLauncherError::UnsupportedTarget(
                    ExternalLauncherUnsupportedTargetError::ExternalLauncherUnsupportedTargetError {
                        editor: input.editor,
                    },
                )));
            }
            self.launcher.launch_editor(input).await
        }
    }

    /// A path naming this machine's disks only: never two leading separators (`//host`,
    /// `\\?\`, `/\host`), and on Windows no prefix but a drive letter. A `~/` file-manager
    /// target is checked after the tilde.
    pub fn local_path(raw: &str) -> bool {
        let path = raw
            .strip_prefix("~/")
            .or_else(|| raw.strip_prefix("~\\"))
            .unwrap_or(raw);
        let mut chars = path.chars();
        let separator = |c: Option<char>| matches!(c, Some('/' | '\\'));
        if separator(chars.next()) && separator(chars.next()) {
            return false;
        }
        match Path::new(path).components().next() {
            Some(Component::Prefix(prefix)) => matches!(prefix.kind(), Prefix::Disk(_)),
            _ => true,
        }
    }

    /// Every workspace the server owns: managed worktree folders, live project roots, live
    /// threads' worktrees, and the scratch folder root.
    struct Roots {
        workspaces: Arc<crate::workspaces::EnvironmentWorkspaces>,
        scratch: PathBuf,
    }
    #[async_trait]
    impl EditorRoots for Roots {
        async fn roots(&self) -> t3_host::Result<Vec<PathBuf>> {
            let mut roots = self.workspaces.roots().await;
            roots.push(self.scratch.clone());
            Ok(roots)
        }
    }

    /// Launches through t3-process's detached spawn: the editor outlives the server, gets the
    /// host environment without `ELECTRON_RUN_AS_NODE` (it would make Electron editors run as
    /// Node and exit) plus the desktop session's display variables, and null stdio.
    pub struct DetachedLaunch;
    #[async_trait]
    impl DetachedEditorLaunch for DetachedLaunch {
        async fn launch(&self, command: EditorCommand) -> t3_host::Result<()> {
            let mut options =
                t3_process::ProcessOptions::new(command.command.as_os_str()).args(&command.args);
            options.env.insert("ELECTRON_RUN_AS_NODE".into(), None);
            for (key, value) in t3_host::editor::graphical_session_env().await {
                options.env.insert(key.into(), Some(value.into()));
            }
            t3_process::spawn_detached(
                &options,
                t3_process::DetachedConsole::None,
                command.verbatim,
            )
                .await
                .map(|_| ())
                .map_err(|error| {
                    tracing::warn!(kind = error_kind(&error), "could not start the editor");
                    t3_host::Error::Io
                })
        }
    }

    fn error_kind(error: &t3_process::ProcessError) -> &'static str {
        match error {
            t3_process::ProcessError::NotFound { .. } => "not-found",
            t3_process::ProcessError::Cancelled => "cancelled",
            _ => "spawn",
        }
    }

    /// The injected edge with the server's real roots and the account home.
    struct Fixed {
        roots: Arc<dyn EditorRoots>,
        edge: EditorEdge,
        home: PathBuf,
    }
    #[async_trait]
    impl EditorDiscovery for Fixed {
        async fn discover(&self) -> t3_host::Result<DiscoveredEditors> {
            Ok(DiscoveredEditors {
                roots: self.roots.roots().await?,
                editors: self.edge.editors.clone(),
            })
        }
        fn home_directory(&self) -> Option<PathBuf> {
            Some(self.home.clone())
        }
        async fn launch(&self, command: EditorCommand) -> t3_host::Result<()> {
            self.edge.launch.launch(command).await
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn only_local_disk_paths_reach_the_launcher() {
            for refused in [
                "//attacker.example/share/file.ts",
                r"\\attacker.example\share\file.ts",
                r"/\attacker.example\share",
                r"\/attacker.example/share",
                r"\\?\C:\Users\me\file.ts",
                r"\\.\pipe\name",
                "//?/C:/x",
                r"~/\\attacker.example\share",
            ] {
                assert!(!super::local_path(refused), "{refused}");
            }
            assert!(super::local_path("/home/me/project/src/main.rs:3:1"));
            assert!(super::local_path("~/project"));
            #[cfg(windows)]
            assert!(super::local_path(r"C:\Users\me\project\file.ts:12"));
        }
    }
}

/// `server.checkSecretSource` and `secrets.answerRequest` over the provider domain's one secret
/// resolver; a readable secret rebuilds the provider instances that read it from that same read.
#[cfg(feature = "providers")]
pub mod secrets {
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use t3_contracts::{
        base_schemas::{ThreadId, TurnItemId},
        orchestration_v2::{OrchestrationV2InternalCommand, OrchestrationV2ServerCommand},
        secret_reference::SecretReference,
        secret_request::SecretRequestFailureReason as Reason,
    };
    use t3_host::{
        secret_requests::{
            RequestCard, RequestCards, RequestError, SecretExpiryWorker, SecretRequests,
        },
        secrets::{ProviderSecrets, Secret, SecretBackend, SecretCheck, SecretRefresh},
    };
    use t3_orchestration::{
        api::{RecordFilter, RecordKind},
        composition::OrchestrationRuntime,
        ports::thread_project::thread_management_service::is_terminal_run,
        stores::receipts::ReceiptStatus,
    };

    pub struct Secrets {
        pub check: SecretCheck,
        /// Absent without the composed orchestration runtime, whose cards it answers.
        pub requests: Option<Arc<SecretRequests>>,
        pub cards: Option<Arc<Cards>>,
        worker: Mutex<Option<SecretExpiryWorker>>,
    }

    impl Secrets {
        #[cfg(feature = "local-services")]
        pub async fn start(
            context: &crate::services::DomainContext<'_>,
            domains: &crate::services::Domains,
        ) -> crate::Result<Option<Self>> {
            let (Some(domain), Some(resolver)) = (
                domains.get::<t3_provider::domain::ProviderDomain>(),
                domains.get::<t3_provider::secrets::SecretResolver>(),
            ) else {
                return Ok(None);
            };
            let backend: Arc<dyn SecretBackend> = Arc::new(ProviderSecrets::new(
                context.local.settings.clone(),
                resolver,
                Arc::new(Refresh(domain)),
            ));
            let runtime = domains
                .get::<crate::adapters::orchestration::OrchestrationAdapter>()
                .and_then(|adapter| adapter.composed().cloned());
            let (requests, cards, worker) = match runtime {
                Some(runtime) => {
                    let cards = Arc::new(Cards { runtime });
                    let requests = Arc::new(
                        SecretRequests::new(
                            context.local.config.paths.secrets(),
                            Arc::new(t3_host::background::SystemClock),
                            cards.clone(),
                            backend.clone(),
                        )
                        .map_err(|_| {
                            crate::Error::Service("cannot open the secret request store".into())
                        })?,
                    );
                    let worker = requests.start().await;
                    (Some(requests), Some(cards), Some(worker))
                }
                None => (None, None, None),
            };
            Ok(Some(Self {
                check: SecretCheck::new(backend),
                requests,
                cards,
                worker: Mutex::new(worker),
            }))
        }

        pub fn stop(&self) {
            drop(self.worker.lock().unwrap_or_else(|e| e.into_inner()).take());
        }
    }

    /// `ProviderRegistry.applySecretAndRefresh`: caches the fresh value and rebuilds the
    /// instances that read it; the domain applies settings and managed policy first.
    struct Refresh(Arc<t3_provider::domain::ProviderDomain>);
    #[async_trait]
    impl SecretRefresh for Refresh {
        async fn apply_secret_and_refresh(
            &self,
            reference: &SecretReference,
            value: &Secret,
        ) -> t3_host::Result<()> {
            self.0
                .apply_secret(reference, value.expose())
                .await
                .map(|_| ())
                .map_err(|_| t3_host::Error::Service)
        }
    }

    /// Secret-request cards in thread projections; recording one dispatches
    /// `secret_request.record` with the card's metadata and status, never the value.
    pub struct Cards {
        pub runtime: Arc<OrchestrationRuntime>,
    }
    impl Cards {
        async fn records(&self, thread: &ThreadId) -> Result<Value, RequestError> {
            let projection = self
                .runtime
                .threads
                .records(
                    thread.clone(),
                    vec![RecordKind::Runs, RecordKind::TurnItems],
                    RecordFilter {
                        turn_item_types: Some(vec!["secret_request".into()]),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|_| RequestError(Reason::LoadFailed))?;
            let active = projection
                .runs
                .iter()
                .filter(|run| !is_terminal_run(run))
                .map(|run| run.id.clone())
                .collect::<Vec<_>>();
            let mut value =
                serde_json::to_value(&projection).map_err(|_| RequestError(Reason::LoadFailed))?;
            value["activeRunIds"] = json!(active);
            Ok(value)
        }
        fn item<'a>(records: &'a Value, item: &TurnItemId) -> Result<&'a Value, RequestError> {
            records["turnItems"]
                .as_array()
                .and_then(|items| items.iter().find(|candidate| candidate["id"] == item.0))
                .filter(|card| {
                    card["type"] == "secret_request"
                        && !card["runId"].is_null()
                        && !card["nodeId"].is_null()
                })
                .ok_or(RequestError(Reason::NotFound))
        }
    }
    #[async_trait]
    impl RequestCards for Cards {
        async fn read(
            &self,
            thread: &ThreadId,
            item: &TurnItemId,
        ) -> Result<RequestCard, RequestError> {
            let records = self.records(thread).await?;
            let card = Self::item(&records, item)?;
            let decode = |value: &Value| {
                serde_json::from_value(value.clone()).map_err(|_| RequestError(Reason::LoadFailed))
            };
            let run_active = records["activeRunIds"]
                .as_array()
                .is_some_and(|ids| ids.contains(&card["runId"]));
            Ok(RequestCard {
                project_id: decode(&records["thread"]["projectId"])?,
                thread_id: thread.clone(),
                item_id: item.clone(),
                status: card["secretStatus"].as_str().unwrap_or_default().to_owned(),
                run_active,
                file_delivery: card["delivery"] == "file",
            })
        }
        async fn record(&self, card: &RequestCard, status: &str) -> Result<(), RequestError> {
            let failed = || RequestError(Reason::RecordFailed);
            let records = self.records(&card.thread_id).await.map_err(|_| failed())?;
            let item = Self::item(&records, &card.item_id).map_err(|_| failed())?;
            let mut command = json!({
                "commandId": format!("secret-request:{}:{status}", card.item_id.0),
                "threadId": card.thread_id,
                "runId": item["runId"],
                "nodeId": item["nodeId"],
                "turnItemId": card.item_id,
                "label": item["label"],
                "reason": item["reason"],
                "secretStatus": status,
            });
            for field in ["url", "placeholder", "delivery"] {
                if let Some(value) = item.get(field).filter(|value| !value.is_null()) {
                    command[field] = value.clone();
                }
            }
            let command = serde_json::from_value(command).map_err(|_| failed())?;
            let receipt = self
                .runtime
                .threads
                .dispatch(OrchestrationV2ServerCommand::Internal(Box::new(
                    OrchestrationV2InternalCommand::SecretRequestRecord(Box::new(command)),
                )))
                .await
                .map_err(|_| failed())?;
            if receipt.status == ReceiptStatus::Accepted {
                Ok(())
            } else {
                Err(failed())
            }
        }
    }
}

/// `peerLinks.*`: links this environment holds to others, and the answers to an agent's
/// requests to link one. Mutations follow Settings → Labs "linked environments" live.
#[cfg(all(feature = "cloud", feature = "project"))]
pub mod peers {
    use async_trait::async_trait;
    use std::sync::{Arc, atomic::Ordering};
    use t3_cloud::{
        api::PeerService,
        link_requests::PeerLinkRequests,
        orchestration::{EventForwarder, OrchestrationStore, ProjectRepository},
        peer::PeerLinks,
    };
    use t3_contracts::base_schemas::ProjectId;

    pub struct Peers {
        pub service: Arc<PeerService>,
        pub links: Arc<PeerLinks>,
        /// The requests an agent's `t3_environment_link` tool records (MCP binds the same one).
        pub requests: Arc<PeerLinkRequests>,
        forwarder: tokio::sync::Mutex<Option<EventForwarder>>,
        follower: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    }

    impl Peers {
        #[cfg(feature = "local-services")]
        pub async fn start(
            context: &crate::services::DomainContext<'_>,
            domains: &crate::services::Domains,
        ) -> crate::Result<Option<Self>> {
            let (Some(cloud), Some(runtime)) = (
                domains.get::<crate::adapters::cloud::CloudAdapter>(),
                domains
                    .get::<crate::adapters::orchestration::OrchestrationAdapter>()
                    .and_then(|adapter| adapter.composed().cloned()),
            ) else {
                return Ok(None);
            };
            let links = Arc::new(PeerLinks::new(
                context.database.clone(),
                cloud.secrets.clone(),
                cloud.http.clone(),
                cloud.clock.clone(),
                context.local.environment.clone(),
            ));
            // Subscribed before the first read so no change is missed; unreadable settings
            // leave linking off.
            let settings = context.local.settings.clone();
            let mut changes = settings.subscribe_changes();
            let enabled = settings
                .snapshot()
                .await
                .is_ok_and(|settings| settings.linked_environments_enabled);
            links.enabled.store(enabled, Ordering::Relaxed);
            let follower = {
                let links = links.clone();
                let shutdown = context.shutdown.clone();
                tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            () = shutdown.cancelled() => break,
                            changed = changes.recv() => match changed {
                                Some(settings) => links
                                    .enabled
                                    .store(settings.linked_environments_enabled, Ordering::Relaxed),
                                None => break,
                            },
                        }
                    }
                })
            };
            let (store, forwarder) = OrchestrationStore::new(
                Arc::new(runtime.threads.clone()),
                runtime.projections.clone(),
                Arc::new(Repositories {
                    database: context.database.clone(),
                    identities: t3_project::identity::RepositoryIdentityResolver::default(),
                }),
                Arc::new(runtime.events.clone()),
            );
            let requests = PeerLinkRequests::new(links.clone(), store, cloud.clock.clone());
            Ok(Some(Self {
                service: Arc::new(PeerService::new(links.clone(), requests.clone())),
                links,
                requests,
                forwarder: tokio::sync::Mutex::new(Some(forwarder)),
                follower: tokio::sync::Mutex::new(Some(follower)),
            }))
        }

        pub async fn stop(&self) -> crate::Result<()> {
            if let Some(follower) = self.follower.lock().await.take() {
                follower.abort();
                let _ = follower.await;
            }
            if let Some(forwarder) = self.forwarder.lock().await.take() {
                forwarder.shutdown().await.map_err(|_| {
                    crate::Error::Service("peer link event forwarder failed".into())
                })?;
            }
            Ok(())
        }
    }

    /// `repositoryIdentities.resolve(project.workspaceRoot)?.canonicalKey`, for a live project.
    struct Repositories {
        database: t3_persistence::Database,
        identities: t3_project::identity::RepositoryIdentityResolver,
    }
    #[async_trait]
    impl ProjectRepository for Repositories {
        async fn canonical_key(&self, project: &ProjectId) -> t3_cloud::Result<Option<String>> {
            let id = project.0.clone();
            let root = self
                .database
                .reader(move |connection| {
                    use t3_persistence::rusqlite::OptionalExtension;
                    Ok(connection
                        .query_row(
                            "SELECT workspace_root FROM projection_projects
                             WHERE project_id = ?1 AND deleted_at IS NULL",
                            [id],
                            |row| row.get::<_, String>(0),
                        )
                        .optional()?)
                })
                .await
                .map_err(|_| t3_cloud::Error::Storage)?;
            let Some(root) = root else {
                return Ok(None);
            };
            Ok(self
                .identities
                .resolve(&root, false)
                .await
                .map(|identity| identity.canonical_key.0))
        }
    }
}

/// `agentSessions.{scan, import}` (`project/AgentSessionScanner.ts`, `AgentSessionImporter.ts`):
/// the provider session stores named by each enabled instance's settings and the host
/// environment, read under the inventory's bounds. A client chooses only the project to import
/// into; the stores, the account home and every limit come from the server.
#[cfg(feature = "project")]
pub mod agent_sessions {
    use std::sync::Arc;
    use t3_project::{
        import_store::RuntimeImportStore,
        inventory::{InventoryPaths, NativeSessionInventory},
        sessions::{AgentSessionImporter, AgentSessionScanner},
    };

    pub struct AgentSessions {
        pub scanner: AgentSessionScanner,
        pub importer: AgentSessionImporter,
    }

    impl AgentSessions {
        #[cfg(feature = "local-services")]
        pub fn start(
            context: &crate::services::DomainContext<'_>,
            domains: &crate::services::Domains,
        ) -> Option<Self> {
            let projects = domains.get::<crate::adapters::project::ProjectAdapter>()?;
            let runtime = domains
                .get::<crate::adapters::orchestration::OrchestrationAdapter>()?
                .composed()
                .cloned()?;
            let paths = &context.local.config.paths;
            let ceiling = context
                .local
                .managed_policy
                .as_ref()
                .and_then(|policy| policy.limits.as_ref())
                .and_then(|limits| limits.max_runtime_mode);
            let store = Arc::new(RuntimeImportStore::new(
                t3_orchestration::stores::StoreDatabase(context.database.clone()),
                context.local.settings.clone(),
                ceiling,
            ));
            let home = context.account_home.clone();
            let temporary_root = std::env::temp_dir();
            let inventory = Arc::new(NativeSessionInventory::new(
                InventoryPaths {
                    home: home.clone(),
                    temporary_root: temporary_root.clone(),
                    base_dir: paths.base_dir.clone(),
                    worktrees_dir: paths.worktrees(),
                },
                context.environment.clone(),
                store.clone(),
            ));
            let scanner = AgentSessionScanner {
                inventory: Arc::new(inventory),
                projects: projects.service().clone(),
                clock: runtime.clock(),
                home,
                temporary_root,
                base_dir: paths.base_dir.clone(),
                worktrees_dir: paths.worktrees(),
            };
            let importer = AgentSessionImporter {
                scanner: scanner.clone(),
                projects: projects.service().clone(),
                projections: runtime.projections.clone(),
                sink: Arc::new(runtime.events.clone()),
                runtimes: store,
                lanes: runtime.thread_lanes(),
            };
            Some(Self { scanner, importer })
        }
    }
}
