//! FirstRunGate.tsx: combine the saved settings and catalog with live, session-scoped
//! workspace evidence. A bootstrap project can look exactly like a user's only project.
use super::*;
use crate::{
    thread_views::{
        composer::preferences::{ComposerPreferences, SettingsHydration},
        threads::pending_work::background_work_holds_completion,
    },
    types::SyncStatus,
    view_logic::onboarding::*,
};
use futures::{future::Either, stream};
use t3_contracts::{
    orchestration_v2::OrchestrationV2ShellThreadStatus,
    rpc::WsSubscribeServerLifecycleRpc,
    server::{ServerBootstrapStatus, ServerLifecycleStreamEvent},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexSetupMode {
    Existing,
    Managed,
}
impl CodexSetupMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Existing => "existing",
            Self::Managed => "managed",
        }
    }
}

#[derive(Clone)]
pub struct FirstRun<P: Platform + Clone + 'static>(ClientRuntime<P>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirstRunStatus {
    pub target: Option<EnvironmentId>,
    pub catalog_ready: bool,
    pub hydration: SettingsHydration,
    pub gate: FirstRunGate,
    pub persist_completion: bool,
}

impl<P: Platform + Clone + 'static> ClientRuntime<P> {
    pub fn first_run(&self) -> FirstRun<P> {
        FirstRun(self.clone())
    }

    /// Test hosts have no native primary discovery. Supply the host route of their real,
    /// already paired supervisor without treating arbitrary saved computers as primary.
    #[cfg(feature = "testing")]
    pub fn supply_test_primary_route(&self, environment: &EnvironmentId) {
        let mut entries = self.inner.environments.borrow_mut();
        let entry = entries
            .iter_mut()
            .find(|entry| &entry.id == environment)
            .expect("test server was paired");
        let mut local = entry
            .routes
            .first()
            .expect("test server has a route")
            .clone();
        local.mode = crate::connection::ConnectionMode::Local;
        local.id = "test-primary".into();
        entry.routes.insert(0, local);
        drop(entries);
        self.notify();
    }

    pub(super) fn follow_welcome(&self, environment: &EnvironmentId) -> AbortHandle {
        let platform = self.inner.platform.clone();
        let store = self.store().clone();
        let environment = environment.clone();
        let sessions = self
            .query_connections(&environment)
            .map(|connection| connection.session)
            .boxed_local();
        self.spawn(
            async move {
                let target = environment.clone();
                let source = store.clone();
                let mut events = EnvironmentStream::recovering(
                    environment.clone(),
                    sessions,
                    &platform,
                    WsSubscribeServerLifecycleRpc::NAME,
                    move |session| {
                        let generation = source
                            .snapshot()
                            .environments
                            .get(&target)
                            .map(|env| env.session);
                        if let Some(generation) = generation {
                            source.set_welcome(&target, generation, None);
                        }
                        session
                            .endpoint
                            .subscribe::<WsSubscribeServerLifecycleRpc>(Default::default())
                            .map(move |event| event.map(|event| (generation, event)))
                            .boxed_local()
                    },
                );
                while let Some(event) = events.next().await {
                    if let Ok((Some(generation), ServerLifecycleStreamEvent::Welcome(event))) =
                        event
                    {
                        store.set_welcome(&environment, generation, Some(event.payload));
                    }
                }
            }
            .boxed_local(),
        )
    }
}

