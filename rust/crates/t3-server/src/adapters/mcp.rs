//! The MCP server that gives agents T3's own tools: `apps/server/src/mcp/McpHttpServer.ts` (the
//! `/mcp` route and its authentication), `McpSessionRegistry.ts` (per-thread credentials, hashed
//! at rest, idle expiry, revocation) and `prepareMcpSession` in `ProviderSessionManager.ts` (the
//! credential a provider session receives when it opens). The catalogue, access rules, dispatch
//! and toolkit bindings are `t3-mcp`'s; this adapter binds them to the services in `Domains`.
//!
//! Bound toolkits: messages and tool search (built in), scheduled tasks, attachment uploads,
//! preview, preview controls and HTML (with the preview runtime), devices (with the device
//! runtime). Every other tool answers `t3-mcp`'s typed `available: false` result naming the
//! missing service; `unbound_tools` lists them with the reason.
use crate::services::{DomainContext, DomainRuntime, Domains};
use futures::future::BoxFuture;
use std::{
    collections::BTreeSet,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
use t3_contracts::{base_schemas::ThreadId, provider_instance::ProviderInstanceId};
use t3_mcp::{
    AccessStore, Capability, CredentialRequest, McpError, McpServer, ProviderSessionConfig,
    Services, SessionRegistry,
    access::ThreadShell,
    adapters::{
        ServiceAdapters, attachments::AttachmentAdapter, orchestration::ProjectionAccess,
        scheduler::SchedulerAdapter,
    },
    provider_sessions::{PreparedCredential, ProviderCredentials},
    transport::HttpState,
};
use t3_orchestration::{
    api::ProjectionReader,
    ports::sessions_recovery::provider_session_manager::{
        PreparedCredential as SessionCredential, PreparedSessionCredential, PublishedCredential,
        SessionCredentialReservation, SessionCredentials,
    },
};

/// Tools no host binding serves yet, with the blocker. Model-facing text is never changed here:
/// each still answers `t3-mcp`'s `available: false` result.
pub const UNBOUND_ORCHESTRATION: &str = "the composed orchestration runtime does not hand over its \
     `Dispatcher` and `ThreadManagementServices` (request in server-seam-requests.md, \
     \"MCP: limited dispatch from the composed runtime\")";

/// Per-thread provider credentials. Bound once the server knows its environment id and port;
/// provider adapters read their thread's configuration when a session opens.
#[derive(Default)]
pub struct McpSessions {
    bound: OnceLock<Bound>,
    projections: OnceLock<Arc<dyn ProjectionReader>>,
    /// Idle window and clock for the credential registry; TypeScript's 24 h on the system clock
    /// unless a test supplies its own.
    clock: Option<(i64, Arc<dyn t3_mcp::session::Clock>)>,
}

struct Bound {
    registry: Arc<SessionRegistry>,
    credentials: ProviderCredentials,
    settings: Arc<t3_config::settings::SettingsService>,
}

impl McpSessions {
    /// A store whose credentials expire after `window_ms` idle on `clock`.
    pub fn with_clock(window_ms: i64, clock: Arc<dyn t3_mcp::session::Clock>) -> Self {
        Self {
            clock: Some((window_ms, clock)),
            ..Self::default()
        }
    }
    /// Binds the credential registry to this environment. A second bind is refused.
    pub fn bind(&self, context: &DomainContext<'_>) -> crate::Result<()> {
        let config = &context.local.config;
        let host = config
            .host
            .as_deref()
            .and_then(|host| host.parse::<IpAddr>().ok())
            .unwrap_or(IpAddr::from([127, 0, 0, 1]));
        let environment = t3_contracts::base_schemas::EnvironmentId(
            context.local.environment.environment_id.0.clone(),
        );
        let address = Some(SocketAddr::new(host, config.port));
        let registry = Arc::new(match &self.clock {
            Some((window, clock)) => {
                SessionRegistry::with_clock(environment, address, *window, clock.clone())
            }
            None => SessionRegistry::new(environment, address),
        });
        self.bound
            .set(Bound {
                credentials: ProviderCredentials::new(registry.clone()),
                registry,
                settings: context.local.settings.clone(),
            })
            .map_err(|_| crate::Error::Service("MCP sessions bound twice".into()))
    }
    fn bound(&self) -> Result<&Bound, McpError> {
        self.bound.get().ok_or(McpError::Internal)
    }
    pub fn registry(&self) -> Option<Arc<SessionRegistry>> {
        self.bound.get().map(|bound| bound.registry.clone())
    }
    /// The configuration a provider session for `thread` was prepared with
    /// (`McpProviderSessions.read`): endpoint and bearer header.
    pub fn read(&self, thread: &ThreadId) -> Option<ProviderSessionConfig> {
        self.bound().ok()?.credentials.read(thread).ok().flatten()
    }
    /// `touchActiveMcpThread`: a provider turn starting is a sign the session is alive, so its
    /// thread's credential stays valid however long the agent goes between tool calls. For the
    /// run host's `touch_mcp_credential` (request: `RuntimePorts::run_host`).
    pub fn touch(&self, thread: &ThreadId) -> Result<(), McpError> {
        self.bound()?.credentials.touch(thread)
    }
    /// Revokes every credential of a thread (thread deleted or its provider disabled).
    pub async fn revoke_thread(&self, thread: &ThreadId) -> Result<(), McpError> {
        self.bound()?.credentials.disable(thread).await
    }
    /// The orchestration runtime's credential port. Sessions open only after start, by which
    /// time `follow_projections` has supplied the thread reads it needs.
    pub fn port(self: &Arc<Self>) -> Arc<dyn SessionCredentials> {
        Arc::new(CredentialPort(self.clone()))
    }
    pub fn follow_projections(&self, projections: Arc<dyn ProjectionReader>) {
        let _ = self.projections.set(projections);
    }

    /// TypeScript's `agentAccessSettings` and lineage read: unreadable settings or thread
    /// withhold browser and device tools; an unreadable lineage is not a sub-agent.
    async fn request(&self, thread: ThreadId, instance: ProviderInstanceId) -> CredentialRequest {
        let shell = match self.projections.get() {
            Some(reader) => reader.shell(thread.clone()).await.ok().flatten(),
            None => None,
        };
        let subagent = shell.as_ref().is_some_and(|shell| {
            shell.lineage.relationship_to_parent
                == Some(
                    t3_contracts::orchestration_v2::AppThreadLineageRelationshipToParent::Subagent,
                )
        });
        let (browser, device) = match (&shell, self.bound.get()) {
            (Some(shell), Some(bound)) => match bound.settings.snapshot().await {
                Ok(settings) => agent_access(&settings, &shell.project_id.0),
                Err(error) => {
                    tracing::warn!(%error, "Could not resolve agent access; withholding browser and device tools.");
                    (false, false)
                }
            },
            _ => (false, false),
        };
        let mut capabilities = BTreeSet::from([
            Capability::Orchestration,
            Capability::Worktree,
            Capability::PullRequests,
        ]);
        if browser {
            capabilities.insert(Capability::Preview);
        }
        if device {
            capabilities.insert(Capability::Device);
        }
        CredentialRequest {
            thread_id: thread,
            provider_instance_id: instance,
            browser_tools_available: browser,
            capabilities: Some(capabilities),
            subagent,
        }
    }
}

/// What a provider adapter reads when it configures a thread's native session.
impl t3_orchestration::provider::T3McpSessions for McpSessions {
    fn read(&self, thread: &ThreadId) -> Option<t3_orchestration::provider::T3McpSession> {
        McpSessions::read(self, thread).map(|config| t3_orchestration::provider::T3McpSession {
            endpoint: config.endpoint,
            authorization_header: config.authorization_header,
        })
    }
}

/// `resolveProjectSettings(...).enableAgent{Browser,Device}Access`: a project override wins,
/// otherwise the environment's setting.
fn agent_access(settings: &t3_contracts::settings::ServerSettings, project: &str) -> (bool, bool) {
    let value = serde_json::to_value(settings).unwrap_or_default();
    let effective = |key: &str| {
        value["projectSettingsOverrides"][project][key]
            .as_bool()
            .or_else(|| value[key].as_bool())
            .unwrap_or(false)
    };
    (
        effective("enableAgentBrowserAccess"),
        effective("enableAgentDeviceAccess"),
    )
}

struct CredentialPort(Arc<McpSessions>);

fn port_error(error: McpError) -> t3_orchestration::error::Error {
    t3_orchestration::error::Error::Service {
        operation: "SessionCredentials.prepare",
        detail: error.to_string(),
    }
}

#[async_trait::async_trait]
impl SessionCredentials for CredentialPort {
    async fn prepare_reserved(
        &self,
        thread: ThreadId,
        instance: ProviderInstanceId,
    ) -> t3_orchestration::error::Result<PreparedSessionCredential> {
        let request = self.0.request(thread, instance).await;
        let bound = self.0.bound().map_err(port_error)?;
        let prepared = bound
            .credentials
            .prepare(request)
            .await
            .map_err(port_error)?;
        let id = prepared
            .config
            .scope
            .thread
            .as_ref()
            .map(|caller| caller.provider_session_id.clone())
            .ok_or_else(|| port_error(McpError::Internal))?;
        let issued = prepared.issued;
        Ok(PreparedSessionCredential::with_reservation(
            SessionCredential { id, issued },
            Box::new(Reservation {
                prepared,
                credentials: bound.credentials.clone(),
            }),
        ))
    }
    async fn revoke(
        &self,
        _thread: ThreadId,
        credential_id: String,
    ) -> t3_orchestration::error::Result<()> {
        self.0
            .bound()
            .and_then(|bound| bound.registry.revoke_provider_session(&credential_id))
            .map_err(port_error)
    }
}

struct Reservation {
    prepared: PreparedCredential,
    credentials: ProviderCredentials,
}

impl SessionCredentialReservation for Reservation {
    fn publish(
        self: Box<Self>,
        owner: t3_contracts::base_schemas::ProviderSessionId,
    ) -> t3_orchestration::error::Result<PublishedCredential> {
        let Reservation {
            prepared,
            credentials,
        } = *self;
        prepared.publish(owner.clone()).map_err(port_error)?;
        Ok(Box::new(Published { credentials, owner }))
    }
}

/// Released when the provider session drops its claim: the credential is revoked once no
/// session holds or reserves it.
struct Published {
    credentials: ProviderCredentials,
    owner: t3_contracts::base_schemas::ProviderSessionId,
}
impl Drop for Published {
    fn drop(&mut self) {
        if let Err(error) = self.credentials.release_runtime(&self.owner) {
            tracing::warn!(%error, "MCP credential release failed");
        }
    }
}

/// The served MCP endpoint: `t3-mcp`'s server over the bound services, and the HTTP state its
/// authentication middleware reads.
pub struct McpAdapter {
    pub server: Arc<McpServer>,
    pub http: Arc<HttpState>,
    pub sessions: Arc<McpSessions>,
    /// `mcpApps.*` (`apps/server/src/mcpApps/McpAppRequests.ts`): calls from an MCP app's view
    /// to the provider session that rendered it; never starts a provider.
    pub apps: t3_mcp::apps::AppRequests,
}

impl McpAdapter {
    /// Built after every domain whose tools it binds. Without the orchestration runtime there is
    /// no thread to act as, so the endpoint is not served.
    pub async fn start(
        context: &DomainContext<'_>,
        domains: &Domains,
    ) -> crate::Result<Option<Arc<Self>>> {
        let Some(sessions) = domains.get::<McpSessions>() else {
            return Ok(None);
        };
        let Some(registry) = sessions.registry() else {
            return Ok(None);
        };
        let Some(runtime) = domains
            .get::<crate::adapters::orchestration::OrchestrationAdapter>()
            .and_then(|orchestration| orchestration.composed().cloned())
        else {
            return Ok(None);
        };
        let projections: Arc<dyn ProjectionReader> = runtime.projections.clone();
        let access: Arc<dyn AccessStore> = Arc::new(ProjectionAccess(projections.clone()));

        let mut adapters = ServiceAdapters::default();
        let orchestration_adapter;
        {
            // Thread writes go through `commands_for(caller)`: the caller's modes, a requested
            // escalation and the link fence are checked under the command lock.
            let orchestration = Arc::new(t3_mcp::adapters::orchestration::OrchestrationAdapter {
                reader: projections.clone(),
                database: t3_orchestration::stores::StoreDatabase(context.database.clone()),
                commands: runtime.clone(),
            });
            adapters.orchestration = Some(orchestration.clone());
            adapters.worktree = Some(Arc::new(t3_mcp::adapters::worktree::WorktreeAdapter {
                orchestration: orchestration.clone(),
            }));
            orchestration_adapter = orchestration;
        }
        #[cfg(feature = "scheduler")]
        if let Some(scheduler) = domains.get::<crate::adapters::scheduler::SchedulerAdapter>() {
            adapters.scheduler = Some(Arc::new(SchedulerAdapter {
                service: scheduler.service(),
                reader: Some(projections.clone()),
            }));
        }
        #[cfg(feature = "workspace")]
        if let Some(workspace) = domains.get::<crate::adapters::workspace::WorkspaceAdapter>() {
            adapters.attachments = Some(Arc::new(AttachmentAdapter::new(
                Arc::new(workspace.attachments.clone()),
                Arc::new(t3_mcp::session::SystemClock::default()),
            )));
        }
        let mut services = Services::default();
        Arc::new(adapters).install(&mut services);
        {
            services.orchestrator = Arc::new(Orchestrator {
                reads: Arc::new(t3_mcp::adapters::threads::ThreadReadAdapter {
                    threads: runtime.threads.clone(),
                    database: t3_orchestration::stores::StoreDatabase(context.database.clone()),
                }),
                delegation: delegation(
                    orchestration_adapter.clone(),
                    domains,
                    context,
                    services.orchestrator.clone(),
                ),
                rest: services.orchestrator.clone(),
            });
        }
        #[cfg(feature = "preview")]
        if let Some(preview) = domains.get::<crate::adapters::preview::PreviewAdapter>() {
            let bound = Arc::new(t3_preview::mcp::McpAdapter {
                preview: preview.browser.clone(),
                html: preview.html.clone(),
                access: access.clone(),
                attachments_dir: context.local.config.paths.attachments(),
            });
            services.preview = bound.clone();
            services.preview_controls = bound.clone();
            services.html = bound;
        }
        #[cfg(feature = "device")]
        if let Some(device) = domains.get::<crate::adapters::device::DeviceAdapter>() {
            services.device = Arc::new(device.runtime.mcp(Arc::new(DeviceThreads {
                access: access.clone(),
                device: device.clone(),
            })));
        }

        let linked = context
            .local
            .settings
            .snapshot()
            .await
            .map(|settings| settings.linked_environments_enabled)
            .unwrap_or(false);
        let http = Arc::new(HttpState {
            registry,
            clients: Some(Arc::new(
                t3_mcp::oauth::OAuthAuthenticator::for_request_origin(
                    context.local.auth.inner().clone(),
                ),
            )),
            linked_environments_enabled: AtomicBool::new(linked),
            runtime_ceiling: context
                .local
                .managed_policy
                .as_ref()
                .and_then(|policy| policy.limits.as_ref())
                .and_then(|limits| limits.max_runtime_mode),
        });
        follow_linked_environments(context, http.clone());
        sessions.follow_projections(projections);
        let apps = t3_mcp::apps::AppRequests::new(Arc::new(AppsBackend {
            models: t3_mcp::apps::ModelContextStore::new(context.database.clone()),
            runtime,
        }));
        Ok(Some(Arc::new(Self {
            server: Arc::new(McpServer::new(services, access)),
            http,
            sessions,
            apps,
        })))
    }
}

/// Settings → Labs "linked environments", read live as TypeScript's middleware does; an
/// unreadable value keeps links off.
fn follow_linked_environments(context: &DomainContext<'_>, http: Arc<HttpState>) {
    let mut changes = context.local.settings.subscribe_changes();
    let shutdown = context.shutdown.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                changed = changes.recv() => match changed {
                    Some(settings) => http
                        .linked_environments_enabled
                        .store(settings.linked_environments_enabled, Ordering::Relaxed),
                    None => break,
                },
            }
        }
    });
}

