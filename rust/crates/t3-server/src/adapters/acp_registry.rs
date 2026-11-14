//! The ACP Registry application host (TypeScript `AcpRegistrySessions` and the handler helpers
//! in `ws.ts`): projects, imported-thread mappings, live sessions and status refreshes for the
//! provider domain's `AcpRegistryService`.
//!
//! The provider domain is built with this host and owns it, so the host holds the domain weakly
//! and is bound once the domain exists. Every working directory it hands out is a project root
//! from the projection, never a path from a client.
use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, Weak},
};
use t3_contracts::{
    acp_registry::AcpRegistryOperationErrorReason as Reason,
    base_schemas::{ProjectId, ThreadId},
    orchestration_v2::OrchestrationV2ThreadHistoryOrigin,
    provider_instance::ProviderInstanceId,
};
use t3_orchestration::composition::OrchestrationRuntime;
use t3_provider::{acp_service::*, domain::ProviderDomain};

pub struct AcpHost {
    runtime: Arc<OrchestrationRuntime>,
    domain: OnceLock<Weak<ProviderDomain>>,
}

impl AcpHost {
    pub fn new(runtime: Arc<OrchestrationRuntime>) -> Arc<Self> {
        Arc::new(Self {
            runtime,
            domain: OnceLock::new(),
        })
    }

    /// Binds the provider domain built with this host.
    pub fn bind(&self, domain: &Arc<ProviderDomain>) {
        let _ = self.domain.set(Arc::downgrade(domain));
    }

    fn domain(&self) -> AcpResult<Arc<ProviderDomain>> {
        self.domain.get().and_then(Weak::upgrade).ok_or_else(|| {
            AcpError::new(
                Reason::InstanceNotFound,
                "Provider services are not available.",
            )
        })
    }
}

fn import_unavailable() -> AcpError {
    AcpError::new(
        Reason::SessionImportFailed,
        "Importing ACP sessions is not available on this server.",
    )
}

#[async_trait::async_trait]
impl AcpSessionHost for AcpHost {
    /// `AcpRegistrySessions.project`: a live (not deleted) project's workspace root.
    async fn project_workspace(&self, project: &ProjectId) -> AcpResult<PathBuf> {
        let id = project.clone();
        let row = self
            .runtime
            .projections
            .read_snapshot(move |db| t3_orchestration::stores::projects::get(db, &id, false))
            .await
            .map_err(|_| AcpError::new(Reason::ProjectNotFound, "The project is unavailable."))?;
        row.map(|row| PathBuf::from(row.workspace_root))
            .ok_or_else(|| AcpError::new(Reason::ProjectNotFound, "The project was not found."))
    }

    /// `AcpRegistrySessions.sessionManager`.
    async fn instance(&self, instance: &ProviderInstanceId) -> AcpResult<AcpInstance> {
        let domain = self.domain()?;
        if domain.instances.get(instance).await.is_none() {
            return Err(AcpError::new(
                Reason::InstanceNotFound,
                "The provider instance was not found.",
            ));
        }
        let Some(registry) = domain.acp_instances.get(instance) else {
            return Err(AcpError::new(
                Reason::SessionListUnsupported,
                "The provider instance does not expose ACP session management.",
            ));
        };
        let snapshot = registry.snapshot().await.map_err(|_| {
            AcpError::new(
                Reason::InstanceNotFound,
                "The provider instance is unavailable.",
            )
        })?;
        Ok(AcpInstance {
            driver: registry.driver.clone(),
            snapshot,
            manager: registry.manager(),
            // Every registry instance has a sign-in controller (`AcpRegistryDriver`'s `auth`).
            managed_auth: true,
        })
    }

    /// `threadManagement.getThreadShell` plus `AgentSessionImporter.isNativeHistoryPending`.
    async fn thread(&self, thread: &ThreadId) -> AcpResult<Option<ImportedThread>> {
        let id = thread.clone();
        self.runtime
            .projections
            .read_snapshot(move |db| {
                let Some(thread) = t3_orchestration::projections::read::thread(db, &id)? else {
                    return Ok(None);
                };
                let has_run: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM orchestration_v2_projection_runs WHERE thread_id=?1)",
                    [&id.0],
                    |row| row.get(0),
                )?;
                let imported = thread.history_origin.flatten()
                    == Some(OrchestrationV2ThreadHistoryOrigin::V1Import);
                Ok(Some(ImportedThread {
                    native_history_pending: !imported && !has_run,
                }))
            })
            .await
            .map_err(|_| {
                AcpError::new(
                    Reason::SessionImportFailed,
                    "Could not inspect existing imported ACP sessions.",
                )
            })
    }

    async fn launch_import(&self, _: ImportLaunch) -> AcpResult<()> {
        Err(import_unavailable())
    }

    async fn import_history(&self, _: &ThreadId, _: &Path) -> AcpResult<()> {
        Err(import_unavailable())
    }

    /// `providerSessionManager.closeInstance`.
    async fn close_sessions(&self, instance: &ProviderInstanceId) -> AcpResult<()> {
        self.runtime
            .sessions
            .close_instance(instance)
            .await
            .map_err(|_| {
                AcpError::new(
                    Reason::ProviderConfigurationFailed,
                    "Could not stop live sessions.",
                )
            })
    }

    /// `providerRegistry.refreshInstance`: a fresh status check; it never fails the operation.
    async fn refresh(&self, instance: &ProviderInstanceId) -> AcpResult<()> {
        let domain = self.domain()?;
        let Ok(input) =
            serde_json::from_value(serde_json::json!({"instanceId": instance, "fresh": true}))
        else {
            return Ok(());
        };
        if domain.providers.refresh_providers(input).await.is_err() {
            tracing::warn!(instance = %instance.0, "ACP Registry status refresh failed");
        }
        Ok(())
    }
}
