use crate::{
    acp_auth_state::AcpAuthenticationState,
    acp_catalog,
    acp_connection::{AcpConnection, AcpUrlConsent, SetupHandler},
    acp_coordinator::AcpRuntimeCoordinator,
    auth::{AuthBackend, AuthContext, AuthReply, CredentialBinding},
    environment::Environment,
    registry_service::{InspectionStatus, RegistryAgentCatalog, ResolvedRegistryAgent},
    sensitive::{Sensitive, ServiceError, ServiceResult},
};
use async_trait::async_trait;
use serde_json::Value;
use std::{future::Future, path::PathBuf, sync::Arc, time::Duration};
use t3_contracts::{
    acp_registry::AcpRegistryUrlAuthAction,
    provider_instance::ProviderInstanceId,
    provider_setup::{
        ProviderAuthInteraction, ProviderAuthMethod, ProviderAuthResponse,
        ProviderBrowserAuthAction, ProviderCredentialOwner,
    },
};
use tokio::sync::{Mutex, mpsc, oneshot};
struct Consent {
    action: Sensitive<AcpRegistryUrlAuthAction>,
    answer: oneshot::Sender<bool>,
}
struct FlowConsent(mpsc::UnboundedSender<Consent>);
#[async_trait]
impl AcpUrlConsent for FlowConsent {
    async fn consent(&self, action: Sensitive<AcpRegistryUrlAuthAction>) -> bool {
        let (answer, receive) = oneshot::channel();
        if self.0.send(Consent { action, answer }).is_err() {
            return false;
        }
        receive.await.unwrap_or(false)
    }
}
async fn with_consent<T>(
    operation: impl Future<Output = ServiceResult<T>>,
    context: &mut AuthContext,
    consents: &mut mpsc::UnboundedReceiver<Consent>,
) -> ServiceResult<T> {
    tokio::pin!(operation);
    let mut pending: Option<oneshot::Sender<bool>> = None;
    loop {
        tokio::select! {
            result=&mut operation=>return result,
            Some(consent)=consents.recv()=>{
                if let Some(previous)=pending.replace(consent.answer){let _=previous.send(false);}
                let action=consent.action.into_inner();context.interaction(ProviderAuthInteraction::Browser{id:action.elicitation_id,url:action.url.into(),requires_consent:true,accepts_callback:None},true,false).await?;
            },
            delivery=context.receive(),if pending.is_some()=>{
                let delivery=delivery?;let accepted=match &delivery.reply{AuthReply::Response(response)=>match response.expose(){ProviderAuthResponse::Browser{action}=>Some(*action==ProviderBrowserAuthAction::Accept),_=>None},_=>None};
                if let Some(accepted)=accepted{if let Some(answer)=pending.take(){let _=answer.send(accepted);}delivery.finish(Ok(()));}else{delivery.finish(Err(ServiceError::Interaction));}
            }
        }
    }
}
#[async_trait]
pub trait AcpTerminalLogin: Send + Sync {
    async fn login(
        &self,
        spawn: &ResolvedRegistryAgent,
        method: Sensitive<Value>,
        context: &mut AuthContext,
    ) -> ServiceResult<()>;
}
pub struct AcpAuth {
    pub instance_id: ProviderInstanceId,
    pub enabled: bool,
    pub settings: Value,
    pub cwd: PathBuf,
    pub environment: Sensitive<Environment>,
    pub catalog: Arc<RegistryAgentCatalog>,
    pub coordinator: Arc<AcpRuntimeCoordinator>,
    pub confirmed: Arc<AcpAuthenticationState>,
    pub terminal: Option<Arc<dyn AcpTerminalLogin>>,
    known: Mutex<Option<(String, Vec<ProviderAuthMethod>)>>,
}
impl AcpAuth {
    pub fn new(
        native: &crate::acp_probe::AcpNativeServices,
        enabled: bool,
        cwd: PathBuf,
        confirmed: Arc<AcpAuthenticationState>,
    ) -> Self {
        Self {
            instance_id: native.instance_id.clone(),
            enabled,
            settings: native.settings.clone(),
            cwd,
            environment: native.environment.clone(),
            catalog: native.catalog.clone(),
            coordinator: native.coordinator.clone(),
            confirmed,
            terminal: None,
            known: Mutex::new(None),
        }
    }
    pub fn agent_key(&self) -> String {
        if self.settings["source"] == "local" {
            format!("local:{}", self.instance_id.0)
        } else {
            self.settings["agentId"].as_str().unwrap_or_default().into()
        }
    }
    pub fn binding(&self) -> CredentialBinding {
        CredentialBinding {
            owner: ProviderCredentialOwner::Provider,
            key: format!("acp:{}", self.agent_key()),
        }
    }
    async fn discover(&self) -> ServiceResult<Vec<ProviderAuthMethod>> {
        let inspected = self
            .catalog
            .inspect(&self.settings, self.environment.expose())
            .await
            .map_err(|_| ServiceError::Response)?;
        if inspected.status != InspectionStatus::Ready {
            return Err(ServiceError::Method);
        }
        let version = inspected.version.flatten();
        let mut known = self.known.lock().await;
        if let Some(version) = version.as_ref()
            && let Some((cached, methods)) = known.as_ref()
            && version == cached
        {
            return Ok(methods.clone());
        }
        let resolved = self
            .catalog
            .resolve(&self.settings, &self.cwd, self.environment.expose())
            .await
            .map_err(|_| ServiceError::Response)?;
        let connection = AcpConnection::spawn(&resolved, Arc::new(SetupHandler { consent: None }))
            .await
            .map_err(|_| ServiceError::Response)?;
        let initialized = connection
            .initialize("t3-code-provider-auth", self.terminal.is_some())
            .await;
        connection.close().await;
        let initialized = initialized.map_err(|_| ServiceError::Response)?;
        let methods = acp_catalog::auth_methods(
            initialized.expose()["authMethods"]
                .as_array()
                .map_or(&[], Vec::as_slice),
            None,
        );
        let methods=methods.into_inner().into_iter().filter(|method|self.terminal.is_some()||method.r#type!=t3_contracts::acp_registry::AcpRegistryProbeAuthMethodType::Terminal).filter_map(|method|serde_json::from_value(serde_json::json!({"id":method.id,"name":method.name,"description":method.description,"type":match method.r#type{t3_contracts::acp_registry::AcpRegistryProbeAuthMethodType::EnvVar=>"credentials",t3_contracts::acp_registry::AcpRegistryProbeAuthMethodType::Terminal=>"terminal",_=>"agent"}})).ok()).collect::<Vec<_>>();
        if let Some(version) = version {
            *known = Some((version, methods.clone()));
        }
        Ok(methods)
    }
    async fn sign_in(&self, method: &str, mut context: AuthContext) -> ServiceResult<()> {
        let _foreground = self.coordinator.foreground_startup(&self.agent_key());
        let resolved = self
            .catalog
            .resolve(&self.settings, &self.cwd, self.environment.expose())
            .await
            .map_err(|_| ServiceError::Response)?;
        let (send, mut receive) = mpsc::unbounded_channel();
        let handler = Arc::new(SetupHandler {
            consent: Some(Arc::new(FlowConsent(send))),
        });
        let connection = AcpConnection::spawn(&resolved, handler.clone())
            .await
            .map_err(|_| ServiceError::Response)?;
        let result: ServiceResult<()> = async {
            let initialized = with_consent(
                async {
                    connection
                        .initialize("t3-code-provider-auth", self.terminal.is_some())
                        .await
                        .map_err(|_| ServiceError::Response)
                },
                &mut context,
                &mut receive,
            )
            .await?;
            let method = initialized.expose()["authMethods"]
                .as_array()
                .and_then(|methods| methods.iter().find(|candidate| candidate["id"] == method))
                .cloned()
                .ok_or(ServiceError::Method)?;
            if method["type"] == "terminal" {
                connection.close().await;
                let terminal = self.terminal.as_ref().ok_or(ServiceError::Unsupported)?;
                terminal
                    .login(&resolved, Sensitive::new(method), &mut context)
                    .await?;
                context.verifying().await;
                let verified = AcpConnection::spawn(&resolved, handler)
                    .await
                    .map_err(|_| ServiceError::Response)?;
                let result: ServiceResult<()> = async {
                    with_consent(
                        async {
                            verified
                                .initialize("t3-code-provider-auth", self.terminal.is_some())
                                .await
                                .map_err(|_| ServiceError::Response)
                        },
                        &mut context,
                        &mut receive,
                    )
                    .await?;
                    with_consent(
                        async {
                            verified
                                .new_session(&self.cwd)
                                .await
                                .map_err(|_| ServiceError::Response)
                        },
                        &mut context,
                        &mut receive,
                    )
                    .await?;
                    Ok(())
                }
                .await;
                verified.close().await;
                result?;
            } else {
                if method["type"].is_null() || method["type"] == "agent" {
                    let id = method["id"].as_str().ok_or(ServiceError::Method)?;
                    with_consent(
                        async {
                            connection
                                .authenticate(id)
                                .await
                                .map_err(|_| ServiceError::Response)
                        },
                        &mut context,
                        &mut receive,
                    )
                    .await?;
                }
                context.verifying().await;
                with_consent(
                    async {
                        connection
                            .new_session(&self.cwd)
                            .await
                            .map_err(|_| ServiceError::Response)
                    },
                    &mut context,
                    &mut receive,
                )
                .await?;
            }
            Ok(())
        }
        .await;
        connection.close().await;
        result?;
        self.confirmed.set(true).await
    }
}
#[async_trait]
impl AuthBackend for AcpAuth {
    fn default_method_id(&self) -> Option<&str> {
        self.settings["authMethodId"]
            .as_str()
            .filter(|method| !method.is_empty())
    }
    fn check_start(&self) -> ServiceResult<()> {
        if self.enabled {
            Ok(())
        } else {
            Err(ServiceError::Disabled)
        }
    }
    async fn methods(&self) -> ServiceResult<Vec<ProviderAuthMethod>> {
        let work = tokio::time::timeout(Duration::from_secs(30), self.discover());
        self.coordinator
            .background_probe(&self.agent_key(), async {
                work.await.map_err(|_| ServiceError::Response)?
            })
            .await?
            .ok_or(ServiceError::Busy)
    }
    async fn authenticate(&self, method: &str, context: AuthContext) -> ServiceResult<()> {
        self.check_start()?;
        self.sign_in(method, context).await
    }
    async fn logout(&self) -> ServiceResult<Option<String>> {
        let _foreground = self.coordinator.foreground_startup(&self.agent_key());
        let work = async {
            let resolved = self
                .catalog
                .resolve(&self.settings, &self.cwd, self.environment.expose())
                .await
                .map_err(|_| ServiceError::Response)?;
            let connection =
                AcpConnection::spawn(&resolved, Arc::new(SetupHandler { consent: None }))
                    .await
                    .map_err(|_| ServiceError::Response)?;
            let result: ServiceResult<()> = async {
                connection
                    .initialize("t3-code-provider-auth", false)
                    .await
                    .map_err(|_| ServiceError::Response)?;
                let method = if connection.generation() == t3_acp::Generation::V1 {
                    "logout"
                } else {
                    "auth/logout"
                };
                connection
                    .request(
                        method,
                        Sensitive::new(serde_json::json!({})),
                        t3_contracts::acp_registry::AcpRegistryOperationErrorReason::LogoutFailed,
                    )
                    .await
                    .map_err(|_| ServiceError::Response)?;
                Ok(())
            }
            .await;
            connection.close().await;
            result
        };
        tokio::time::timeout(Duration::from_secs(60), work)
            .await
            .map_err(|_| ServiceError::Response)??;
        self.confirmed.set(false).await?;
        Ok(None)
    }
}