impl DomainRuntime for McpAdapter {
    fn name(&self) -> &'static str {
        "mcp"
    }
    fn shutdown(&self) -> BoxFuture<'_, crate::Result<()>> {
        Box::pin(async move {
            self.http
                .registry
                .revoke_all()
                .map_err(|error| crate::Error::Service(error.to_string()))
        })
    }
}

/// Device tools act in the calling thread's own workspace, from the workspace authority.
#[cfg(feature = "device")]
struct DeviceThreads {
    access: Arc<dyn AccessStore>,
    device: Arc<crate::adapters::device::DeviceAdapter>,
}

#[cfg(feature = "device")]
fn thread_not_found() -> McpError {
    McpError::failure("thread_not_found", "The calling thread was not found.")
}

#[cfg(feature = "device")]
#[async_trait::async_trait]
impl t3_device::mcp::Threads for DeviceThreads {
    async fn shell(&self, thread: &ThreadId) -> t3_mcp::Result<ThreadShell> {
        self.access
            .thread_shell(thread)
            .await?
            .filter(|shell| !shell.deleted)
            .ok_or_else(thread_not_found)
    }
    async fn workspace(&self, thread: &ThreadId) -> t3_mcp::Result<PathBuf> {
        self.device
            .open_context(&thread.0)
            .await
            .workspace_root
            .ok_or_else(thread_not_found)
    }
}