impl<P: Platform + Clone + 'static> FirstRun<P> {
    pub fn target(&self) -> Option<EnvironmentId> {
        self.0.settings().primary_environment()
    }

    pub fn catalog_ready(&self) -> bool {
        self.0.inner.restored.get()
    }

    pub async fn complete(&self) -> Result<(), ClientError> {
        let prefs = self.0.composer_preferences();
        // A writer without a hydrated cache merges the very record used as its CAS condition.
        // The mounted cache may predate an edit from another window.
        let writer = ComposerPreferences::new(Rc::new(self.0.inner.platform.clone()));
        let date = crate::time::iso_from_epoch_ms(self.0.inner.platform.now_epoch_ms());
        // Host navigation may unmount Welcome during a disk write. The runtime owns the
        // update through commit or rollback, even when its original waiter is gone.
        let (reply, saved) = futures::channel::oneshot::channel();
        (self.0.inner.spawn)(
            async move {
                let result = async {
                    writer
                        .update(move |settings| {
                            settings.onboarding_completed_at.get_or_insert(date);
                        })
                        .await?;
                    prefs.load().await
                }
                .await;
                let _ = reply.send(result);
            }
            .boxed_local(),
        );
        saved.await.map_err(|_| ClientError::Cancelled)?
    }

    /// Persist a populated workspace even when its welcome page or pairing route is open.
    /// Retry a failed conditional write while the same primary still qualifies.
    pub async fn follow_completion(&self) {
        let mut changes = self.changes();
        loop {
            let status = self.snapshot();
            if status.persist_completion && status.gate.decision == FirstRunDecision::App {
                let mut delay = 1000;
                while self.complete().await.is_err() {
                    self.0.inner.platform.sleep(delay).await;
                    delay = (delay * 2).min(60_000);
                    let next = self.snapshot();
                    if next.target != status.target
                        || !next.persist_completion
                        || next.gate.decision != FirstRunDecision::App
                    {
                        break;
                    }
                }
            }
            if changes.next().await.is_none() {
                return;
            }
        }
    }

    pub(super) fn changes(&self) -> LocalBoxStream<'static, ()> {
        let shells = self.0.store().subscribe().filter_map(|change| async move {
            change
                .scopes
                .iter()
                .any(|scope| matches!(scope, Scope::Shell(_)))
                .then_some(())
        });
        stream::select(
            stream::select(self.0.changes(), shells),
            self.0.composer_preferences().changes(),
        )
        .boxed_local()
    }

    pub fn snapshot(&self) -> FirstRunStatus {
        let prefs = self.0.composer_preferences();
        let hydration = prefs.hydration();
        let target = self.target();
        let mut input = FirstRunInput {
            // Before restore we cannot distinguish a primary from a remote-only catalog.
            enabled: target.is_some() || !self.catalog_ready(),
            hydrated: hydration == SettingsHydration::Ready,
            completed: prefs.snapshot().onboarding_completed_at.is_some(),
            catalog_ready: self.catalog_ready(),
            ..Default::default()
        };
        if let Some(target) = target
            .as_ref()
            .filter(|_| input.hydrated && !input.completed)
        {
            self.workspace_evidence(target, &mut input);
        }
        let (decision, persist_completion) = resolve_first_run_decision(&input);
        let mut previous = self.0.inner.first_run_gate.borrow_mut();
        let gate = match (&*previous, &target) {
            (Some((id, previous)), Some(target)) if id == target => {
                previous.transition(FirstRunGateEvent::Evidence(decision))
            }
            _ => FirstRunGate {
                decision,
                stalled: false,
            },
        };
        *previous = target.clone().map(|id| (id, gate));
        FirstRunStatus {
            catalog_ready: input.catalog_ready,
            target,
            hydration,
            gate,
            persist_completion,
        }
    }

    fn workspace_evidence(&self, target: &EnvironmentId, input: &mut FirstRunInput) {
        let store = self.0.store().snapshot();
        let primary = store.environments.get(target);
        let config = primary.and_then(|env| env.config.as_deref());
        let welcome = primary.and_then(|env| env.welcome.as_deref());
        input.server_config_available = config.is_some();
        input.authoritative = primary.is_some_and(|env| env.shell.status == SyncStatus::Live);
        input.workspace_provenance_authoritative = first_run_provenance_authoritative(
            welcome.is_some(),
            welcome
                .and_then(|w| w.bootstrap_status.flatten())
                .map(|s| s == ServerBootstrapStatus::Pending),
        );
        input.bootstrapped = self.0.environments().iter().all(|entry| {
            store
                .environments
                .get(&entry.id)
                .is_some_and(|env| env.shell.snapshot.is_some())
                || disconnected_shell_is_settled(&entry.state)
        });
        let mut workspace = FirstRunWorkspace {
            primary_environment: Some(&target.0),
            server_cwd: config.map(|cfg| cfg.cwd.0.as_str()),
            bootstrap_project: welcome
                .and_then(|w| w.bootstrap_project_id.as_ref()?.as_ref())
                .map(|id| id.0.as_str()),
            bootstrap_thread: welcome
                .and_then(|w| w.bootstrap_thread_id.as_ref()?.as_ref())
                .map(|id| id.0.as_str()),
            bootstrap_project_created: welcome.and_then(|w| w.bootstrap_project_created.flatten())
                == Some(true),
            bootstrap_thread_created: welcome.and_then(|w| w.bootstrap_thread_created.flatten())
                == Some(true),
            ..Default::default()
        };
        input.workspace_authoritative = true;
        for (environment, state) in &store.environments {
            let Some(shell) = &state.shell.snapshot else {
                continue;
            };
            input.project_count += shell.projects.len();
            input.thread_count += shell.threads.len();
            if !shell.projects.is_empty() || !shell.threads.is_empty() {
                input.workspace_authoritative &= state.shell.status == SyncStatus::Live;
            }
            if input.project_count == 1
                && let [project] = shell.projects.as_slice()
            {
                workspace.projects.push(FirstRunProject {
                    id: &project.id.0,
                    environment: &environment.0,
                    workspace_root: &project.workspace_root.0,
                });
            }
            if input.thread_count == 1
                && let [thread] = shell.threads.as_slice()
            {
                let held = thread
                    .pending_background_tasks
                    .as_ref()
                    .and_then(Option::as_deref)
                    .is_some_and(background_work_holds_completion);
                workspace.threads.push(FirstRunThread {
                    id: &thread.id.0,
                    project: &thread.project_id.0,
                    environment: &environment.0,
                    has_latest_run: thread.latest_run_id.is_some(),
                    has_user_message: thread.latest_user_message_at.is_some(),
                    has_runtime: thread.latest_run_id.is_some()
                        || (held && thread.status != OrchestrationV2ShellThreadStatus::Failed),
                });
            }
        }
        input.workspace_fresh = input.thread_count <= 1
            && input.project_count <= 1
            && is_fresh_first_run_workspace(&workspace);
    }

    /// Current-first, with one four-second deadline for a hydrated pending decision.
    /// Only a changed decision, recovery state or persistence request reaches the view.
    pub fn watch(&self) -> LocalBoxStream<'static, FirstRunStatus> {
        let service = self.clone();
        let changes = self.changes();
        stream::unfold(
            (service, changes, None::<FirstRunStatus>, None::<u64>),
            |(service, mut changes, previous, mut deadline)| async move {
                loop {
                    let mut next = service.snapshot();
                    let same_target = previous
                        .as_ref()
                        .filter(|prior| prior.target == next.target);
                    if let Some(prior) = same_target {
                        next.gate = prior
                            .gate
                            .transition(FirstRunGateEvent::Evidence(next.gate.decision));
                    } else {
                        deadline = None;
                    }
                    let platform = &service.0.inner.platform;
                    let now = platform.monotonic_ms();
                    if next.gate.decision == FirstRunDecision::Pending
                        && next.target.is_some()
                        && next.hydration == SettingsHydration::Ready
                    {
                        let end = *deadline.get_or_insert_with(|| now.saturating_add(4000));
                        if now >= end {
                            next.gate = next.gate.transition(FirstRunGateEvent::Timeout);
                        }
                    } else {
                        deadline = None;
                    }
                    if previous.as_ref() != Some(&next) {
                        return Some((next.clone(), (service, changes, Some(next), deadline)));
                    }
                    if let Some(remaining) = deadline
                        .and_then(|end| end.checked_sub(now))
                        .filter(|ms| *ms > 0)
                    {
                        if let Either::Left((None, _)) =
                            futures::future::select(changes.next(), platform.sleep(remaining)).await
                        {
                            return None;
                        }
                    } else {
                        changes.next().await?;
                    }
                }
            },
        )
        .boxed_local()
    }
}

