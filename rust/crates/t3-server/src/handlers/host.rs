//! Package A: host-service RPCs: client activity, npm release lookup, triage preparation and
//! database storage.
#[cfg(feature = "host")]
use crate::registry::{Failure, ServerDefect};
use crate::{registry::HandlerRegistry, services::Domains};
use t3_contracts::rpc::*;

pub fn register_with(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    register_cleanup(registry, domains)?;
    #[cfg(feature = "host")]
    if let Some(host) = domains.get::<crate::adapters::host::HostAdapter>() {
        let h = host.clone();
        registry.register::<WsServerReportClientActivityRpc, _, _>(move |ctx, input| {
            let host = h.clone();
            async move {
                // The lease belongs to this socket; a call outside a socket has none to renew.
                let client = ctx
                    .call
                    .socket_id()
                    .and_then(crate::adapters::host::HostAdapter::rpc_client)
                    .ok_or_else(|| {
                        Failure::Defect(ServerDefect::Internal {
                            message: "client activity needs a WebSocket connection".into(),
                        })
                    })?;
                host.background
                    .report_client_activity(ctx.session_id.clone(), client, input)
                    .await
                    .map_err(|e| {
                        Failure::Defect(ServerDefect::Internal {
                            message: e.to_string(),
                        })
                    })
            }
        })?;
        let h = host.clone();
        registry.register::<WsServerGetNpmReleaseRpc, _, _>(move |_, input| {
            let h = h.clone();
            async move {
                h.npm.get(&input.spec).await.map_err(|error| {
                    Failure::Typed(WsServerGetNpmReleaseError::ServerNpmReleaseError(Box::new(
                        error,
                    )))
                })
            }
        })?;
        let h = host.clone();
        registry.register::<WsServerPrepareTriageRpc, _, _>(move |_, input| {
            let h = h.clone();
            async move {
                h.triage.prepare(input).await.map_err(|error| {
                    Failure::Typed(WsServerPrepareTriageError::ServerPrepareTriageError(
                        Box::new(t3_contracts::server::ServerPrepareTriageError {
                            tag: t3_contracts::server::ServerPrepareTriageErrorTag::ServerPrepareTriageError,
                            cause: Some(Some(serde_json::json!({"message": error.to_string()}))),
                        }),
                    ))
                })
            }
        })?;
        #[cfg(feature = "orchestration")]
        {
            let h = host.clone();
            // The current status (null until known), then each change; dropping it unsubscribes.
            registry.register_stream::<WsServerGetDatabaseStorageRpc, _, _, _>(move |_, _| {
                let mut status = h.storage.service.status();
                async move {
                    Ok(async_stream::stream! {
                        loop {
                            let current = status.borrow_and_update().clone();
                            yield Ok(current);
                            if status.changed().await.is_err() {
                                break;
                            }
                        }
                    })
                }
            })?;
            let h = host;
            registry.register::<WsServerReclaimDatabaseSpaceRpc, _, _>(move |_, _| {
                let h = h.clone();
                async move {
                    h.storage.service.request_reclaim().await.map_err(|error| {
                        Failure::Typed(WsServerReclaimDatabaseSpaceError::DatabaseReclaimError(
                            Box::new(error),
                        ))
                    })
                }
            })?;
        }
        #[cfg(not(feature = "orchestration"))]
        {
            registry.unimplemented::<WsServerGetDatabaseStorageRpc>()?;
            registry.unimplemented::<WsServerReclaimDatabaseSpaceRpc>()?;
        }
        return Ok(());
    }
    let _ = domains;
    registry.unimplemented::<WsServerReportClientActivityRpc>()?;
    registry.unimplemented::<WsServerGetNpmReleaseRpc>()?;
    registry.unimplemented::<WsServerPrepareTriageRpc>()?;
    registry.unimplemented::<WsServerGetDatabaseStorageRpc>()?;
    registry.unimplemented::<WsServerReclaimDatabaseSpaceRpc>()
}

fn register_cleanup(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    #[cfg(feature = "storage-cleanup")]
    if let Some(cleanup) = domains.get::<crate::adapters::storage_cleanup::CleanupAdapter>() {
        let reports = cleanup.clone();
        registry.register_stream::<WsServerGetStorageCleanupReportRpc, _, _, _>(move |_, _| {
            let reports = reports.clone();
            async move {
                let mut receiver = reports.service.reports().await;
                Ok(async_stream::stream! {
                    while let Some(report) = receiver.recv().await { yield Ok(report); }
                })
            }
        })?;
        return registry.register::<WsServerRunStorageCleanupRpc, _, _>(move |ctx, _| {
            let cleanup = cleanup.clone();
            async move {
                cleanup.run().await.map_err(|error| {
                    Failure::Typed(WsServerRunStorageCleanupError::ServerSettingsError(
                        Box::new(t3_contracts::settings::ServerSettingsError {
                            _tag:
                                t3_contracts::settings::ServerSettingsErrorTag::ServerSettingsError,
                            settings_path: ctx
                                .services
                                .local
                                .config
                                .paths
                                .settings()
                                .display()
                                .to_string(),
                            operation: t3_contracts::settings::ServerSettingsOperation::ReadFile,
                            provider_instance_id: None,
                            environment_variable: None,
                            managed_setting: None,
                            managed_by: None,
                            provider_label: None,
                            cause: Some(Some(
                                serde_json::json!({"name":"Error","message":error.to_string()}),
                            )),
                        }),
                    ))
                })
            }
        });
    }
    let _ = domains;
    registry.unimplemented::<WsServerGetStorageCleanupReportRpc>()?;
    registry.unimplemented::<WsServerRunStorageCleanupRpc>()
}