/// MCP app requests over the composed runtime: the app's tool item, the provider thread's live
/// session, fork lineage and the app's model context.
struct AppsBackend {
    runtime: Arc<t3_orchestration::composition::OrchestrationRuntime>,
    models: t3_mcp::apps::ModelContextStore,
}

fn unavailable<E>(_: E) -> McpError {
    McpError::unavailable()
}

impl AppsBackend {
    async fn records(
        &self,
        thread: &ThreadId,
        fields: Vec<t3_orchestration::api::RecordKind>,
    ) -> t3_mcp::Result<serde_json::Value> {
        let records = self
            .runtime
            .threads
            .records(thread.clone(), fields, Default::default())
            .await
            .map_err(unavailable)?;
        serde_json::to_value(records).map_err(|_| McpError::Internal)
    }
}

#[async_trait::async_trait]
impl t3_mcp::apps::AppBackend for AppsBackend {
    async fn app(
        &self,
        thread_id: &ThreadId,
        item_id: &t3_contracts::base_schemas::TurnItemId,
    ) -> t3_mcp::Result<Option<t3_mcp::apps::AppReference>> {
        let Some(item) = self
            .runtime
            .projections
            .turn_item(thread_id.clone(), item_id.clone())
            .await
            .map_err(unavailable)?
        else {
            return Ok(None);
        };
        let item = serde_json::to_value(item).map_err(|_| McpError::Internal)?;
        if item["type"] != "dynamic_tool" {
            return Ok(None);
        }
        let Some(app) = t3_shared::tool_output::mcp_app_from_tool_item(&item)
            .map_err(|_| McpError::Internal)?
        else {
            return Ok(None);
        };
        Ok(Some(t3_mcp::apps::AppReference {
            server: app["server"].as_str().unwrap_or_default().into(),
            tool: app["tool"].as_str().unwrap_or_default().into(),
            provider_thread_id: item["providerThreadId"].as_str().map(str::to_owned),
        }))
    }
    async fn session(
        &self,
        _provider_thread_id: &str,
    ) -> t3_mcp::Result<Option<Arc<dyn t3_mcp::apps::AppSession>>> {
        Err(McpError::Internal)
    }
    async fn session_for(
        &self,
        thread_id: &ThreadId,
        provider_thread_id: &str,
    ) -> t3_mcp::Result<Option<Arc<dyn t3_mcp::apps::AppSession>>> {
        use t3_orchestration::api::SessionDirectory;
        let records = self
            .records(
                thread_id,
                vec![t3_orchestration::api::RecordKind::ProviderThreads],
            )
            .await?;
        let Some(provider_thread) = records["providerThreads"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|candidate| candidate["id"] == provider_thread_id)
            .cloned()
        else {
            return Ok(None);
        };
        let Some(session_id) = provider_thread["providerSessionId"].as_str() else {
            return Ok(None);
        };
        let Some(session) = self
            .runtime
            .sessions
            .get(t3_contracts::base_schemas::ProviderSessionId(
                session_id.into(),
            ))
            .await
            .map_err(unavailable)?
        else {
            return Ok(None);
        };
        if session
            .extensions()
            .is_none_or(|extensions| !extensions.capabilities().mcp_apps)
        {
            return Err(t3_mcp::apps::fail(thread_id, "provider-unsupported"));
        }
        Ok(Some(Arc::new(LiveAppSession {
            session,
            thread: serde_json::from_value(provider_thread).map_err(|_| McpError::Internal)?,
        })))
    }
    async fn fork_parent(&self, thread_id: &ThreadId) -> t3_mcp::Result<Option<ThreadId>> {
        let records = self.records(thread_id, Vec::new()).await?;
        let lineage = &records["thread"]["lineage"];
        Ok((lineage["relationshipToParent"] == "fork")
            .then(|| lineage["parentThreadId"].as_str())
            .flatten()
            .map(|id| ThreadId(id.into())))
    }
    async fn set_model_context(
        &self,
        input: t3_mcp::apps::ModelContextInput,
    ) -> t3_mcp::Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as i64);
        let updated_at = crate::adapters::orchestration::iso_from_unix_millis(now);
        self.models.set(input, updated_at.0).await
    }
}

