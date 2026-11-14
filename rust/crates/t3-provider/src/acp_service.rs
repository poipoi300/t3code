use crate::{acp_coordinator::AcpRuntimeCoordinator, auth::AuthService, sensitive::Sensitive};
use async_trait::async_trait;
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_contracts::{
    acp_registry::*,
    base_schemas::{ProjectId, ThreadId},
    orchestration_v2::{
        CommandThreadCreateImportedNativeThread, OrchestrationV2Actor,
        OrchestrationV2ThreadLaunchInput,
    },
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
    server::ServerProvider,
    speech::True,
};

#[derive(Clone, thiserror::Error)]
#[error("{message}")]
pub struct AcpError {
    pub reason: AcpRegistryOperationErrorReason,
    message: &'static str,
}
impl std::fmt::Debug for AcpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}
impl AcpError {
    pub fn new(reason: AcpRegistryOperationErrorReason, message: &'static str) -> Self {
        Self { reason, message }
    }
    pub fn wire(self) -> AcpRegistryOperationError {
        AcpRegistryOperationError {
            _tag: AcpRegistryOperationErrorTag::Error,
            reason: self.reason,
            message: self.message.into(),
            auth_methods: None,
            auth_action: None,
            cause: None,
        }
    }
}
pub type AcpResult<T> = Result<T, AcpError>;
#[async_trait]
pub trait AcpCatalogOperations: Send + Sync {
    async fn search(&self, input: AcpRegistrySearchInput) -> AcpResult<AcpRegistrySearchResult>;
    async fn prepare(&self, input: AcpRegistryPrepareInput) -> AcpResult<AcpRegistryPrepareResult>;
    async fn uninstall(
        &self,
        input: AcpRegistryManagedBinaryUninstallInput,
    ) -> AcpResult<AcpRegistryManagedBinaryUninstallResult>;
}
#[async_trait]
pub trait AcpSessionManager: Send + Sync {
    async fn list_sessions(
        &self,
        cwd: &Path,
        cursor: Option<&str>,
    ) -> AcpResult<AcpRegistryListSessionsResult>;
    async fn delete_session(&self, cwd: &Path, session: &str) -> AcpResult<()>;
    async fn list_providers(&self, cwd: &Path) -> AcpResult<AcpRegistryListProvidersResult>;
    async fn set_provider(
        &self,
        cwd: &Path,
        input: Sensitive<AcpRegistrySetProviderInput>,
    ) -> AcpResult<()>;
    async fn disable_provider(&self, cwd: &Path, provider: &str) -> AcpResult<()>;
    async fn logout(&self, cwd: &Path) -> AcpResult<()>;
}
pub struct AcpInstance {
    pub driver: ProviderDriverKind,
    pub snapshot: ServerProvider,
    pub manager: Arc<dyn AcpSessionManager>,
    pub managed_auth: bool,
}
#[derive(Clone, Debug)]
pub struct ImportedThread {
    pub native_history_pending: bool,
}
pub struct ImportLaunch {
    pub launch: OrchestrationV2ThreadLaunchInput,
    pub native: CommandThreadCreateImportedNativeThread,
    pub created_by: OrchestrationV2Actor,
}
#[async_trait]
pub trait AcpSessionHost: Send + Sync {
    async fn project_workspace(&self, project: &ProjectId) -> AcpResult<PathBuf>;
    async fn instance(&self, instance: &ProviderInstanceId) -> AcpResult<AcpInstance>;
    async fn thread(&self, thread: &ThreadId) -> AcpResult<Option<ImportedThread>>;
    async fn launch_import(&self, launch: ImportLaunch) -> AcpResult<()>;
    async fn import_history(&self, thread: &ThreadId, cwd: &Path) -> AcpResult<()>;
    async fn close_sessions(&self, instance: &ProviderInstanceId) -> AcpResult<()>;
    async fn refresh(&self, instance: &ProviderInstanceId) -> AcpResult<()>;
}
pub fn imported_thread_id(
    driver: &ProviderDriverKind,
    instance: &ProviderInstanceId,
    session: &str,
) -> ThreadId {
    fn encode(text: &str) -> String {
        let mut encoded = String::new();
        for byte in text.bytes() {
            if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
                encoded.push(byte as char);
            } else {
                use std::fmt::Write;
                let _ = write!(encoded, "%{byte:02X}");
            }
        }
        encoded
    }
    ThreadId(format!(
        "thread:provider:{}:provider-instance:{}:native-thread:{}",
        encode(&driver.0),
        encode(&instance.0),
        encode(session)
    ))
}
pub struct AcpRegistryService {
    catalog: Arc<dyn AcpCatalogOperations>,
    host: Arc<dyn AcpSessionHost>,
    coordinator: Arc<AcpRuntimeCoordinator>,
    auth: Arc<AuthService>,
    cwd: PathBuf,
}
impl AcpRegistryService {
    pub fn new(
        catalog: Arc<dyn AcpCatalogOperations>,
        host: Arc<dyn AcpSessionHost>,
        coordinator: Arc<AcpRuntimeCoordinator>,
        auth: Arc<AuthService>,
        cwd: PathBuf,
    ) -> Self {
        Self {
            catalog,
            host,
            coordinator,
            auth,
            cwd,
        }
    }
    pub async fn search(
        &self,
        input: AcpRegistrySearchInput,
    ) -> AcpResult<AcpRegistrySearchResult> {
        self.catalog.search(input).await
    }
    pub async fn prepare(
        &self,
        input: AcpRegistryPrepareInput,
    ) -> AcpResult<AcpRegistryPrepareResult> {
        self.catalog.prepare(input).await
    }
    pub async fn uninstall_managed_binary(
        &self,
        input: AcpRegistryManagedBinaryUninstallInput,
    ) -> AcpResult<AcpRegistryManagedBinaryUninstallResult> {
        self.catalog.uninstall(input).await
    }
    pub fn accept_url_authentication(
        &self,
        input: AcpRegistryAcceptUrlAuthInput,
    ) -> AcpRegistryAcceptUrlAuthResult {
        AcpRegistryAcceptUrlAuthResult {
            accepted: self.coordinator.accept_url_auth(&input),
        }
    }
    pub async fn list_sessions(
        &self,
        input: AcpRegistryListSessionsInput,
    ) -> AcpResult<AcpRegistryListSessionsResult> {
        let cwd = self.host.project_workspace(&input.project_id).await?;
        let instance = self.host.instance(&input.instance_id).await?;
        let mut listed = instance
            .manager
            .list_sessions(&cwd, input.cursor.as_deref())
            .await?;
        let mut sessions = std::mem::take(&mut listed.sessions).into_iter().enumerate();
        let mut jobs = tokio::task::JoinSet::new();
        let mut finished = vec![];
        let error = || {
            AcpError::new(
                AcpRegistryOperationErrorReason::SessionImportFailed,
                "Could not inspect existing imported ACP sessions.",
            )
        };
        loop {
            while jobs.len() < 16 {
                let Some((index, mut session)) = sessions.next() else {
                    break;
                };
                let id =
                    imported_thread_id(&instance.driver, &input.instance_id, &session.session_id.0);
                let host = self.host.clone();
                jobs.spawn(async move {
                    let thread = host.thread(&id).await.map_err(|_| {
                        AcpError::new(
                            AcpRegistryOperationErrorReason::SessionImportFailed,
                            "Could not inspect existing imported ACP sessions.",
                        )
                    })?;
                    session.native_history_pending = thread
                        .as_ref()
                        .filter(|thread| thread.native_history_pending)
                        .map(|_| true);
                    session.imported_thread_id = thread.map(|_| id);
                    Ok::<_, AcpError>((index, session))
                });
            }
            match jobs.join_next().await {
                Some(result) => finished.push(result.map_err(|_| error())??),
                None => break,
            }
        }
        finished.sort_by_key(|(index, _)| *index);
        listed.sessions = finished.into_iter().map(|(_, session)| session).collect();
        Ok(listed)
    }
    pub async fn delete_session(
        &self,
        input: AcpRegistryDeleteSessionInput,
    ) -> AcpResult<AcpRegistryDeleteSessionResult> {
        let _guard = self.coordinator.session_mutation.lock().await;
        let cwd = self.host.project_workspace(&input.project_id).await?;
        let instance = self.host.instance(&input.instance_id).await?;
        if !instance
            .snapshot
            .native_sessions
            .as_ref()
            .and_then(Option::as_ref)
            .is_some_and(|sessions| sessions.can_delete == Some(Some(true)))
        {
            return Err(AcpError::new(
                AcpRegistryOperationErrorReason::SessionDeleteUnsupported,
                "The ACP agent does not advertise session deletion.",
            ));
        }
        let thread = imported_thread_id(&instance.driver, &input.instance_id, &input.session_id.0);
        if self
            .host
            .thread(&thread)
            .await
            .map_err(|_| {
                AcpError::new(
                    AcpRegistryOperationErrorReason::SessionDeleteFailed,
                    "Could not inspect the imported ACP session mapping.",
                )
            })?
            .is_some()
        {
            return Err(AcpError::new(
                AcpRegistryOperationErrorReason::SessionDeleteFailed,
                "Delete the imported T3 thread before deleting its native ACP session.",
            ));
        }
        instance
            .manager
            .delete_session(&cwd, &input.session_id.0)
            .await?;
        Ok(AcpRegistryDeleteSessionResult { deleted: True })
    }
    pub async fn list_providers(
        &self,
        input: AcpRegistryListProvidersInput,
    ) -> AcpResult<AcpRegistryListProvidersResult> {
        let cwd = self.host.project_workspace(&input.project_id).await?;
        let instance = self.host.instance(&input.instance_id).await?;
        if instance.snapshot.configurable_providers != Some(Some(true)) {
            return Err(AcpError::new(
                AcpRegistryOperationErrorReason::ProvidersUnsupported,
                "The ACP agent does not advertise provider configuration.",
            ));
        }
        instance.manager.list_providers(&cwd).await
    }
    pub async fn set_provider(
        &self,
        input: Sensitive<AcpRegistrySetProviderInput>,
    ) -> AcpResult<AcpRegistrySetProviderResult> {
        let input = input.expose();
        let cwd = self.host.project_workspace(&input.project_id).await?;
        let instance = self.host.instance(&input.instance_id).await?;
        if input
            .headers
            .as_ref()
            .is_some_and(|headers| headers.len() > 32)
        {
            return Err(AcpError::new(
                AcpRegistryOperationErrorReason::ProviderConfigurationFailed,
                "ACP provider configuration accepts at most 32 headers.",
            ));
        }
        let listed = instance.manager.list_providers(&cwd).await?;
        if !listed.providers.iter().any(|provider| {
            provider.provider_id == input.provider_id
                && provider.supported.contains(&input.api_type)
        }) {
            return Err(AcpError::new(
                AcpRegistryOperationErrorReason::ProviderConfigurationFailed,
                "The ACP agent did not advertise this provider and API type.",
            ));
        }
        self.host
            .close_sessions(&input.instance_id)
            .await
            .map_err(|_| {
                AcpError::new(
                    AcpRegistryOperationErrorReason::ProviderConfigurationFailed,
                    "Could not stop live sessions before updating the ACP provider.",
                )
            })?;
        instance
            .manager
            .set_provider(&cwd, Sensitive::new(input.clone()))
            .await?;
        self.host.refresh(&input.instance_id).await?;
        Ok(AcpRegistrySetProviderResult { configured: True })
    }
    pub async fn disable_provider(
        &self,
        input: AcpRegistryDisableProviderInput,
    ) -> AcpResult<AcpRegistryDisableProviderResult> {
        let cwd = self.host.project_workspace(&input.project_id).await?;
        let instance = self.host.instance(&input.instance_id).await?;
        let listed = instance.manager.list_providers(&cwd).await?;
        if !listed
            .providers
            .iter()
            .any(|provider| provider.provider_id == input.provider_id && !provider.required)
        {
            return Err(AcpError::new(
                AcpRegistryOperationErrorReason::ProviderConfigurationFailed,
                "The ACP provider was not advertised or is required and cannot be disabled.",
            ));
        }
        self.host
            .close_sessions(&input.instance_id)
            .await
            .map_err(|_| {
                AcpError::new(
                    AcpRegistryOperationErrorReason::ProviderConfigurationFailed,
                    "Could not stop live sessions before disabling the ACP provider.",
                )
            })?;
        instance
            .manager
            .disable_provider(&cwd, &input.provider_id.0)
            .await?;
        self.host.refresh(&input.instance_id).await?;
        Ok(AcpRegistryDisableProviderResult { disabled: True })
    }
    pub async fn logout(
        &self,
        input: AcpRegistryLogoutInput,
    ) -> AcpResult<AcpRegistryLogoutResult> {
        let instance = self.host.instance(&input.instance_id).await?;
        if instance.snapshot.auth.can_logout != Some(Some(true)) {
            return Err(AcpError::new(
                AcpRegistryOperationErrorReason::LogoutUnsupported,
                "The ACP agent does not advertise logout.",
            ));
        }
        if instance.managed_auth {
            self.auth
                .logout(t3_contracts::provider_setup::ProviderSetupInput {
                    instance_id: input.instance_id.clone(),
                })
                .await
                .map_err(|_| {
                    AcpError::new(
                        AcpRegistryOperationErrorReason::LogoutFailed,
                        "Could not sign out of the ACP agent.",
                    )
                })?;
        } else {
            self.host
                .close_sessions(&input.instance_id)
                .await
                .map_err(|_| {
                    AcpError::new(
                        AcpRegistryOperationErrorReason::LogoutFailed,
                        "Could not stop live sessions before ACP logout.",
                    )
                })?;
            instance.manager.logout(&self.cwd).await?;
        }
        self.host.refresh(&input.instance_id).await?;
        Ok(AcpRegistryLogoutResult { logged_out: True })
    }
    pub async fn import_session(
        &self,
        input: AcpRegistryImportSessionInput,
    ) -> AcpResult<AcpRegistryImportSessionResult> {
        let _guard = self.coordinator.session_mutation.lock().await;
        let cwd = self.host.project_workspace(&input.project_id).await?;
        let instance = self.host.instance(&input.instance_id).await?;
        if !instance
            .snapshot
            .native_sessions
            .as_ref()
            .and_then(Option::as_ref)
            .is_some_and(|sessions| sessions.can_load || sessions.can_resume)
        {
            return Err(AcpError::new(
                AcpRegistryOperationErrorReason::SessionResumeUnsupported,
                "The ACP agent cannot load or resume native sessions.",
            ));
        }
        let thread = imported_thread_id(&instance.driver, &input.instance_id, &input.session_id.0);
        if let Some(existing) = self.host.thread(&thread).await.map_err(|_| {
            AcpError::new(
                AcpRegistryOperationErrorReason::SessionImportFailed,
                "Could not inspect the imported ACP session mapping.",
            )
        })? {
            self.host.import_history(&thread, &cwd).await.map_err(|_| {
                AcpError::new(
                    AcpRegistryOperationErrorReason::SessionImportFailed,
                    "Could not import the ACP session history.",
                )
            })?;
            return Ok(AcpRegistryImportSessionResult {
                thread_id: thread,
                imported: existing.native_history_pending,
            });
        }
        let model = instance
            .snapshot
            .models
            .iter()
            .find(|model| model.is_default == Some(Some(true)))
            .or_else(|| instance.snapshot.models.first())
            .map_or("default", |model| model.slug.0.as_str());
        let error = || {
            AcpError::new(
                AcpRegistryOperationErrorReason::SessionImportFailed,
                "Could not create a T3 thread for the ACP session.",
            )
        };
        let launch=serde_json::from_value(json!({"commandId":uuid::Uuid::new_v4().to_string(),"threadId":thread,"projectId":input.project_id,"title":input.title.as_ref().and_then(Option::as_ref).map_or("Imported ACP session",|title|title.0.as_str()),"modelSelection":{"instanceId":input.instance_id,"model":model},"runtimeMode":"approval-required","interactionMode":"default","workspaceStrategy":{"type":"root"},"creationSource":"web"})).map_err(|_|error())?;
        let mut metadata = json!({"itemIdentityVersion":2});
        if let Some(title) = input.title {
            metadata["title"] = json!(title);
        }
        if let Some(updated) = input.updated_at {
            metadata["updatedAt"] = json!(updated);
        }
        let native=serde_json::from_value(json!({"ref":{"driver":instance.driver,"nativeId":input.session_id,"strength":"strong"},"metadata":metadata})).map_err(|_|error())?;
        if self
            .host
            .launch_import(ImportLaunch {
                launch,
                native,
                created_by: OrchestrationV2Actor::User,
            })
            .await
            .is_err()
        {
            if self
                .host
                .thread(&thread)
                .await
                .map_err(|_| {
                    AcpError::new(
                        AcpRegistryOperationErrorReason::SessionImportFailed,
                        "Could not inspect the imported ACP session after launch failed.",
                    )
                })?
                .is_some()
            {
                return Ok(AcpRegistryImportSessionResult {
                    thread_id: thread,
                    imported: false,
                });
            }
            return Err(error());
        }
        let _ = self.host.import_history(&thread, &cwd).await;
        Ok(AcpRegistryImportSessionResult {
            thread_id: thread,
            imported: true,
        })
    }
}
