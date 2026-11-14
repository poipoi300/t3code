use crate::{
    acp_catalog,
    acp_connection::{AcpConnection, CoordinatedConsent, SetupHandler},
    acp_coordinator::{AcpRuntimeCoordinator, AvailableCommands},
    acp_service::{AcpError, AcpResult, AcpSessionManager},
    environment::Environment,
    registry_service::RegistryAgentCatalog,
    sensitive::Sensitive,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};
use t3_contracts::{acp_registry::*, provider_instance::ProviderInstanceId, speech::True};
type Reason = AcpRegistryOperationErrorReason;
#[derive(Debug)]
pub struct AcpProbeOutcome {
    pub probe: Sensitive<AcpRegistryProbeResult>,
    pub commands: AvailableCommands,
}
#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub struct AcpProbeFailure {
    pub error: AcpError,
    pub auth_methods: Sensitive<Vec<AcpRegistryProbeAuthMethod>>,
}
impl AcpProbeFailure {
    pub fn wire(self) -> Sensitive<AcpRegistryOperationError> {
        let mut error = self.error.wire();
        if !self.auth_methods.expose().is_empty() {
            error.auth_methods = Some(self.auth_methods.into_inner());
        }
        Sensitive::new(error)
    }
}
struct Close {
    connection: Option<AcpConnection>,
    session: Option<String>,
}
impl Close {
    async fn finish(mut self) {
        if let Some(connection) = self.connection.take() {
            if let Some(session) = self.session.as_ref() {
                connection.delete_probe_session(session).await;
            }
            connection.close().await;
        }
    }
}
impl Drop for Close {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            let session = self.session.take();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    if let Some(session) = session {
                        connection.delete_probe_session(&session).await;
                    }
                    connection.close().await;
                });
            }
        }
    }
}
pub struct AcpNativeServices {
    pub instance_id: ProviderInstanceId,
    pub settings: Value,
    pub environment: Sensitive<Environment>,
    pub catalog: Arc<RegistryAgentCatalog>,
    pub coordinator: Arc<AcpRuntimeCoordinator>,
}
impl AcpNativeServices {
    fn handler(&self) -> Arc<SetupHandler> {
        Arc::new(SetupHandler {
            consent: Some(Arc::new(CoordinatedConsent {
                coordinator: self.coordinator.clone(),
                instance_id: self.instance_id.clone(),
            })),
        })
    }
    pub async fn probe(&self, cwd: &Path) -> Result<AcpProbeOutcome, AcpProbeFailure> {
        let mut methods = Sensitive::new(vec![]);
        let work = async {
            let resolved = self
                .catalog
                .resolve(&self.settings, cwd, self.environment.expose())
                .await?;
            let connection = AcpConnection::spawn(&resolved, self.handler()).await?;
            let mut close = Close {
                connection: Some(connection),
                session: None,
            };
            let result = async {
                let connection = close.connection.as_mut().ok_or_else(|| {
                    AcpError::new(Reason::ProbeFailed, "The ACP probe connection closed.")
                })?;
                let initialized = connection
                    .initialize("t3-code-provider-test", false)
                    .await?;
                methods = acp_catalog::auth_methods(
                    initialized.expose()["authMethods"]
                        .as_array()
                        .map_or(&[], Vec::as_slice),
                    Some((&resolved.executable.to_string_lossy(), &resolved.args)),
                );
                let setup = connection.new_session(cwd).await?;
                close.session = setup.expose()["sessionId"].as_str().map(str::to_owned);
                let commands = connection
                    .advertised_commands()
                    .await
                    .map(|commands| (*commands).clone())
                    .unwrap_or(AvailableCommands {
                        slash_commands: vec![],
                        skills: vec![],
                    });
                let configuration = acp_catalog::live_configuration(setup.expose());
                let capabilities = &initialized.expose()["agentCapabilities"];
                let present = |path: &str| {
                    capabilities
                        .pointer(path)
                        .is_some_and(|value| !value.is_null())
                };
                let probe = AcpRegistryProbeResult {
                    instance_id: self.instance_id.clone(),
                    ready: True,
                    icon: resolved.agent.as_ref().and_then(|agent| agent.icon.clone()),
                    auth_methods: methods.expose().clone(),
                    models: configuration.models,
                    current_model_id: configuration.current_model_id.map(Into::into),
                    config_options: configuration.config_options,
                    model_option_ids: configuration
                        .model_option_ids
                        .map(|ids| ids.into_iter().map(Into::into).collect()),
                    session_management: AcpRegistrySessionManagement {
                        can_list: present("/sessionCapabilities/list"),
                        can_load: capabilities["loadSession"] == true,
                        can_resume: present("/sessionCapabilities/resume"),
                        can_logout: present("/auth/logout"),
                        can_delete: present("/sessionCapabilities/delete"),
                        can_configure_providers: present("/providers"),
                    },
                };
                Ok(AcpProbeOutcome {
                    probe: Sensitive::new(probe),
                    commands,
                })
            }
            .await;
            close.finish().await;
            result
        };
        let timeout = Duration::from_millis(crate::versions::probe_timeout_ms(
            "acp_registry",
            60_000,
            self.environment.expose(),
        ));
        match tokio::time::timeout(timeout, work).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(error)) => Err(AcpProbeFailure {
                error,
                auth_methods: methods,
            }),
            Err(_) => Err(AcpProbeFailure {
                error: AcpError::new(
                    Reason::ProbeFailed,
                    "The ACP agent did not resolve and create a test session before the probe deadline.",
                ),
                auth_methods: methods,
            }),
        }
    }
    async fn execute(
        &self,
        cwd: &Path,
        operation: Operation,
    ) -> AcpResult<(Sensitive<Value>, Sensitive<Value>)> {
        let failure = operation.failure();
        let work = async {
            let resolved = self
                .catalog
                .resolve(&self.settings, cwd, self.environment.expose())
                .await?;
            let connection = AcpConnection::spawn(&resolved, self.handler()).await?;
            let close = Close {
                connection: Some(connection),
                session: None,
            };
            let result=async{
                let connection=close.connection.as_ref().ok_or_else(||AcpError::new(failure.clone(),"The ACP management connection closed."))?;let initialized=connection.initialize("t3-code-session-manager",false).await?;let capabilities=&initialized.expose()["agentCapabilities"];
                let advertised=capabilities.pointer(operation.capability()).is_some_and(|value|!value.is_null());
                if !advertised{return Err(AcpError::new(operation.unsupported(),"The ACP agent does not advertise this management operation."));}
                let(method,params)=match operation{
                    Operation::List(cursor)=>{let mut params=json!({"cwd":cwd});if let Some(cursor)=cursor{params["cursor"]=json!(cursor);}("session/list",params)},
                    Operation::Delete(session)=>("session/delete",json!({"sessionId":session})),
                    Operation::Providers=>("providers/list",json!({"cwd":cwd})),
                    Operation::Set(input)=>{let input=input.into_inner();let mut params=json!({"cwd":cwd,"providerId":input.provider_id,"apiType":input.api_type,"baseUrl":input.base_url});if let Some(headers)=input.headers{params["headers"]=json!(headers);}("providers/set",params)},
                    Operation::Disable(provider)=>("providers/disable",json!({"cwd":cwd,"providerId":provider})),
                    Operation::Logout=>(if connection.generation()==t3_acp::Generation::V1{"logout"}else{"auth/logout"},json!({})),
                };
                let response=connection.request(method,Sensitive::new(params),failure.clone()).await?;Ok((initialized,response))
            }.await;
            close.finish().await;
            result
        };
        tokio::time::timeout(Duration::from_secs(60), work)
            .await
            .map_err(|_| AcpError::new(failure, "The ACP management request timed out."))?
    }
}
enum Operation {
    List(Option<String>),
    Delete(String),
    Providers,
    Set(Sensitive<AcpRegistrySetProviderInput>),
    Disable(String),
    Logout,
}
impl Operation {
    fn failure(&self) -> Reason {
        match self {
            Self::List(_) => Reason::ProbeFailed,
            Self::Delete(_) => Reason::SessionDeleteFailed,
            Self::Providers => Reason::ProvidersListFailed,
            Self::Set(_) | Self::Disable(_) => Reason::ProviderConfigurationFailed,
            Self::Logout => Reason::LogoutFailed,
        }
    }
    fn capability(&self) -> &'static str {
        match self {
            Self::List(_) => "/sessionCapabilities/list",
            Self::Delete(_) => "/sessionCapabilities/delete",
            Self::Providers | Self::Set(_) | Self::Disable(_) => "/providers",
            Self::Logout => "/auth/logout",
        }
    }
    fn unsupported(&self) -> Reason {
        match self {
            Self::List(_) => Reason::SessionListUnsupported,
            Self::Delete(_) => Reason::SessionDeleteUnsupported,
            Self::Providers | Self::Set(_) | Self::Disable(_) => Reason::ProvidersUnsupported,
            Self::Logout => Reason::LogoutUnsupported,
        }
    }
}
fn bounded(value: &Value, limit: usize) -> Option<String> {
    let text = value.as_str()?;
    (!text.is_empty()
        && t3_contracts::wire::trim_js(text) == text
        && text.encode_utf16().count() <= limit)
        .then(|| text.into())
}
fn label(value: &Value, limit: usize) -> Option<String> {
    let text = t3_contracts::wire::trim_js(value.as_str()?);
    (!text.is_empty())
        .then(|| String::from_utf16_lossy(&text.encode_utf16().take(limit).collect::<Vec<_>>()))
}
#[async_trait]
impl AcpSessionManager for AcpNativeServices {
    async fn list_sessions(
        &self,
        cwd: &Path,
        cursor: Option<&str>,
    ) -> AcpResult<AcpRegistryListSessionsResult> {
        let (initialized, response) = self
            .execute(cwd, Operation::List(cursor.map(str::to_owned)))
            .await?;
        let response = response.expose();
        let capabilities = &initialized.expose()["agentCapabilities"];
        let sessions = response["sessions"]
            .as_array()
            .ok_or_else(|| AcpError::new(Reason::ProbeFailed, "The ACP session list was invalid."))?
            .iter()
            .take(256)
            .filter_map(|session| {
                Some(AcpRegistrySession {
                    session_id: bounded(&session["sessionId"], 1024)?.into(),
                    cwd: bounded(&session["cwd"], 4096)?.into(),
                    additional_directories: session["additionalDirectories"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|directory| bounded(directory, 4096).map(Into::into))
                        .take(32)
                        .collect(),
                    title: label(&session["title"], 1024),
                    updated_at: label(&session["updatedAt"], 128),
                    imported_thread_id: None,
                    native_history_pending: None,
                })
            })
            .collect();
        Ok(AcpRegistryListSessionsResult {
            sessions,
            next_cursor: label(&response["nextCursor"], 2048),
            can_load: capabilities["loadSession"] == true,
            can_resume: capabilities
                .pointer("/sessionCapabilities/resume")
                .is_some_and(|value| !value.is_null()),
            can_delete: capabilities
                .pointer("/sessionCapabilities/delete")
                .is_some_and(|value| !value.is_null()),
        })
    }
    async fn delete_session(&self, cwd: &Path, session: &str) -> AcpResult<()> {
        self.execute(cwd, Operation::Delete(session.into()))
            .await
            .map(|_| ())
    }
    async fn list_providers(&self, cwd: &Path) -> AcpResult<AcpRegistryListProvidersResult> {
        let (_, response) = self.execute(cwd, Operation::Providers).await?;
        let providers = response.expose()["providers"]
            .as_array()
            .ok_or_else(|| {
                AcpError::new(
                    Reason::ProvidersListFailed,
                    "The ACP provider list was invalid.",
                )
            })?
            .iter()
            .take(64)
            .filter_map(|provider| {
                let id = bounded(&provider["providerId"], 256)?;
                let current = &provider["current"];
                let current = bounded(&current["apiType"], 64)
                    .zip(current["baseUrl"].as_str().and_then(acp_catalog::web_url))
                    .map(|(api_type, base_url)| AcpRegistryProviderConnection {
                        api_type: api_type.into(),
                        base_url,
                    });
                Some(AcpRegistryConfigurableProvider {
                    provider_id: id.into(),
                    supported: provider["supported"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|protocol| bounded(protocol, 64).map(Into::into))
                        .take(16)
                        .collect(),
                    required: provider["required"] == true,
                    current,
                })
            })
            .collect();
        Ok(AcpRegistryListProvidersResult { providers })
    }
    async fn set_provider(
        &self,
        cwd: &Path,
        input: Sensitive<AcpRegistrySetProviderInput>,
    ) -> AcpResult<()> {
        self.execute(cwd, Operation::Set(input)).await.map(|_| ())
    }
    async fn disable_provider(&self, cwd: &Path, provider: &str) -> AcpResult<()> {
        self.execute(cwd, Operation::Disable(provider.into()))
            .await
            .map(|_| ())
    }
    async fn logout(&self, cwd: &Path) -> AcpResult<()> {
        self.execute(cwd, Operation::Logout).await.map(|_| ())
    }
}