struct LiveAppSession {
    session: Arc<dyn t3_orchestration::provider::SessionRuntime>,
    thread: t3_contracts::orchestration_v2::OrchestrationV2ProviderThread,
}

fn provider_failure(error: t3_orchestration::provider::ProviderError) -> McpError {
    match error {
        t3_orchestration::provider::ProviderError::SessionStopped => McpError::Domain {
            tag: "ProviderAdapterSessionStoppedError".into(),
            message: error.to_string(),
            fields: serde_json::Value::Null,
        },
        _ => McpError::unavailable(),
    }
}

impl LiveAppSession {
    fn extensions(&self) -> t3_mcp::Result<&dyn t3_orchestration::provider::SessionExtensions> {
        self.session.extensions().ok_or_else(McpError::unavailable)
    }
}

#[async_trait::async_trait]
impl t3_mcp::apps::AppSession for LiveAppSession {
    async fn list_tools(&self, server: &str) -> t3_mcp::Result<Vec<serde_json::Value>> {
        let tools = self
            .extensions()?
            .list_tools(self.thread.clone(), server.into())
            .await
            .map_err(provider_failure)?;
        Ok(tools
            .into_iter()
            .map(|tool| {
                let mut value =
                    serde_json::json!({"name": tool.name, "inputSchema": tool.input_schema});
                if let Some(meta) = tool.meta {
                    value["_meta"] = meta;
                }
                if let Some(annotations) = tool.annotations {
                    value["annotations"] = annotations;
                }
                value
            })
            .collect())
    }
    async fn call_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: serde_json::Value,
    ) -> t3_mcp::Result<t3_contracts::mcp_apps::McpAppCallToolResult> {
        let result = self
            .extensions()?
            .call_tool(self.thread.clone(), server.into(), tool.into(), arguments)
            .await
            .map_err(provider_failure)?;
        let mut value = serde_json::json!({"content": result.content});
        if let Some(structured) = result.structured_content {
            value["structuredContent"] = structured;
        }
        if let Some(is_error) = result.is_error {
            value["isError"] = is_error.into();
        }
        if let Some(meta) = result.meta {
            value["_meta"] = meta;
        }
        serde_json::from_value(value).map_err(|_| McpError::Internal)
    }
    async fn read_resource(
        &self,
        server: &str,
        uri: &str,
    ) -> t3_mcp::Result<t3_contracts::mcp_apps::McpAppReadResourceResult> {
        let contents = self
            .extensions()?
            .read_resource(self.thread.clone(), server.into(), uri.into())
            .await
            .map_err(provider_failure)?;
        Ok(t3_contracts::mcp_apps::McpAppReadResourceResult { contents })
    }
}
/// The orchestrator toolkit: thread reads from the projections, everything else from the
/// concrete sibling bindings (scheduler) or their typed `available: false`.
struct Orchestrator {
    reads: Arc<t3_mcp::adapters::threads::ThreadReadAdapter>,
    /// `delegate_task`, `task_status` and `create_threads`.
    delegation: Arc<dyn t3_mcp::services::OrchestratorService>,
    rest: Arc<dyn t3_mcp::services::OrchestratorService>,
}

