use crate::{
    auth::{AuthController, CredentialAccess},
    catalog::ManifestSource,
    chatgpt::{ChatGptAuth, REQUIRED_SCOPE},
    codex_installation::{CodexInstallation, ExecutableLease},
    environment::{CodexHomeLayout, Environment},
    homes::materialize_codex_shadow,
    instances::InstanceSpec,
    native_probe::NativeProbe,
    native_provider::{ExecutableProbe, NativeProvider, NativeProviderContext},
    sensitive::{Sensitive, ServiceError, ServiceResult},
    service::ProviderInstanceService,
    token_broker::BrokerListener,
    transport::{HttpRequest, HttpTransport, ProviderClock},
};
use async_trait::async_trait;
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use t3_contracts::{
    provider_instance::ProviderInstanceId,
    server::{ServerProvider, ServerProviderModel},
};

pub struct ManagedCodexRuntime {
    pub auth: Arc<ChatGptAuth>,
    pub controller: Arc<AuthController>,
    pub installation: Arc<CodexInstallation>,
    pub layout: CodexHomeLayout,
    pub environment: Environment,
    pub environment_id: String,
    pub instance: ProviderInstanceId,
    pub helper_command: String,
    pub helper_args: Vec<String>,
    pub listener: Arc<BrokerListener>,
}
pub struct EffectiveCodexRuntime {
    pub executable: ExecutableLease,
    pub environment: Environment,
    pub launch_args: String,
    pub revision: String,
    _access: CredentialAccess,
}
impl EffectiveCodexRuntime {
    pub fn cancellation(&self) -> t3_process::CancellationToken {
        self._access.cancellation.clone()
    }
}
impl ManagedCodexRuntime {
    pub async fn resolve(&self) -> ServiceResult<EffectiveCodexRuntime> {
        let access = self.controller.admit_access().await?;
        if self.listener.endpoint.is_empty() {
            return Err(ServiceError::Unavailable);
        }
        let executable = self.installation.acquire().await?;
        let credentials = tokio::select! {
            biased;
            _ = access.cancellation.cancelled() => return Err(ServiceError::Cancelled),
            credentials = self.auth.access(None) => credentials?,
        };
        let revision = ChatGptAuth::profile_key(&credentials)?;
        materialize_codex_shadow(&self.layout)?;
        let home = self
            .layout
            .effective_home
            .as_ref()
            .unwrap_or(&self.layout.shared_home);
        std::fs::create_dir_all(home).map_err(|_| ServiceError::Storage)?;
        let mut environment = self.environment.clone();
        environment.retain(|key, _| {
            ![
                "ACCESS_TOKEN",
                "T3CODE_CODEX_LAUNCH_ARGS",
                "OPENAI_API_KEY",
                "OPENAI_BASE_URL",
                "CODEX_HOME",
            ]
            .iter()
            .any(|name| key.eq_ignore_ascii_case(name))
        });
        environment.insert("CODEX_HOME".into(), home.to_string_lossy().into_owned());
        let mut args = self.helper_args.clone();
        args.extend([
            "__managed-chatgpt-token".to_owned(),
            self.listener.endpoint.clone(),
            self.instance.0.clone(),
            self.environment_id.clone(),
            revision.clone(),
        ]);
        let launch_args = managed_launch_args(&self.helper_command, &args)?;
        if access.cancellation.is_cancelled() {
            return Err(ServiceError::Cancelled);
        }
        Ok(EffectiveCodexRuntime {
            executable,
            environment,
            launch_args,
            revision,
            _access: access,
        })
    }
}
pub fn managed_launch_args(command: &str, args: &[String]) -> ServiceResult<String> {
    let encode = |value: &str| serde_json::to_string(value).map_err(|_| ServiceError::Response);
    let command = encode(command)?;
    let args = serde_json::to_string(args).map_err(|_| ServiceError::Response)?;
    let config = [
        "model_provider=\"openai_token_sharing\"".to_owned(),
        format!(
            "model_providers.openai_token_sharing={{name=\"OpenAI Token Sharing\",base_url=\"https://api.openai.com/v1\",model_catalog_url=\"https://api.openai.com/v1/models\",wire_api=\"responses\",requires_openai_auth=false,supports_websockets=false,auth={{command={command},args={args},timeout_ms=80000,refresh_interval_ms=0}}}}"
        ),
        "features.api_key_model_discovery=true".into(),
        "features.multi_agent=false".into(),
        "features.multi_agent_v2=false".into(),
    ];
    config
        .iter()
        .map(|value| encode(value).map(|v| format!("-c {v}")))
        .collect::<ServiceResult<Vec<_>>>()
        .map(|v| v.join(" "))
}
pub async fn chatgpt_models(
    http: &dyn HttpTransport,
    token: Sensitive<String>,
    native: &[ServerProviderModel],
) -> ServiceResult<Vec<ServerProviderModel>> {
    let response = tokio::time::timeout(
        Duration::from_secs(15),
        http.send(HttpRequest {
            method: "GET",
            url: "https://api.openai.com/v1/models".into(),
            headers: BTreeMap::from([(
                "authorization".into(),
                Sensitive::new(format!("Bearer {}", token.expose())),
            )]),
            body: None,
        }),
    )
    .await
    .map_err(|_| ServiceError::Network)??;
    if response.status != 200 {
        return Err(ServiceError::Response);
    }
    let models = response.body.expose()["models"]
        .as_array()
        .ok_or(ServiceError::Response)?;
    let mut output = vec![];
    for entry in models {
        let slug = entry["slug"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(ServiceError::Response)?;
        let name = entry["display_name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(ServiceError::Response)?;
        let visibility = entry["visibility"].as_str().ok_or(ServiceError::Response)?;
        if visibility != "list" {
            continue;
        }
        let mut model = if let Some(native) = native.iter().find(|m| m.slug.0 == slug) {
            native.clone()
        } else {
            crate::catalog::model(slug, name, None)
        };
        model.slug = slug.to_owned().into();
        model.name = name.to_owned().into();
        model.is_custom = false;
        if model.capabilities.is_none()
            && entry["default_reasoning_level"].is_string()
            && entry["supported_reasoning_levels"].is_array()
        {
            let levels:Vec<_>=entry["supported_reasoning_levels"].as_array().ok_or(ServiceError::Response)?.iter().map(|v|json!({"reasoningEffort":v["effort"],"description":v["description"].as_str().unwrap_or_default()})).collect();
            model.capabilities = Some(crate::probe_models::codex_capabilities(
                &json!({"model":slug,"defaultReasoningEffort":entry["default_reasoning_level"],"supportedReasoningEfforts":levels}),
            )?);
        }
        if let Some(capabilities) = model.capabilities.as_mut()
            && let Some(descriptors) = capabilities
                .option_descriptors
                .as_mut()
                .and_then(Option::as_mut)
        {
            descriptors.retain(|d| crate::catalog::option_id(d) != "serviceTier");
        }
        output.push(model);
    }
    Ok(output)
}
pub struct ManagedCodexProvider {
    pub runtime: Arc<ManagedCodexRuntime>,
    pub spec: Arc<InstanceSpec>,
    pub context: NativeProviderContext,
    pub protocols: Arc<dyn NativeProbe>,
    pub executables: Arc<dyn ExecutableProbe>,
    pub clock: Arc<dyn ProviderClock>,
    pub manifest: ManifestSource,
    pub http: Arc<dyn HttpTransport>,
}
impl ManagedCodexProvider {
    pub fn initial_snapshot(&self) -> ServiceResult<ServerProvider> {
        let checked = chrono::DateTime::from_timestamp_millis(self.clock.now_ms())
            .ok_or(ServiceError::Response)?
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let mut value = json!({"instanceId":self.spec.instance_id,"driver":"codex","displayName":self.spec.display_name.as_deref().unwrap_or("Codex"),"enabled":self.spec.enabled,"installed":false,"version":null,"status":if self.spec.enabled{"warning"}else{"disabled"},"auth":{"status":"unknown"},"models":[],"checkedAt":checked,"setup":{"canAuthenticate":true,"canInstall":true},"continuation":{"groupKey":self.runtime.layout.continuation_key},"runtimePaths":{"homePath":self.runtime.layout.shared_home.to_string_lossy(),"shadowHomePath":if self.runtime.layout.auth_overlay{self.runtime.layout.effective_home.as_ref().map(|p|p.to_string_lossy().into_owned())}else{None}},"reportsContextWindow":true,"showInteractionModeToggle":true,"supportedRuntimeModes":["approval-required","auto-accept-edits","auto","full-access"]});
        if let Some(color) = self.spec.accent_color.as_ref() {
            value["accentColor"] = json!(color);
        }
        value["message"] = json!(if self.spec.enabled {
            "Codex provider status has not been checked in this session yet."
        } else {
            "Codex is disabled in T3 Code settings."
        });
        serde_json::from_value(value).map_err(|_| ServiceError::Response)
    }
    pub async fn into_managed(
        self,
        options: crate::managed_provider::ManagedOptions,
    ) -> ServiceResult<Arc<crate::managed_provider::ManagedProvider>> {
        let initial = self.initial_snapshot()?;
        let manifest = self.manifest.subscribe();
        let background = options.background.clone();
        let changes = self
            .runtime
            .controller
            .subscribe("managed-codex-snapshot".into());
        let provider =
            crate::managed_provider::ManagedProvider::new(Arc::new(self), initial, options)?;
        provider.recheck_on_auth(changes, background.clone()).await;
        provider.recheck_on_manifest(manifest, background);
        Ok(provider)
    }
}
#[async_trait]
impl ProviderInstanceService for ManagedCodexProvider {
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.spec.instance_id
    }
    async fn snapshot(&self, cwd: Option<&str>, _fresh: bool) -> ServiceResult<ServerProvider> {
        let mut value =
            serde_json::to_value(self.initial_snapshot()?).map_err(|_| ServiceError::Response)?;
        let checked = value["checkedAt"]
            .as_str()
            .ok_or(ServiceError::Response)?
            .to_owned();
        if self.spec.enabled {
            value["auth"] = json!({"status":"unauthenticated"});
            match self.runtime.installation.resolve().await {
                Err(_) => {
                    value["message"] = json!("Set up Codex to get started.");
                }
                Ok(executable) => {
                    value["installed"] = json!(true);
                    value["version"] = json!(executable.version);
                    let saved = self.runtime.auth.read().await.ok().flatten();
                    if let Some(saved) = saved {
                        value["auth"] = json!({"status":"unauthenticated","label":"ChatGPT"});
                        if let Some(email) = saved.email.as_ref().filter(|s| !s.trim().is_empty()) {
                            value["auth"]["email"] = json!(email.trim());
                        }
                        if !saved.scopes.iter().any(|s| s == REQUIRED_SCOPE) {
                            value["message"] = json!(
                                "Signed in with ChatGPT, but token sharing is disabled. Sign in again and enable token sharing, or use another provider."
                            );
                        } else {
                            value["auth"]["status"] = json!("authenticated");
                            value["auth"]["type"] = json!("chatgpt");
                            value["auth"]["subscriptionSharing"] = json!(true);
                            value["auth"]["profileId"] = json!(saved.client_id);
                            value["usageLimits"] = json!({"checkedAt":checked,"windows":[],"unavailable":{"reason":"unsupported","message":"ChatGPT tracks subscription usage across connected apps. Open Usage settings with the account you connected to Codex."},"externalUsage":{"label":"ChatGPT usage","url":"https://chatgpt.com/#settings/Usage"}});
                            let work=async {
                                let effective=self.runtime.resolve().await?;
                                let config=json!({"setupMode":"existing","binaryPath":effective.executable.executable.path,"homePath":self.runtime.layout.shared_home,"shadowHomePath":if self.runtime.layout.auth_overlay{self.runtime.layout.effective_home.clone()}else{None},"launchArgs":effective.launch_args,"customModels":[]});
                                let spec=Arc::new(InstanceSpec {instance_id:self.spec.instance_id.clone(),driver:self.spec.driver.clone(),display_name:self.spec.display_name.clone(),accent_color:self.spec.accent_color.clone(),enabled:self.spec.enabled,config,variables:Default::default()});
                                let context=NativeProviderContext {environment:effective.environment.clone(),home:self.context.home.clone(),cwd:self.context.cwd.clone(),platform:self.context.platform,managed_mcp:self.context.managed_mcp,server_version:self.context.server_version.clone()};
                                let provider=NativeProvider::new(spec,context,self.protocols.clone(),self.executables.clone(),self.clock.clone(),self.manifest.clone());
                                let cancellation=effective.cancellation();
                                let draft=tokio::select! {biased;
                                    _=cancellation.cancelled()=>return Err(ServiceError::Cancelled),
                                    draft=provider.snapshot(cwd,true)=>draft?,
                                };
                                let credentials=tokio::select! {biased;
                                    _=cancellation.cancelled()=>return Err(ServiceError::Cancelled),
                                    credentials=self.runtime.auth.access(Some(effective.revision.clone()))=>credentials?,
                                };
                                if ChatGptAuth::profile_key(&saved)?!=effective.revision{return Err(ServiceError::ProfileChanged);}
                                let models=tokio::select! {biased;
                                    _=cancellation.cancelled()=>return Err(ServiceError::Cancelled),
                                    models=chatgpt_models(&*self.http,credentials.access_token,&draft.models)=>models?,
                                };
                                let mut draft=serde_json::to_value(draft).map_err(|_|ServiceError::Response)?;
                                for key in ["auth","usageLimits","setup","runtimePaths","continuation"]{draft[key]=value[key].clone();}
                                if let Some(commands)=draft["slashCommands"].as_array_mut(){commands.retain(|command|command["name"]!="feedback");}
                                draft["models"]=json!(models);Ok::<_,ServiceError>(draft)
                            }.await;
                            match work {
                                Ok(draft) => value = draft,
                                Err(_) => {
                                    let current = self.runtime.auth.read().await.ok().flatten();
                                    let sharing = current.as_ref().is_some_and(|record| {
                                        record.scopes.iter().any(|scope| scope == REQUIRED_SCOPE)
                                    });
                                    value["auth"] = json!({"status":if sharing {"authenticated"}else{"unauthenticated"},"label":"ChatGPT"});
                                    if let Some(current) = current {
                                        if sharing {
                                            value["auth"]["subscriptionSharing"] = json!(true);
                                            value["auth"]["profileId"] = json!(current.client_id);
                                        }
                                        if let Some(email) = current
                                            .email
                                            .as_deref()
                                            .map(str::trim)
                                            .filter(|email| !email.is_empty())
                                        {
                                            value["auth"]["email"] = json!(email);
                                        }
                                    }
                                    if !sharing {
                                        value
                                            .as_object_mut()
                                            .ok_or(ServiceError::Response)?
                                            .remove("usageLimits");
                                    }
                                    value["message"] = json!(
                                        "Could not check Codex right now. Retry, or reconnect in provider settings."
                                    );
                                }
                            }
                        }
                    } else {
                        value["message"] = json!("Sign in with ChatGPT to use Codex.");
                    }
                }
            }
        }
        serde_json::from_value::<ServerProvider>(value).map_err(|_| ServiceError::Response)
    }
}
