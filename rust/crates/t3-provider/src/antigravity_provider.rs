use crate::{
    antigravity_catalog::{self, ThinkingLevels},
    antigravity_profile::AuthConfig,
    antigravity_runtime::{AntigravityRuntimeFactory, Catalog, Started},
    antigravity_usage::AntigravityUsage,
    catalog::{self, ManifestSource},
    instances::InstanceSpec,
    native_provider::NativeProviderContext,
    sensitive::{ServiceError, ServiceResult},
    service::ProviderInstanceService,
    transport::ProviderClock,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex as StdMutex, Weak},
    time::Duration,
};
use t3_contracts::{
    provider_instance::ProviderInstanceId,
    server::{ServerProvider, ServerProviderAuthStatus, ServerProviderState},
};
use tokio::sync::{Mutex, watch};
struct Task(tokio::task::JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}
struct State {
    snapshot: ServerProvider,
    revision: u64,
    thinking: ThinkingLevels,
}
pub struct AntigravityProvider {
    spec: Arc<InstanceSpec>,
    context: NativeProviderContext,
    auth: AuthConfig,
    factory: Arc<dyn AntigravityRuntimeFactory>,
    clock: Arc<dyn ProviderClock>,
    manifest: ManifestSource,
    state: Mutex<State>,
    changes: watch::Sender<Option<ServerProvider>>,
    usage: Option<Arc<AntigravityUsage>>,
    usage_task: StdMutex<Option<Task>>,
    self_ref: Weak<Self>,
    auth_controller: tokio::sync::RwLock<Weak<crate::auth::AuthController>>,
    process_gate: Arc<tokio::sync::RwLock<()>>,
}
impl AntigravityProvider {
    pub fn new(
        spec: Arc<InstanceSpec>,
        context: NativeProviderContext,
        factory: Arc<dyn AntigravityRuntimeFactory>,
        clock: Arc<dyn ProviderClock>,
        manifest: ManifestSource,
        usage: Option<Arc<AntigravityUsage>>,
    ) -> ServiceResult<Arc<Self>> {
        let auth = AuthConfig::from_settings(&spec.config);
        let checked = stamp(clock.now_ms())?;
        let mut value = json!({"instanceId":spec.instance_id,"driver":"antigravity","displayName":spec.display_name.as_deref().unwrap_or("Antigravity"),"enabled":spec.enabled,"installed":false,"version":null,"status":if spec.enabled{"warning"}else{"disabled"},"auth":{"status":"unknown","type":auth.method},"setup":{"canAuthenticate":true,"canInstall":true},"checkedAt":checked,"models":[],"skills":[],"slashCommands":[],"showInteractionModeToggle":false,"supportedRuntimeModes":["approval-required","auto-accept-edits","full-access"],"supportsConversationRollback":false,"supportsTextGeneration":false,"workspaceSnapshots":[],"continuation":{"groupKey":spec.default_continuation_key()},"message":if spec.enabled{"Checking Antigravity availability."}else{"Antigravity is disabled in T3 Code settings."}});
        if let Some(color) = &spec.accent_color {
            value["accentColor"] = json!(color);
        }
        let snapshot: ServerProvider =
            serde_json::from_value(value).map_err(|_| ServiceError::Response)?;
        let (changes, _) = watch::channel(Some(snapshot.clone()));
        Ok(Arc::new_cyclic(|weak| Self {
            spec,
            context,
            auth,
            factory,
            clock,
            manifest,
            state: Mutex::new(State {
                snapshot,
                revision: 0,
                thinking: ThinkingLevels::new(),
            }),
            changes,
            usage,
            usage_task: StdMutex::new(None),
            self_ref: weak.clone(),
            auth_controller: tokio::sync::RwLock::new(Weak::new()),
            process_gate: Arc::new(tokio::sync::RwLock::new(())),
        }))
    }
    pub async fn current(&self) -> ServerProvider {
        self.state.lock().await.snapshot.clone()
    }
    pub async fn bind_auth(&self, controller: &Arc<crate::auth::AuthController>) {
        *self.auth_controller.write().await = Arc::downgrade(controller);
    }
    pub async fn setup_guard(&self) -> tokio::sync::OwnedRwLockWriteGuard<()> {
        self.process_gate.clone().write_owned().await
    }
    pub async fn into_managed(
        self: Arc<Self>,
        options: crate::managed_provider::ManagedOptions,
    ) -> ServiceResult<Arc<crate::managed_provider::ManagedProvider>> {
        let initial = self.current().await;
        let background = options.background.clone();
        let manifest = self.manifest.subscribe();
        let provider = crate::managed_provider::ManagedProvider::new(self, initial, options)?;
        provider.recheck_on_manifest(manifest, background);
        Ok(provider)
    }
    fn publish(&self, state: &State) {
        self.changes.send_replace(Some(state.snapshot.clone()));
    }
    fn sign_in_message(&self) -> String {
        if self.auth.uses_browser() {
            "Sign in with Google to use Antigravity.".into()
        } else {
            format!(
                "Connect with {} to use Antigravity and load models.",
                self.auth.label()
            )
        }
    }
    fn unchecked_message(&self) -> String {
        if self.auth.uses_browser() {
            "Antigravity is installed. Google account access is not checked yet.".into()
        } else {
            format!(
                "Antigravity is installed. Connect with {} to load models.",
                self.auth.label()
            )
        }
    }
    fn classify(&self, snapshot: &mut ServerProvider) {
        let manifest = self.manifest.current();
        let catalog = manifest.catalog("antigravity", self.clock.now_ms());
        snapshot.models = catalog::apply_catalog(&snapshot.models, catalog.as_ref(), |model| model);
        snapshot.compatibility_advisory = crate::versions::resolve_compatibility(
            manifest.compatibility.as_deref().unwrap_or_default(),
            "antigravity",
            snapshot.version.as_ref().map(|version| version.0.as_str()),
            &self.context.server_version,
        );
    }
    async fn check(&self) -> ServiceResult<ServerProvider> {
        let before = self.state.lock().await.revision;
        if !self.spec.enabled {
            return Ok(self.current().await);
        }
        let controller = self.auth_controller.read().await.upgrade();
        let access = if let Some(controller) = controller {
            match controller.admit_access().await {
                Ok(access) => Some(access),
                Err(ServiceError::Busy) => return Ok(self.current().await),
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        let _permit = self.process_gate.read().await;
        let environment = self.spec.environment(
            &self.context.environment,
            &self.context.home,
            self.context.platform,
        );
        let timeout = Duration::from_millis(crate::versions::probe_timeout_ms(
            "antigravity",
            90_000,
            &environment,
        ));
        let result = tokio::time::timeout(timeout, async {
            if self.auth.issue().is_some() { return Err(ServiceError::Identity); }
            if let Some(access)=&access {tokio::select!{biased;_=access.cancellation.cancelled()=>Err(ServiceError::Cancelled),result=self.factory.health()=>result}}else{self.factory.health().await}
        })
        .await;
        if matches!(&result, Ok(Err(ServiceError::Cancelled))) {
            return Ok(self.current().await);
        }
        let supports_text_generation = if matches!(&result, Ok(Ok(_))) {
            self.factory.supports_text_generation().await
        } else {
            false
        };
        let mut state = self.state.lock().await;
        if state.revision != before {
            return Ok(state.snapshot.clone());
        }
        let missing = matches!(&result, Ok(Err(ServiceError::Unavailable)));
        let initialized = result.as_ref().ok().and_then(|result| result.as_ref().ok());
        let error = if let Some(issue) = self.auth.issue() {
            Some(issue.into())
        } else if missing {
            Some(
                "Antigravity ACP runtime is not installed or its executable could not be found."
                    .into(),
            )
        } else if result.is_err() {
            Some(format!(
                "Antigravity did not respond to its local health check within {} seconds.",
                timeout.as_secs_f64()
            ))
        } else if result.as_ref().is_ok_and(|result| result.is_err()) {
            Some("Antigravity could not complete its local health check.".into())
        } else {
            None
        };
        state.snapshot.installed = !missing;
        state.snapshot.checked_at = t3_contracts::wire::WireDateTime(stamp(self.clock.now_ms())?);
        if let Some(initialized) = initialized {
            if let Some(version) = initialized.expose()["agentInfo"]["version"]
                .as_str()
                .filter(|version| !version.is_empty() && *version != "unknown")
            {
                state.snapshot.version = Some(version.into());
            }
            state.snapshot.supports_text_generation = Some(Some(
                supports_text_generation
                    && state.snapshot.auth.status != ServerProviderAuthStatus::Unauthenticated,
            ));
        }
        let authenticated = state.snapshot.auth.status == ServerProviderAuthStatus::Authenticated;
        state.snapshot.status = if error.is_some() {
            ServerProviderState::Error
        } else if authenticated {
            ServerProviderState::Ready
        } else {
            ServerProviderState::Warning
        };
        state.snapshot.message = error
            .or_else(|| {
                if authenticated {
                    None
                } else if state.snapshot.auth.status == ServerProviderAuthStatus::Unauthenticated {
                    Some(self.sign_in_message())
                } else {
                    Some(self.unchecked_message())
                }
            })
            .map(|message| Some(message.into()));
        if missing {
            state.snapshot.models.clear();
            state.snapshot.slash_commands.clear();
            state.snapshot.skills.clear();
            state.snapshot.workspace_snapshots = Some(vec![]);
            state.snapshot.supports_text_generation = Some(Some(false));
            state.snapshot.usage_limits = None;
        }
        self.classify(&mut state.snapshot);
        self.publish(&state);
        let snapshot = state.snapshot.clone();
        drop(state);
        if initialized.is_some() {
            self.refresh_usage();
        }
        Ok(snapshot)
    }
    pub async fn session_started(&self, started: Started, cwd: Option<&str>) -> ServiceResult<()> {
        self.catalog_started(started, &Catalog::default(), cwd, None)
            .await
    }
    pub async fn catalog_started(
        &self,
        started: Started,
        catalog: &Catalog,
        cwd: Option<&str>,
        expected_revision: Option<u64>,
    ) -> ServiceResult<()> {
        if !self.spec.enabled {
            return Ok(());
        }
        let supports_text_generation = self.factory.supports_text_generation().await;
        let mut state = self.state.lock().await;
        if expected_revision.is_some_and(|revision| revision != state.revision) {
            return Err(ServiceError::Cancelled);
        }
        state.revision = state.revision.wrapping_add(1);
        for options in &catalog.config_updates {
            antigravity_catalog::observe(options.expose(), &mut state.thinking);
        }
        if !catalog.commands.is_empty() {
            state.snapshot.slash_commands = antigravity_catalog::commands(&catalog.commands);
        }
        let options = started.setup.expose()["configOptions"]
            .as_array()
            .map_or(&[][..], Vec::as_slice);
        antigravity_catalog::observe(options, &mut state.thinking);
        state.snapshot.models =
            antigravity_catalog::models(started.setup.expose(), &state.thinking);
        state.snapshot.installed = true;
        state.snapshot.status = ServerProviderState::Ready;
        state.snapshot.auth = serde_json::from_value(
            json!({"status":"authenticated","type":self.auth.method,"label":self.auth.label()}),
        )
        .map_err(|_| ServiceError::Response)?;
        state.snapshot.message = None;
        state.snapshot.checked_at = t3_contracts::wire::WireDateTime(stamp(self.clock.now_ms())?);
        state.snapshot.supports_text_generation = Some(Some(supports_text_generation));
        if let Some(version) = started.initialize.expose()["agentInfo"]["version"]
            .as_str()
            .filter(|version| !version.is_empty())
        {
            state.snapshot.version = Some(version.into());
        }
        if let Some(cwd) = cwd {
            self.workspace(&mut state.snapshot, cwd, None, None)?;
        }
        self.classify(&mut state.snapshot);
        self.publish(&state);
        drop(state);
        self.refresh_usage();
        Ok(())
    }
    pub async fn config_updated(&self, options: &[Value]) {
        let mut state = self.state.lock().await;
        antigravity_catalog::observe(options, &mut state.thinking);
        if state.snapshot.auth.status != ServerProviderAuthStatus::Authenticated {
            return;
        }
        state.snapshot.models =
            antigravity_catalog::models(&json!({"configOptions":options}), &state.thinking);
        self.classify(&mut state.snapshot);
        self.publish(&state);
    }
    fn workspace(
        &self,
        snapshot: &mut ServerProvider,
        cwd: &str,
        commands: Option<Vec<t3_contracts::server::ServerProviderSlashCommand>>,
        skills: Option<Vec<t3_contracts::server::ServerProviderSkill>>,
    ) -> ServiceResult<()> {
        let previous = snapshot
            .workspace_snapshots
            .as_ref()
            .and_then(|workspaces| workspaces.iter().find(|workspace| workspace.cwd.0 == cwd));
        let commands = commands
            .or_else(|| previous.map(|workspace| workspace.slash_commands.clone()))
            .unwrap_or_else(|| snapshot.slash_commands.clone());
        let skills = skills
            .or_else(|| previous.map(|workspace| workspace.skills.clone()))
            .unwrap_or_default();
        let workspace=serde_json::from_value(json!({"cwd":cwd,"checkedAt":stamp(self.clock.now_ms())?,"slashCommands":commands,"skills":skills})).map_err(|_|ServiceError::Response)?;
        let workspaces = snapshot.workspace_snapshots.get_or_insert_default();
        workspaces.retain(|workspace| workspace.cwd.0 != cwd);
        workspaces.push(workspace);
        if workspaces.len() > 32 {
            workspaces.remove(0);
        }
        Ok(())
    }
    pub async fn commands_updated(
        &self,
        commands: &[Value],
        cwd: Option<&str>,
    ) -> ServiceResult<()> {
        if !self.spec.enabled {
            return Ok(());
        }
        let mut state = self.state.lock().await;
        if state.snapshot.auth.status == ServerProviderAuthStatus::Unauthenticated {
            return Ok(());
        }
        let commands = antigravity_catalog::commands(commands);
        state.snapshot.slash_commands = commands.clone();
        if let Some(cwd) = cwd {
            self.workspace(&mut state.snapshot, cwd, Some(commands), None)?;
        }
        self.publish(&state);
        Ok(())
    }
    pub async fn signed_out(&self) -> ServiceResult<()> {
        self.clear_usage().await;
        if !self.spec.enabled {
            return Ok(());
        }
        let mut state = self.state.lock().await;
        state.revision = state.revision.wrapping_add(1);
        state.thinking.clear();
        state.snapshot.auth = serde_json::from_value(if self.auth.uses_browser() {
            json!({"status":"unauthenticated"})
        } else {
            json!({"status":"unauthenticated","type":self.auth.method,"label":self.auth.label()})
        })
        .map_err(|_| ServiceError::Response)?;
        state.snapshot.status = ServerProviderState::Warning;
        state.snapshot.message = Some(Some(self.sign_in_message().into()));
        state.snapshot.checked_at = t3_contracts::wire::WireDateTime(stamp(self.clock.now_ms())?);
        state.snapshot.models.clear();
        state.snapshot.slash_commands.clear();
        state.snapshot.skills.clear();
        state.snapshot.workspace_snapshots = Some(vec![]);
        state.snapshot.supports_text_generation = Some(Some(false));
        state.snapshot.usage_limits = None;
        self.publish(&state);
        Ok(())
    }
    pub async fn signed_in(&self) -> ServiceResult<()> {
        self.clear_usage().await;
        let mut state = self.state.lock().await;
        state.revision = state.revision.wrapping_add(1);
        state.snapshot.usage_limits = None;
        self.publish(&state);
        Ok(())
    }
    async fn clear_usage(&self) {
        self.usage_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(usage) = &self.usage {
            usage.clear().await;
        }
    }
    pub fn refresh_usage(&self) {
        let Some(usage) = self.usage.clone() else {
            return;
        };
        let mut task = self
            .usage_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if task.as_ref().is_some_and(|task| !task.0.is_finished()) {
            return;
        }
        let weak = self.self_ref.clone();
        *task = Some(Task(tokio::spawn(async move {
            let Some(provider) = weak.upgrade() else {
                return;
            };
            let revision = provider.state.lock().await.revision;
            drop(provider);
            let Ok(limits) = usage.probe().await else {
                return;
            };
            let Some(provider) = weak.upgrade() else {
                return;
            };
            let mut state = provider.state.lock().await;
            if revision != state.revision {
                return;
            }
            let previous = state
                .snapshot
                .usage_limits
                .as_ref()
                .and_then(Option::as_ref)
                .cloned()
                .map(Arc::new);
            let probed = Arc::new(limits);
            state.snapshot.usage_limits =
                crate::usage::usage_after_probe(previous.as_ref(), Some(&probed))
                    .map(|limits| Some((*limits).clone()));
            provider.publish(&state);
        })));
    }
}
fn stamp(ms: i64) -> ServiceResult<String> {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or(ServiceError::Response)
}
#[async_trait]
impl ProviderInstanceService for AntigravityProvider {
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.spec.instance_id
    }
    fn owns_usage_state(&self) -> bool {
        true
    }
    fn subscribe_snapshots(&self) -> Option<watch::Receiver<Option<ServerProvider>>> {
        Some(self.changes.subscribe())
    }
    fn catalogs_per_cwd(&self) -> bool {
        true
    }
    async fn snapshot(&self, cwd: Option<&str>, _fresh: bool) -> ServiceResult<ServerProvider> {
        if let Some(cwd) = cwd {
            if !self.spec.enabled {
                return Ok(self.current().await);
            }
            let root = std::path::PathBuf::from(cwd);
            let environment = self.spec.environment(
                &self.context.environment,
                &self.context.home,
                self.context.platform,
            );
            let home = crate::antigravity_profile::user_home(
                self.context.platform,
                &environment,
                &self.context.home,
            );
            let skills = tokio::task::spawn_blocking(move || {
                crate::antigravity_skills::discover(&root, &home)
            })
            .await
            .map_err(|_| ServiceError::Storage)??;
            let mut state = self.state.lock().await;
            self.workspace(&mut state.snapshot, cwd, None, Some(skills))?;
            self.publish(&state);
            Ok(state.snapshot.clone())
        } else {
            self.check().await
        }
    }
    async fn refresh_models(&self) -> ServiceResult<()> {
        if !self.spec.enabled {
            return Ok(());
        }
        let controller = self.auth_controller.read().await.upgrade();
        let access = if let Some(controller) = controller {
            Some(controller.admit_access().await?)
        } else {
            None
        };
        let _permit = self.process_gate.read().await;
        let revision = self.state.lock().await.revision;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        let runtime = tokio::time::timeout_at(deadline, self.factory.create(None))
            .await
            .map_err(|_| ServiceError::Expired)??;
        let work = async {
            let started = runtime.start().await?;
            let catalog = runtime.catalog(&started).await?;
            Ok::<_, ServiceError>((started, catalog))
        };
        let result = tokio::time::timeout_at(deadline, async {
            if let Some(access) = &access {
                tokio::select! {biased;_=access.cancellation.cancelled()=>Err(ServiceError::Cancelled),result=work=>result}
            } else { work.await }
        }).await.map_err(|_| ServiceError::Expired);
        runtime.close().await;
        let (started, catalog) = result??;
        self.catalog_started(started, catalog.expose(), None, Some(revision))
            .await
    }
    async fn invalidate_usage_cache(&self) -> ServiceResult<()> {
        self.clear_usage().await;
        self.refresh_usage();
        Ok(())
    }
    async fn maintenance(
        &self,
        _fresh: bool,
    ) -> ServiceResult<crate::maintenance::MaintenanceCapabilities> {
        Ok(crate::maintenance::MaintenanceCapabilities {
            driver: "antigravity".into(),
            package: None,
            update: None,
            install_path: None,
            manual_command: None,
        })
    }
}