/// Delegated tasks over the caller's commands, with provider snapshots from the provider domain;
/// without the provider domain they answer their missing piece.
#[cfg(feature = "providers")]
fn delegation(
    orchestration: Arc<t3_mcp::adapters::orchestration::OrchestrationAdapter>,
    domains: &Domains,
    context: &DomainContext<'_>,
    fallback: Arc<dyn t3_mcp::services::OrchestratorService>,
) -> Arc<dyn t3_mcp::services::OrchestratorService> {
    let (Some(providers), Some(source)) = (
        domains.get::<t3_provider::domain::ProviderDomain>(),
        domains
            .get::<crate::adapters::orchestration::OrchestrationAdapter>()
            .and_then(|o| o.provider_source().cloned()),
    ) else {
        return fallback;
    };
    Arc::new(t3_mcp::adapters::delegation::DelegationAdapter {
        orchestration,
        host: Arc::new(DelegationHost {
            providers,
            source,
            settings: context.local.settings.clone(),
        }),
    })
}
#[cfg(not(feature = "providers"))]
fn delegation(
    _: Arc<t3_mcp::adapters::orchestration::OrchestrationAdapter>,
    _: &Domains,
    _: &DomainContext<'_>,
    fallback: Arc<dyn t3_mcp::services::OrchestratorService>,
) -> Arc<dyn t3_mcp::services::OrchestratorService> {
    fallback
}