impl<P: Platform + Clone + 'static> FirstRun<P> {
    fn endpoint(&self, environment: &EnvironmentId) -> Result<SessionEndpoint, ClientError> {
        self.0
            .client(environment)
            .and_then(|client| client.session())
            .ok_or(ClientError::Disconnected)
    }

    pub fn can_manage(&self, environment: &EnvironmentId) -> bool {
        self.endpoint(environment).ok().and_then(|endpoint| {
            endpoint.grants_scope(t3_contracts::auth::AuthEnvironmentScope::ProvidersManage)
        }) == Some(true)
    }

    fn management_endpoint(
        &self,
        environment: &EnvironmentId,
    ) -> Result<SessionEndpoint, ClientError> {
        let endpoint = self.endpoint(environment)?;
        if endpoint.grants_scope(t3_contracts::auth::AuthEnvironmentScope::ProvidersManage)
            != Some(true)
        {
            return Err(ClientError::PermissionDenied {
                method: "providers:manage".into(),
            });
        }
        Ok(endpoint)
    }

    /// The local provider owns its loopback callback. A remote login needs a
    /// separate callback receiver or a trusted handoff on the primary computer.
    pub fn local_sign_in(&self, environment: &EnvironmentId) -> bool {
        let url = self
            .0
            .client(environment)
            .and_then(|client| client.http_base_url())
            .and_then(|raw| url::Url::parse(&raw).ok());
        url.as_ref()
            .and_then(|url| url.host())
            .is_some_and(|host| match host {
                url::Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
                url::Host::Ipv4(ip) => ip.is_loopback(),
                url::Host::Ipv6(ip) => ip.is_loopback(),
            })
    }

    async fn prepare_managed_codex(
        &self,
        environment: &EnvironmentId,
        id: &t3_contracts::provider_instance::ProviderInstanceId,
    ) -> Result<SessionEndpoint, ClientError> {
        use t3_contracts::{
            provider_setup::*,
            rpc::{WsProviderInstallStartRpc, WsProviderInstallSubscribeRpc},
        };
        let mut changes = self.0.store().subscribe_environment(environment);
        // Mode changes republish server config. The previous CLI's capabilities cannot
        // authorize starting the managed runtime.
        let (endpoint, can_install) = loop {
            let endpoint = self.management_endpoint(environment)?;
            let snapshot = self.0.store().snapshot();
            if let Some(config) = snapshot
                .environments
                .get(environment)
                .and_then(|state| state.config.as_ref())
            {
                let managed = config
                    .settings
                    .provider_instances
                    .get(id)
                    .and_then(|instance| instance.config.as_ref())
                    .and_then(|config| config.get("setupMode"))
                    .and_then(|mode| mode.as_str())
                    == Some("managed");
                if managed
                    && let Some(setup) = config
                        .providers
                        .0
                        .iter()
                        .find(|p| p.instance_id == *id)
                        .and_then(|p| p.setup.as_ref()?.as_ref())
                        .filter(|setup| setup.can_authenticate)
                {
                    break (endpoint, setup.can_install);
                }
            }
            changes.next().await.ok_or(ClientError::Disconnected)?;
        };
        let input = ProviderSetupInput {
            instance_id: id.clone(),
        };
        let mut installs = endpoint.subscribe::<WsProviderInstallSubscribeRpc>(input.clone());
        let current = installs.next().await.ok_or(ClientError::Disconnected)??;
        let update = current.source.as_ref().and_then(Option::as_ref)
            != Some(&ProviderInstallSource::Local)
            && current.version.is_some()
            && current.version != current.installed_version;
        if current.installed_version.is_some() && !update {
            return Ok(endpoint);
        }
        if !can_install {
            return Err(ClientError::Unsupported("managed Codex installation"));
        }
        let mut state = endpoint.call::<WsProviderInstallStartRpc>(input).await?;
        let operation = state.operation_id.clone();
        loop {
            use ProviderInstallPhase::*;
            if state.operation_id == operation
                && !matches!(state.phase, Downloading | Extracting | Verifying)
            {
                return if !matches!(state.phase, Failed | Cancelled)
                    && state.installed_version.is_some()
                {
                    Ok(endpoint)
                } else {
                    Err(ClientError::Rejected {
                        message: state
                            .message
                            .unwrap_or_else(|| "Could not install Codex.".into()),
                        detail: None,
                    })
                };
            }
            state = installs.next().await.ok_or(ClientError::Disconnected)??;
        }
    }

    pub async fn sign_in_page(
        &self,
        environment: &EnvironmentId,
        id: t3_contracts::provider_instance::ProviderInstanceId,
    ) -> Result<String, ClientError> {
        use t3_contracts::{
            base_schemas::TrimmedNonEmptyString,
            provider_setup::*,
            rpc::{WsProviderAuthStartRpc, WsProviderAuthSubscribeRpc},
        };
        if !self.local_sign_in(environment) {
            return Err(ClientError::Unsupported("remote ChatGPT sign-in handoff"));
        }
        let endpoint = self.prepare_managed_codex(environment, &id).await?;
        let started = endpoint
            .call::<WsProviderAuthStartRpc>(ProviderAuthStartInput {
                instance_id: id.clone(),
                method_id: Some(TrimmedNonEmptyString("chatgpt".into())),
                return_url: None,
                callback_mode: Some(ProviderAuthCallbackMode::Server),
            })
            .await?;
        let flow = started.flow_id.clone().ok_or(ClientError::Cancelled)?;
        let mut updates = stream::once(async { Ok(started) })
            .chain(
                endpoint.subscribe::<WsProviderAuthSubscribeRpc>(ProviderSetupInput {
                    instance_id: id,
                }),
            )
            .boxed_local();
        while let Some(state) = updates.next().await {
            let state = state?;
            if state.flow_id.as_ref() != Some(&flow) {
                continue;
            }
            if state.phase == ProviderAuthPhase::Waiting {
                if let Some(Some(ProviderAuthInteraction::Browser { url, .. })) = state.interaction
                {
                    return Ok(url.0);
                }
                if let Some(url) = state.authorization_url {
                    return Ok(url);
                }
            }
            if matches!(
                state.phase,
                ProviderAuthPhase::Failed
                    | ProviderAuthPhase::Cancelled
                    | ProviderAuthPhase::Succeeded
            ) {
                return Err(ClientError::Rejected {
                    message: state.message.unwrap_or_else(|| "Sign-in ended.".into()),
                    detail: None,
                });
            }
        }
        Err(ClientError::Disconnected)
    }

    pub async fn refresh_providers(
        &self,
        environment: &EnvironmentId,
        instance: Option<&t3_contracts::provider_instance::ProviderInstanceId>,
    ) -> Result<(), ClientError> {
        use t3_contracts::rpc::WsServerRefreshProvidersRpc;
        let input = match instance {
            Some(id) => serde_json::json!({"instanceId":id}),
            None => serde_json::json!({}),
        };
        self.endpoint(environment)?
            .call::<WsServerRefreshProvidersRpc>(serde_json::from_value(input)?)
            .await?;
        Ok(())
    }

    /// Mutate the selected instance only. Other computers can add instances while setup is open.
    pub async fn enable_provider(
        &self,
        environment: &EnvironmentId,
        id: &t3_contracts::provider_instance::ProviderInstanceId,
        setup_mode: Option<CodexSetupMode>,
    ) -> Result<(), ClientError> {
        use t3_contracts::{
            provider_instance::{ProviderInstanceConfig, ProviderInstanceMutation},
            rpc::{WsServerUpdateSettingsPayload, WsServerUpdateSettingsRpc},
        };
        let config = self
            .0
            .store()
            .snapshot()
            .environments
            .get(environment)
            .and_then(|env| env.config.clone())
            .ok_or(ClientError::Disconnected)?;
        let provider = config
            .providers
            .0
            .iter()
            .find(|provider| &provider.instance_id == id)
            .ok_or(ClientError::Disconnected)?;
        let base = config
            .settings
            .provider_instances
            .get(id)
            .cloned()
            .unwrap_or_else(|| ProviderInstanceConfig {
                driver: provider.driver.clone(),
                enabled: None,
                display_name: None,
                accent_color: None,
                config: None,
                environment: None,
            });
        let mut instance = crate::view_logic::provider_readiness::turned_on_instance(&base);
        if let Some(mode) = setup_mode {
            if provider.driver.0 != "codex" {
                return Err(ClientError::Unsupported("Codex setup"));
            }
            let mut config = instance
                .config
                .take()
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
            config.insert("enabled".into(), true.into());
            config.insert("setupMode".into(), mode.as_str().into());
            instance.config = Some(config.into());
        }
        self.management_endpoint(environment)?
            .call::<WsServerUpdateSettingsRpc>(WsServerUpdateSettingsPayload {
                patch: Default::default(),
                provider_instance_mutation: Some(ProviderInstanceMutation::Upsert {
                    instance_id: id.clone(),
                    instance,
                }),
            })
            .await?;
        let _ = self.refresh_providers(environment, Some(id)).await;
        Ok(())
    }

    pub fn setup_terminal_id(&self, driver: &str) -> Result<String, ClientError> {
        Ok(format!(
            "onboarding-{driver}-{}",
            crate::operations::allocate_command_id(&self.0.inner.platform, None)?.0
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupTerminalState {
    Preparing,
    Ready,
    OpenFailed,
    WriteFailed,
}

impl<P: Platform + Clone + 'static> FirstRun<P> {
    /// The view owns the returned stream. Closing it during an in-flight open still closes
    /// the PTY once the server acknowledges it, without typing into an abandoned session.
    pub fn setup_terminal(
        &self,
        key: ThreadKey,
        id: String,
        cwd: String,
        instance: t3_contracts::provider_instance::ProviderInstanceId,
        command: String,
    ) -> LocalBoxStream<'static, SetupTerminalState> {
        use futures::channel::oneshot;
        use t3_contracts::{base_schemas::TrimmedNonEmptyString as S, terminal::TerminalOpenInput};
        let sessions = self.0.terminals();
        let (cancel, closed) = oneshot::channel::<()>();
        let (out, events) = mpsc::unbounded();
        (self.0.inner.spawn)(
            async move {
                if out.is_closed() {
                    return;
                }
                let input = TerminalOpenInput {
                    thread_id: S(key.thread.0.clone()),
                    terminal_id: S(id.clone()),
                    cwd: S(cwd),
                    provider_instance_id: Some(Some(instance)),
                    worktree_path: None,
                    cols: None,
                    rows: None,
                    env: None,
                    stop_on_settle: None,
                };
                if sessions.open(&key, input).await.is_err() {
                    let _ = out.unbounded_send(SetupTerminalState::OpenFailed);
                    return;
                }
                if !out.is_closed() {
                    let state = if !command.chars().any(char::is_control)
                        && sessions.write(&key, &id, command).await.is_ok()
                    {
                        SetupTerminalState::Ready
                    } else {
                        SetupTerminalState::WriteFailed
                    };
                    let _ = out.unbounded_send(state);
                    let _ = closed.await;
                }
                if sessions.can_operate(&key.environment) {
                    let _ = sessions.close(&key, &id, true).await;
                }
            }
            .boxed_local(),
        );
        stream::unfold((events, cancel), |(mut events, cancel)| async move {
            events.next().await.map(|event| (event, (events, cancel)))
        })
        .boxed_local()
    }
}

impl<P: Platform + Clone + 'static> FirstRun<P> {
    pub async fn search_registry(
        &self,
        environment: &EnvironmentId,
        query: &str,
    ) -> Result<Vec<t3_contracts::acp_registry::AcpRegistrySearchAgent>, ClientError> {
        use t3_contracts::{
            acp_registry::AcpRegistrySearchInput, base_schemas::TrimmedString,
            rpc::WsServerSearchAcpRegistryRpc,
        };
        let query = query.trim();
        let response = self
            .endpoint(environment)?
            .call::<WsServerSearchAcpRegistryRpc>(AcpRegistrySearchInput {
                query: TrimmedString(query.into()),
                limit: query.is_empty().then_some(Some(512)),
            })
            .await?;
        Ok(response.agents)
    }
    pub async fn prepare_registry_agent(
        &self,
        environment: &EnvironmentId,
        agent: &t3_contracts::acp_registry::AcpRegistrySearchAgent,
    ) -> Result<(), ClientError> {
        use t3_contracts::{
            acp_registry::AcpRegistryPrepareInput, rpc::WsServerPrepareAcpRegistryAgentRpc,
        };
        self.management_endpoint(environment)?
            .call::<WsServerPrepareAcpRegistryAgentRpc>(AcpRegistryPrepareInput {
                agent_id: agent.id.clone(),
            })
            .await?;
        Ok(())
    }

    pub async fn add_registry_agent(
        &self,
        environment: &EnvironmentId,
        agent: &t3_contracts::acp_registry::AcpRegistrySearchAgent,
        label: &str,
    ) -> Result<(), ClientError> {
        use t3_contracts::{
            provider_instance::{ProviderInstanceId, ProviderInstanceMutation},
            rpc::{WsServerUpdateSettingsPayload, WsServerUpdateSettingsRpc},
        };
        let label = label.trim();
        let slug = label
            .to_ascii_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("_");
        if slug.is_empty() {
            return Err(ClientError::InvalidPayload("Enter an instance ID.".into()));
        }
        let base = format!("acpRegistry_{}", &slug[..slug.len().min(48)]);
        let config = self
            .0
            .store()
            .snapshot()
            .environments
            .get(environment)
            .and_then(|env| env.config.clone())
            .ok_or(ClientError::Disconnected)?;
        let occupied = |id: &str| {
            config
                .settings
                .provider_instances
                .keys()
                .any(|key| key.0 == id)
                || config.providers.0.iter().any(|p| p.instance_id.0 == id)
        };
        let mut id = base.clone();
        for n in 2.. {
            if !occupied(&id) {
                break;
            }
            let suffix = format!("_{n}");
            id = format!("{}{suffix}", &base[..base.len().min(64 - suffix.len())]);
        }
        let instance = serde_json::from_value(
            serde_json::json!({"driver":"acpRegistry","enabled":true,"displayName":label,"config":{"agentId":agent.id,"distribution":"auto"}}),
        )?;
        self.management_endpoint(environment)?
            .call::<WsServerUpdateSettingsRpc>(WsServerUpdateSettingsPayload {
                patch: Default::default(),
                provider_instance_mutation: Some(ProviderInstanceMutation::Create {
                    instance_id: ProviderInstanceId(id),
                    instance,
                }),
            })
            .await?;
        let _ = self.refresh_providers(environment, None).await;
        Ok(())
    }
}

fn disconnected_shell_is_settled(state: &ConnectionState) -> bool {
    match state.phase {
        Phase::Blocked | Phase::Offline => true,
        Phase::Backoff => !state.desired || state.attempt > 2,
        Phase::Available => true,
        Phase::Connecting | Phase::Connected => false,
    }
}

#[cfg(test)]
mod bootstrap_tests {
    use super::*;

    #[test]
    fn shell_bootstrap_uses_the_catalog_connection_projection() {
        for (phase, desired, attempt, settled) in [
            (Phase::Available, true, 0, true),
            (Phase::Available, false, 0, true),
            (Phase::Connecting, true, 1, false),
            (Phase::Connected, true, 1, false),
            (Phase::Backoff, true, 2, false),
            (Phase::Backoff, false, 1, true),
            (Phase::Backoff, true, 3, true),
            (Phase::Blocked, true, 1, true),
            (Phase::Offline, true, 1, true),
        ] {
            let state = ConnectionState {
                phase,
                desired,
                attempt,
                ..Default::default()
            };
            assert_eq!(disconnected_shell_is_settled(&state), settled, "{state:?}");
        }
    }
}
