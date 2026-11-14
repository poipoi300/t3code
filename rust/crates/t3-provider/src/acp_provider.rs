use crate::{
    acp_auth_state::AcpAuthenticationState,
    acp_coordinator::LiveConfiguration,
    acp_probe::{AcpNativeServices, AcpProbeFailure, AcpProbeOutcome},
    catalog,
    instances::InstanceSpec,
    registry_service::{InspectionStatus, RegistryInspection},
    sensitive::{ServiceError, ServiceResult},
    service::ProviderInstanceService,
    transport::ProviderClock,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex, Weak},
};
use t3_contracts::{
    acp_registry::AcpRegistryOperationErrorReason as Reason,
    provider_instance::ProviderInstanceId,
    server::{ServerProvider, ServerProviderAuthStatus, ServerProviderModel, ServerProviderState},
};
use tokio::sync::{Mutex, watch};
struct Task(tokio::task::JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}
#[derive(Default)]
struct State {
    current: Option<ServerProvider>,
    cache: Option<(i64, ServerProvider)>,
    learned: Vec<ServerProviderModel>,
    generation: u64,
    probing: bool,
    probe: Option<Task>,
}
pub struct AcpProvider {
    spec: Arc<InstanceSpec>,
    native: Arc<AcpNativeServices>,
    confirmed: Arc<AcpAuthenticationState>,
    clock: Arc<dyn ProviderClock>,
    cwd: PathBuf,
    state: Mutex<State>,
    changes: watch::Sender<Option<ServerProvider>>,
    self_ref: Weak<Self>,
    watcher: StdMutex<Option<Task>>,
}
pub fn models(
    discovery: Option<&LiveConfiguration>,
    custom: &[Value],
    previous: &[ServerProviderModel],
) -> Vec<ServerProviderModel> {
    let all = catalog::capabilities(
        discovery
            .map(|discovery| discovery.config_options.clone())
            .unwrap_or_default(),
    );
    let specific = discovery
        .and_then(|discovery| discovery.model_option_ids.as_ref())
        .into_iter()
        .flatten()
        .collect::<BTreeSet<_>>();
    let shared = discovery
        .map(|discovery| {
            discovery
                .config_options
                .iter()
                .filter(|descriptor| !specific.contains(&catalog::option_id(descriptor).to_owned()))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let shared_ids = shared
        .iter()
        .map(catalog::option_id)
        .collect::<BTreeSet<_>>();
    let discovered = discovery
        .map(|discovery| discovery.models.as_slice())
        .unwrap_or_default();
    let mut built_in = vec![];
    if discovered.is_empty() {
        let mut model = catalog::model("default", "Default", Some(all.clone()));
        model.is_default = Some(Some(true));
        built_in.push(model);
    } else {
        for native in discovered {
            let current = discovery.and_then(|discovery| discovery.current_model_id.as_deref())
                == Some(native.id.0.as_str());
            let capabilities = if current {
                all.clone()
            } else {
                let mut descriptors = shared.clone();
                descriptors.extend(
                    previous
                        .iter()
                        .find(|model| model.slug == native.id)
                        .and_then(|model| model.capabilities.as_ref())
                        .map(catalog::descriptors)
                        .unwrap_or_default()
                        .iter()
                        .filter(|descriptor| !shared_ids.contains(catalog::option_id(descriptor)))
                        .cloned(),
                );
                catalog::capabilities(descriptors)
            };
            let mut model = catalog::model(&native.id.0, &native.name.0, Some(capabilities));
            if current {
                model.is_default = Some(Some(true));
            }
            built_in.push(model);
        }
    }
    catalog::custom_models(&built_in, custom, Some(&all))
}
fn icon(agent: &str, reported: Option<&str>) -> Option<String> {
    if let Some(url) = reported
        .and_then(|url| url::Url::parse(url).ok())
        .filter(|url| {
            url.scheme() == "https"
                && url.host_str() == Some("cdn.agentclientprotocol.com")
                && url.port().is_none()
                && url.username().is_empty()
                && url.password().is_none()
        })
    {
        return Some(url.into());
    }
    regex::Regex::new(r"^[a-z0-9][a-z0-9._-]*$")
        .expect("registry agent id")
        .is_match(agent)
        .then(|| format!("https://cdn.agentclientprotocol.com/registry/v1/latest/{agent}.svg"))
}
impl AcpProvider {
    pub async fn new(
        spec: Arc<InstanceSpec>,
        native: Arc<AcpNativeServices>,
        confirmed: Arc<AcpAuthenticationState>,
        clock: Arc<dyn ProviderClock>,
        cwd: PathBuf,
    ) -> Arc<Self> {
        let (changes, _) = watch::channel(None);
        let provider = Arc::new_cyclic(|self_ref| Self {
            spec,
            native,
            confirmed,
            clock,
            cwd,
            state: Mutex::new(State::default()),
            changes,
            self_ref: self_ref.clone(),
            watcher: StdMutex::new(None),
        });
        let coordinator = &provider.native.coordinator;
        let mut commands = coordinator.subscribe_commands(provider.spec.instance_id.clone());
        let mut configuration =
            coordinator.subscribe_configuration(provider.spec.instance_id.clone());
        let mut action = coordinator.subscribe_url_auth(provider.spec.instance_id.clone());
        let mut confirmed = provider.confirmed.subscribe();
        let weak = Arc::downgrade(&provider);
        let task = tokio::spawn(async move {
            loop {
                let changed = tokio::select! {value=commands.recv()=>value.map(|_|false),value=configuration.recv()=>value.map(|_|false),value=action.recv()=>value.map(|_|false),value=confirmed.changed()=>value.ok().map(|_|{confirmed.borrow_and_update();true})};
                let Some(auth_changed) = changed else {
                    break;
                };
                let Some(provider) = weak.upgrade() else {
                    break;
                };
                if auth_changed {
                    let _ = provider.invalidate_models(None, true).await;
                    if !provider.confirmed.get() {
                        provider
                            .native
                            .coordinator
                            .clear_commands(&provider.spec.instance_id);
                        provider
                            .native
                            .coordinator
                            .clear_configuration(&provider.spec.instance_id);
                        provider.state.lock().await.learned.clear();
                    }
                    let _ = provider.snapshot(None, false).await;
                } else {
                    let mut state = provider.state.lock().await;
                    if let Some(mut current) = state.current.clone() {
                        provider.live(&mut current, &mut state.learned).await;
                        state.current = Some(current.clone());
                        provider.changes.send_replace(Some(current));
                    }
                }
            }
        });
        *provider
            .watcher
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Task(task));
        provider
    }
    fn custom(&self) -> Vec<Value> {
        self.spec.config["customModels"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }
    fn checked(&self) -> ServiceResult<String> {
        chrono::DateTime::from_timestamp_millis(self.clock.now_ms())
            .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            .ok_or(ServiceError::Response)
    }
    fn base(&self, inspection: Option<&RegistryInspection>) -> ServiceResult<ServerProvider> {
        let mut value = json!({"instanceId":self.spec.instance_id,"driver":"acpRegistry","enabled":self.spec.enabled,"installed":false,"version":null,"status":if self.spec.enabled{"warning"}else{"disabled"},"supportsTextGeneration":false,"auth":{"status":"unknown","canLogout":false},"checkedAt":self.checked()?,"continuation":{"groupKey":self.spec.default_continuation_key()},"setup":{"canInstall":false,"canAuthenticate":false},"models":models(None,&self.custom(),&[]),"slashCommands":[],"skills":[]});
        if let Some(name) = self.spec.display_name.as_ref() {
            value["displayName"] = json!(name);
        }
        if let Some(color) = self.spec.accent_color.as_ref() {
            value["accentColor"] = json!(color);
        }
        if self.spec.config["source"] != "local"
            && let Some(url) = icon(
                self.spec.config["agentId"].as_str().unwrap_or_default(),
                None,
            )
        {
            value["iconUrl"] = json!(url);
        }
        if !self.spec.enabled {
            value["message"] = json!("ACP Registry is disabled in T3 Code settings.");
        } else if let Some(inspection) = inspection {
            value["version"] = json!(inspection.version.as_ref().and_then(Option::as_ref));
            let id = inspection.agent_id.as_deref().unwrap_or_default();
            match inspection.status {
                InspectionStatus::Ready => {
                    value["installed"] = json!(true);
                    value["status"] = json!("ready");
                    value["message"] = json!(
                        "Checking ACP authentication, models, and commands in the background..."
                    );
                    value["setup"]["canAuthenticate"] = json!(
                        self.spec.config["source"] != "local"
                            && !self.spec.config["agentId"]
                                .as_str()
                                .unwrap_or_default()
                                .is_empty()
                    );
                    if let Some(url) = inspection.documentation_url.as_ref() {
                        value["setup"]["documentationUrl"] = json!(url);
                    }
                }
                InspectionStatus::Unconfigured => {
                    value["message"] = json!(
                        "Select an ACP Registry agent or configure a local ACP executable before starting a thread."
                    )
                }
                InspectionStatus::NotFound => {
                    value["status"] = json!("error");
                    value["message"] =
                        json!(format!("ACP Registry does not contain agent '{id}'."));
                }
                InspectionStatus::Unsupported => {
                    value["status"] = json!("error");
                    value["message"] = json!(format!(
                        "ACP Registry agent '{id}' has no compatible distribution for this environment."
                    ));
                }
                InspectionStatus::MissingRunner => {
                    value["status"] = json!("error");
                    value["message"] =
                        json!(if inspection.distribution.as_deref() == Some("local") {
                            "Local ACP executable is not available on this environment's PATH."
                                .into()
                        } else {
                            format!(
                                "ACP executable '{}' is not available on this environment's PATH.",
                                inspection.runner.as_deref().unwrap_or_default()
                            )
                        });
                }
                InspectionStatus::Unprepared => {
                    value["message"] = json!(format!(
                        "ACP Registry agent '{id}' has not been prepared on this environment."
                    ))
                }
            }
        } else {
            value["status"] = json!("error");
            value["message"] = json!("Could not inspect the ACP Registry agent.");
        }
        serde_json::from_value(value).map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Response,
        ))
    }
    async fn readiness(&self) -> ServiceResult<ServerProvider> {
        if !self.spec.enabled {
            return self.base(None);
        }
        let inspected = self
            .native
            .catalog
            .inspect(&self.native.settings, self.native.environment.expose())
            .await
            .ok();
        self.base(inspected.as_ref())
    }
    async fn live(&self, provider: &mut ServerProvider, learned: &mut Vec<ServerProviderModel>) {
        if provider.auth.status == ServerProviderAuthStatus::Unauthenticated {
            let _ = self.confirmed.set(false).await;
        }
        if provider.enabled
            && provider.installed
            && provider.auth.status == ServerProviderAuthStatus::Unknown
            && self.confirmed.get()
        {
            provider.auth.status = ServerProviderAuthStatus::Authenticated;
        }
        if let Some(commands) = self.native.coordinator.commands(&self.spec.instance_id) {
            provider.slash_commands = commands.slash_commands.clone();
            provider.skills = commands.skills.clone();
        }
        if let Some(configuration) = self
            .native
            .coordinator
            .configuration(&self.spec.instance_id)
        {
            provider.message = None;
            if provider.enabled {
                provider.status = ServerProviderState::Ready;
            }
            if !configuration.models.is_empty() || !configuration.config_options.is_empty() {
                let mut discovery = (*configuration).clone();
                if discovery.models.is_empty() {
                    discovery.models = provider
                        .models
                        .iter()
                        .filter(|model| !model.is_custom)
                        .map(|model| t3_contracts::acp_registry::AcpRegistryProbeModel {
                            id: model.slug.clone(),
                            name: model.name.clone(),
                            description: None,
                        })
                        .collect();
                    discovery.current_model_id = provider
                        .models
                        .iter()
                        .find(|model| model.is_default == Some(Some(true)))
                        .map(|model| model.slug.0.clone());
                }
                provider.models = models(
                    Some(&discovery),
                    &self.custom(),
                    if learned.is_empty() {
                        &provider.models
                    } else {
                        learned
                    },
                );
                *learned = provider.models.clone();
            }
        }
        provider.auth.action = self
            .native
            .coordinator
            .url_auth_action(&self.spec.instance_id)
            .map(|action| Some(action.into_inner()));
    }
    fn enrich(
        &self,
        mut base: ServerProvider,
        result: Result<AcpProbeOutcome, AcpProbeFailure>,
    ) -> ServiceResult<ServerProvider> {
        let mut value = serde_json::to_value(&base).map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Response,
        ))?;
        value
            .as_object_mut()
            .ok_or(ServiceError::Response)?
            .remove("message");
        match result {
            Ok(outcome) => {
                let probe = outcome.probe.expose();
                let discovery = LiveConfiguration {
                    models: probe.models.clone(),
                    current_model_id: probe.current_model_id.as_ref().map(|model| model.0.clone()),
                    config_options: probe.config_options.clone(),
                    model_option_ids: probe
                        .model_option_ids
                        .as_ref()
                        .map(|ids| ids.iter().map(|id| id.0.clone()).collect()),
                };
                value["models"] = json!(models(Some(&discovery), &self.custom(), &[]));
                value["slashCommands"] = json!(outcome.commands.slash_commands);
                value["skills"] = json!(outcome.commands.skills);
                value["auth"]["canLogout"] = json!(probe.session_management.can_logout);
                value["nativeSessions"] = json!({"canList":probe.session_management.can_list,"canLoad":probe.session_management.can_load,"canResume":probe.session_management.can_resume,"canDelete":probe.session_management.can_delete});
                value["configurableProviders"] =
                    json!(probe.session_management.can_configure_providers);
                value["setup"]["canAuthenticate"] = json!(!probe.auth_methods.is_empty());
                if self.spec.config["source"] != "local"
                    && let Some(url) = icon(
                        self.spec.config["agentId"].as_str().unwrap_or_default(),
                        probe.icon.as_deref(),
                    )
                {
                    value["iconUrl"] = json!(url);
                }
            }
            Err(failure) => {
                value["status"] = json!("warning");
                value["message"] = json!(failure.error.to_string());
                if failure.error.reason == Reason::AuthenticationFailed {
                    value["auth"]["status"] = json!("unauthenticated");
                    let methods = failure.auth_methods.expose();
                    let preferred = self.spec.config["authMethodId"]
                        .as_str()
                        .unwrap_or_default();
                    if let Some(method) = methods
                        .iter()
                        .find(|method| method.id.0 == preferred)
                        .or_else(|| methods.first())
                    {
                        value["auth"]["type"] = json!(method.r#type);
                        value["auth"]["label"] = json!(method.name);
                        value["message"]=json!(match method.r#type{t3_contracts::acp_registry::AcpRegistryProbeAuthMethodType::EnvVar if method.env_var_names.as_ref().is_some_and(|names|!names.is_empty())=>format!("Set {} under this instance's environment variables in provider settings. T3 Code will detect it on the next provider refresh.",method.env_var_names.as_ref().into_iter().flatten().map(|name|name.0.as_str()).collect::<Vec<_>>().join(", ")),t3_contracts::acp_registry::AcpRegistryProbeAuthMethodType::Terminal if method.command.is_some()=>format!("Sign in in provider settings using \"{}\". The login terminal runs on this environment.",method.name.0),_=>format!("Sign in in provider settings using \"{}\".",method.name.0)});
                    }
                    if self.spec.config["source"] == "local" {
                        value["setup"]["canAuthenticate"] = json!(!methods.is_empty());
                    }
                }
            }
        }
        base = serde_json::from_value(value).map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Response,
        ))?;
        Ok(base)
    }
    fn start_probe(&self, state: &mut State, base: ServerProvider) {
        state.probing = true;
        let generation = state.generation;
        let weak = self.self_ref.clone();
        let native = self.native.clone();
        let cwd = self.cwd.clone();
        let key = if self.spec.config["source"] == "local" {
            format!("local:{}", self.spec.instance_id.0)
        } else {
            self.spec.config["agentId"]
                .as_str()
                .unwrap_or_default()
                .into()
        };
        state.probe = Some(Task(tokio::spawn(async move {
            let result = native
                .coordinator
                .background_probe(&key, native.probe(&cwd))
                .await;
            let Some(provider) = weak.upgrade() else {
                return;
            };
            let mut state = provider.state.lock().await;
            if generation != state.generation {
                return;
            }
            state.probing = false;
            let result = match result {
                Ok(None) => return,
                Ok(Some(outcome)) => Ok(outcome),
                Err(failure) => Err(failure),
            };
            let Ok(mut next) = provider.enrich(base, result) else {
                return;
            };
            if next.status == ServerProviderState::Ready {
                state.cache = Some((
                    provider.clock.now_ms().saturating_add(900_000),
                    next.clone(),
                ));
            }
            provider.live(&mut next, &mut state.learned).await;
            state.current = Some(next.clone());
            provider.changes.send_replace(Some(next));
        })));
    }
}
#[async_trait]
impl ProviderInstanceService for AcpProvider {
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.spec.instance_id
    }
    fn subscribe_snapshots(&self) -> Option<watch::Receiver<Option<ServerProvider>>> {
        Some(self.changes.subscribe())
    }
    async fn snapshot(&self, _: Option<&str>, fresh: bool) -> ServiceResult<ServerProvider> {
        let mut state = self.state.lock().await;
        if fresh {
            state.generation = state.generation.wrapping_add(1);
            state.cache = None;
            state.probe = None;
            state.probing = false;
        }
        let base = self.readiness().await?;
        let cached = state
            .cache
            .as_ref()
            .filter(|(expires, snapshot)| {
                base.enabled
                    && base.installed
                    && self.clock.now_ms() < *expires
                    && snapshot.version == base.version
            })
            .map(|(_, snapshot)| snapshot.clone());
        let mut next = cached.clone().unwrap_or_else(|| base.clone());
        next.checked_at = base.checked_at.clone();
        self.live(&mut next, &mut state.learned).await;
        state.current = Some(next.clone());
        self.changes.send_replace(Some(next.clone()));
        if next.enabled && next.installed && cached.is_none() && !state.probing {
            self.start_probe(&mut state, base);
        }
        Ok(next)
    }
    async fn invalidate_models(&self, _: Option<&str>, _: bool) -> ServiceResult<()> {
        let mut state = self.state.lock().await;
        state.generation = state.generation.wrapping_add(1);
        state.cache = None;
        state.probe = None;
        state.probing = false;
        Ok(())
    }
    async fn invalidate_caches(&self) -> ServiceResult<()> {
        self.native
            .coordinator
            .clear_configuration(&self.spec.instance_id);
        self.state.lock().await.learned.clear();
        self.invalidate_models(None, true).await
    }
    async fn maintenance(
        &self,
        _: bool,
    ) -> ServiceResult<crate::maintenance::MaintenanceCapabilities> {
        Ok(crate::maintenance::MaintenanceCapabilities {
            driver: "acpRegistry".into(),
            package: None,
            update: None,
            install_path: None,
            manual_command: None,
        })
    }
}