#[cfg(feature = "providers")]
struct DelegationHost {
    providers: Arc<t3_provider::domain::ProviderDomain>,
    source: Arc<crate::adapters::native_providers::ProviderSource>,
    settings: Arc<t3_config::settings::SettingsService>,
}

#[cfg(feature = "providers")]
#[async_trait::async_trait]
impl t3_mcp::adapters::delegation::DelegationHost for DelegationHost {
    async fn providers(&self) -> t3_mcp::Result<Vec<serde_json::Value>> {
        self.providers
            .providers
            .get_providers()
            .await
            .0
            .iter()
            .map(|provider| serde_json::to_value(provider).map_err(|_| McpError::Internal))
            .collect()
    }
    async fn orchestration_instances(&self) -> t3_mcp::Result<BTreeSet<String>> {
        use t3_orchestration::ports::sessions_recovery::provider_adapter_registry::ProviderInstanceSource;
        Ok(self
            .source
            .instances()
            .await
            .map_err(|_| McpError::unavailable())?
            .iter()
            .map(|registration| registration.adapter.instance_id().0.clone())
            .collect())
    }
    async fn enabled_for_project(
        &self,
        project: &t3_contracts::base_schemas::ProjectId,
        instance: &str,
    ) -> t3_mcp::Result<bool> {
        let settings = self
            .settings
            .snapshot()
            .await
            .map_err(|_| McpError::unavailable())?;
        t3_orchestration::ports::thread_project::thread_management_service::provider_instance_enabled(
            &settings,
            project,
            &ProviderInstanceId(instance.into()),
        )
        .map_err(|_| McpError::unavailable())
    }
    async fn refresh_instance(&self, instance: &str) -> t3_mcp::Result<Vec<serde_json::Value>> {
        let input = serde_json::from_value(serde_json::json!({ "instanceId": instance, "fresh": true }))
            .map_err(|_| McpError::Internal)?;
        self.providers
            .providers
            .refresh_providers(input)
            .await
            .map_err(|_| McpError::unavailable())?;
        t3_mcp::adapters::delegation::DelegationHost::providers(self).await
    }
}

#[async_trait::async_trait]
impl t3_mcp::services::OrchestratorService for Orchestrator {
    async fn orchestrator_capabilities(
        &self,
        context: t3_mcp::CallContext,
        input: t3_contracts::orchestrator_mcp::OrchestratorMcpCapabilitiesInput,
    ) -> t3_mcp::Result<t3_contracts::orchestrator_mcp::OrchestratorMcpCapabilitiesResult> {
        t3_mcp::services::OrchestratorService::orchestrator_capabilities(
            self.rest.as_ref(),
            context,
            input,
        )
        .await
    }
    async fn delegate_task(
        &self,
        context: t3_mcp::CallContext,
        input: t3_contracts::orchestrator_mcp::OrchestratorMcpDelegateTaskInput,
    ) -> t3_mcp::Result<t3_contracts::orchestrator_mcp::OrchestratorMcpDelegateTaskResult> {
        t3_mcp::services::OrchestratorService::delegate_task(
            self.delegation.as_ref(),
            context,
            input,
        )
        .await
    }
    async fn task_status(
        &self,
        context: t3_mcp::CallContext,
        input: t3_contracts::orchestrator_mcp::OrchestratorMcpTaskStatusInput,
    ) -> t3_mcp::Result<t3_contracts::orchestrator_mcp::OrchestratorMcpDelegateTaskResult> {
        t3_mcp::services::OrchestratorService::task_status(self.delegation.as_ref(), context, input)
            .await
    }
    async fn task_cancel(
        &self,
        context: t3_mcp::CallContext,
        input: t3_contracts::orchestrator_mcp::OrchestratorMcpTaskCancelInput,
    ) -> t3_mcp::Result<t3_contracts::orchestrator_mcp::OrchestratorMcpTaskCancelResult> {
        t3_mcp::services::OrchestratorService::task_cancel(self.rest.as_ref(), context, input).await
    }
    async fn t3_workflow_stop(
        &self,
        context: t3_mcp::CallContext,
        input: t3_mcp::DecodedInput,
    ) -> t3_mcp::Result<serde_json::Value> {
        t3_mcp::services::OrchestratorService::t3_workflow_stop(self.rest.as_ref(), context, input)
            .await
    }
    async fn t3_background_wait(
        &self,
        context: t3_mcp::CallContext,
        input: t3_mcp::DecodedInput,
    ) -> t3_mcp::Result<serde_json::Value> {
        t3_mcp::services::OrchestratorService::t3_background_wait(
            self.rest.as_ref(),
            context,
            input,
        )
        .await
    }
    async fn schedule_task(
        &self,
        context: t3_mcp::CallContext,
        input: t3_contracts::orchestrator_mcp::OrchestratorMcpSaveScheduledTaskInput,
    ) -> t3_mcp::Result<t3_contracts::orchestrator_mcp::OrchestratorMcpScheduleTaskResult> {
        t3_mcp::services::OrchestratorService::schedule_task(self.rest.as_ref(), context, input)
            .await
    }
    async fn list_scheduled_tasks(
        &self,
        context: t3_mcp::CallContext,
        input: t3_contracts::orchestrator_mcp::OrchestratorMcpListScheduledTasksInput,
    ) -> t3_mcp::Result<t3_contracts::orchestrator_mcp::OrchestratorMcpListScheduledTasksResult>
    {
        t3_mcp::services::OrchestratorService::list_scheduled_tasks(
            self.rest.as_ref(),
            context,
            input,
        )
        .await
    }
    async fn delete_scheduled_task(
        &self,
        context: t3_mcp::CallContext,
        input: t3_contracts::orchestrator_mcp::OrchestratorMcpDeleteScheduledTaskInput,
    ) -> t3_mcp::Result<t3_contracts::orchestrator_mcp::OrchestratorMcpDeleteScheduledTaskResult>
    {
        t3_mcp::services::OrchestratorService::delete_scheduled_task(
            self.rest.as_ref(),
            context,
            input,
        )
        .await
    }
    async fn request_secret(
        &self,
        context: t3_mcp::CallContext,
        input: t3_contracts::orchestrator_mcp::OrchestratorMcpRequestSecretInput,
    ) -> t3_mcp::Result<t3_contracts::orchestrator_mcp::OrchestratorMcpRequestSecretResult> {
        t3_mcp::services::OrchestratorService::request_secret(self.rest.as_ref(), context, input)
            .await
    }
    async fn create_threads(
        &self,
        context: t3_mcp::CallContext,
        input: t3_contracts::orchestrator_mcp::OrchestratorMcpCreateThreadsInput,
    ) -> t3_mcp::Result<t3_contracts::orchestrator_mcp::OrchestratorMcpCreateThreadsResult> {
        t3_mcp::services::OrchestratorService::create_threads(self.delegation.as_ref(), context, input)
            .await
    }
    async fn t3_thread_list(
        &self,
        context: t3_mcp::CallContext,
        input: t3_contracts::orchestrator_mcp::OrchestratorMcpThreadListInput,
    ) -> t3_mcp::Result<t3_contracts::orchestrator_mcp::OrchestratorMcpThreadListResult> {
        t3_mcp::services::OrchestratorService::t3_thread_list(self.reads.as_ref(), context, input)
            .await
    }
    async fn t3_thread_read(
        &self,
        context: t3_mcp::CallContext,
        input: t3_contracts::orchestrator_mcp::OrchestratorMcpThreadReadInput,
    ) -> t3_mcp::Result<t3_contracts::orchestrator_mcp::OrchestratorMcpThreadReadResult> {
        t3_mcp::services::OrchestratorService::t3_thread_read(self.reads.as_ref(), context, input)
            .await
    }
    async fn t3_thread_update(
        &self,
        context: t3_mcp::CallContext,
        input: t3_mcp::DecodedInput,
    ) -> t3_mcp::Result<t3_contracts::thread_metadata_mcp::ThreadMetadataMcpUpdateResult> {
        t3_mcp::services::OrchestratorService::t3_thread_update(self.rest.as_ref(), context, input)
            .await
    }
    async fn t3_thread_send(
        &self,
        context: t3_mcp::CallContext,
        input: t3_contracts::orchestrator_mcp::OrchestratorMcpThreadSendInput,
    ) -> t3_mcp::Result<t3_contracts::orchestrator_mcp::OrchestratorMcpThreadSendResult> {
        t3_mcp::services::OrchestratorService::t3_thread_send(self.rest.as_ref(), context, input)
            .await
    }
    async fn t3_thread_wait(
        &self,
        context: t3_mcp::CallContext,
        input: t3_contracts::orchestrator_mcp::OrchestratorMcpThreadWaitInput,
    ) -> t3_mcp::Result<t3_contracts::orchestrator_mcp::OrchestratorMcpThreadWaitResult> {
        t3_mcp::services::OrchestratorService::t3_thread_wait(self.rest.as_ref(), context, input)
            .await
    }
    async fn t3_thread_interrupt(
        &self,
        context: t3_mcp::CallContext,
        input: t3_contracts::orchestrator_mcp::OrchestratorMcpThreadInterruptInput,
    ) -> t3_mcp::Result<t3_contracts::orchestrator_mcp::OrchestratorMcpThreadInterruptResult> {
        t3_mcp::services::OrchestratorService::t3_thread_interrupt(
            self.rest.as_ref(),
            context,
            input,
        )
        .await
    }
    async fn t3_environment_links(
        &self,
        context: t3_mcp::CallContext,
        input: t3_mcp::DecodedInput,
    ) -> t3_mcp::Result<t3_contracts::orchestrator_mcp::OrchestratorMcpEnvironmentLinksResult> {
        t3_mcp::services::OrchestratorService::t3_environment_links(
            self.rest.as_ref(),
            context,
            input,
        )
        .await
    }
    async fn t3_environment_link(
        &self,
        context: t3_mcp::CallContext,
        input: t3_contracts::orchestrator_mcp::OrchestratorMcpEnvironmentLinkInput,
    ) -> t3_mcp::Result<t3_contracts::orchestrator_mcp::OrchestratorMcpEnvironmentLinkResult> {
        t3_mcp::services::OrchestratorService::t3_environment_link(
            self.rest.as_ref(),
            context,
            input,
        )
        .await
    }
    async fn t3_environment_unlink(
        &self,
        context: t3_mcp::CallContext,
        input: t3_contracts::orchestrator_mcp::OrchestratorMcpEnvironmentUnlinkInput,
    ) -> t3_mcp::Result<t3_contracts::orchestrator_mcp::OrchestratorMcpEnvironmentUnlinkResult>
    {
        t3_mcp::services::OrchestratorService::t3_environment_unlink(
            self.rest.as_ref(),
            context,
            input,
        )
        .await
    }
}
