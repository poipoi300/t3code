use crate::{registry::HandlerRegistry, services::Domains};
use t3_contracts::rpc::*;
pub const METHODS: &[&str] = &[
    "server.refreshProviders",
    "server.updateProvider",
    "provider.consumeResetCredit",
    "provider.auth.start",
    "provider.auth.respond",
    "provider.auth.complete",
    "provider.chatgpt.reconnect-profile",
    "provider.chatgpt.import-profile",
    "provider.chatgpt.handoff.subscribe",
    "provider.auth.cancel",
    "provider.auth.removeMethod",
    "provider.auth.logout",
    "provider.auth.subscribe",
    "provider.install.start",
    "provider.install.cancel",
    "provider.install.subscribe",
    "provider.install.remove",
    "server.searchAcpRegistry",
    "server.prepareAcpRegistryAgent",
    "server.uninstallAcpRegistryManagedBinary",
    "server.acceptAcpRegistryUrlAuth",
    "server.listAcpRegistrySessions",
    "server.importAcpRegistrySession",
    "server.deleteAcpRegistrySession",
    "server.listAcpRegistryProviders",
    "server.setAcpRegistryProvider",
    "server.disableAcpRegistryProvider",
    "server.logoutAcpRegistry",
    "server.listTextGeneratorModels",
];
pub fn register_with(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    #[cfg(feature = "providers")]
    if let Some(adapter) = domains.get::<crate::adapters::providers::ProvidersAdapter>() {
        register_status(registry, adapter.clone())?;
        register_mutations(registry, adapter)?;
        return registry.unimplemented::<WsProviderConsumeResetCreditRpc>();
    }
    let _ = domains;
    registry.unimplemented::<WsProviderAuthSubscribeRpc>()?;
    registry.unimplemented::<WsProviderInstallSubscribeRpc>()?;
    registry.unimplemented::<WsServerSearchAcpRegistryRpc>()?;
    registry.unimplemented::<WsServerListTextGeneratorModelsRpc>()?;
    register_remaining(registry)
}
fn register_remaining(registry: &mut HandlerRegistry) -> crate::Result<()> {
    registry.unimplemented::<WsChatGptReconnectProfileRpc>()?;
    registry.unimplemented::<WsServerUpdateProviderRpc>()?;
    registry.unimplemented::<WsProviderAuthStartRpc>()?;
    registry.unimplemented::<WsProviderAuthRespondRpc>()?;
    registry.unimplemented::<WsProviderAuthCompleteRpc>()?;
    registry.unimplemented::<WsChatGptImportProfileRpc>()?;
    registry.unimplemented::<WsProviderAuthCancelRpc>()?;
    registry.unimplemented::<WsProviderAuthRemoveMethodRpc>()?;
    registry.unimplemented::<WsProviderAuthLogoutRpc>()?;
    registry.unimplemented::<WsProviderInstallStartRpc>()?;
    registry.unimplemented::<WsProviderInstallCancelRpc>()?;
    registry.unimplemented::<WsProviderInstallRemoveRpc>()?;
    registry.unimplemented::<WsServerPrepareAcpRegistryAgentRpc>()?;
    registry.unimplemented::<WsServerUninstallAcpRegistryManagedBinaryRpc>()?;
    register_blocked(registry)
}
fn register_blocked(registry: &mut HandlerRegistry) -> crate::Result<()> {
    registry.unimplemented::<WsServerRefreshProvidersRpc>()?;
    registry.unimplemented::<WsProviderConsumeResetCreditRpc>()?;
    registry.unimplemented::<WsChatGptHandoffSubscribeRpc>()?;
    // The ACP Registry methods are registered by `handlers/acp_registry.rs`.
    Ok(())
}

#[cfg(feature = "providers")]
fn register_status(
    registry: &mut HandlerRegistry,
    adapter: std::sync::Arc<crate::adapters::providers::ProvidersAdapter>,
) -> crate::Result<()> {
    use crate::registry::Failure;
    let a = adapter.clone();
    registry.register::<WsServerRefreshProvidersRpc, _, _>(move |ctx, input| {
        let a = a.clone();
        async move {
            let id = input.instance_id.clone().flatten();
            let scan = if input.model_catalog_only.flatten() != Some(true)
                && id.is_some()
                && let Some(cwd) = input.cwd.as_ref().and_then(Option::as_ref)
            {
                Some(
                    a.workspace_scan_directory(&ctx.services, &cwd.0)
                        .await
                        .map_err(|error| {
                            Failure::Defect(crate::registry::ServerDefect::Internal {
                                message: error.to_string(),
                            })
                        })?,
                )
            } else {
                None
            };
            a.domain
                .refresh_providers_at(input, scan.as_deref())
                .await
                .map_err(|failure| {
                    use t3_provider::service::RefreshProvidersError;
                    match failure {
                        RefreshProvidersError::Models { instance_id, error } => {
                            Failure::Typed(WsServerRefreshProvidersError::ProviderSetupError(
                                crate::adapters::providers::setup_error(
                                    &instance_id,
                                    "refresh-models",
                                    &error.to_string(),
                                ),
                            ))
                        }
                        RefreshProvidersError::Service(error) => {
                            Failure::Defect(crate::registry::ServerDefect::Internal {
                                message: error.to_string(),
                            })
                        }
                    }
                })
        }
    })?;
    let a = adapter.clone();
    registry.register_stream::<WsChatGptHandoffSubscribeRpc, _, _, _>(move |ctx, input| {
        let a = a.clone();
        async move {
            // The destination instance belongs to the other environment. The host creates an
            // isolated controller with this socket's session and keeps its profiles in memory.
            let id = input.instance_id.clone();
            let typed = move |error| {
                Failure::Typed(ProviderSetupRpcError::ProviderSetupError(
                    crate::adapters::providers::auth_error(&id, "handoff", error),
                ))
            };
            let mut handoff = a
                .domain
                .handoff
                .subscribe(input, &ctx.session_id.0)
                .await
                .map_err(&typed)?;
            Ok(async_stream::try_stream! {
                while let Some(state) = handoff.next().await.map_err(&typed)? {
                    yield state.into_inner();
                }
            })
        }
    })?;
    let a = adapter.clone();
    registry.register::<WsChatGptReconnectProfileRpc, _, _>(move |_, input| {
        let a = a.clone();
        async move {
            a.reconnect_profile(input)
                .await
                .map_err(|error| Failure::Typed(ProviderSetupRpcError::ProviderSetupError(error)))
        }
    })?;
    let a = adapter.clone();
    registry.register_stream::<WsProviderAuthSubscribeRpc,_,_,_>(move |ctx,input|{
        let a=a.clone();async move {
            let mut subscription=a.auth_status(input,ctx.session_id.0).await.map_err(|error|Failure::Typed(ProviderSetupRpcError::ProviderSetupError(error)))?;
            Ok(async_stream::stream! {while let Some(state)=subscription.next().await {yield Ok(state.into_inner());}})
        }
    })?;
    let a = adapter.clone();
    registry.register_stream::<WsProviderInstallSubscribeRpc,_,_,_>(move |_,input|{
        let a=a.clone();async move {
            let mut subscription=a.install_status(input).await.map_err(|error|Failure::Typed(ProviderSetupRpcError::ProviderSetupError(error)))?;
            Ok(async_stream::stream! {
                loop {let state=subscription.borrow_and_update().clone();yield Ok(state); if subscription.changed().await.is_err(){break;}}
            })
        }
    })?;
    let a = adapter.clone();
    registry.register::<WsServerSearchAcpRegistryRpc, _, _>(move |_, input| {
        let a = a.clone();
        async move {
            a.domain.acp.search(input).await.map_err(|error| {
                Failure::Typed(WsServerSearchAcpRegistryError::AcpRegistryOperationError(
                    Box::new(error.wire()),
                ))
            })
        }
    })?;
    registry.register::<WsServerListTextGeneratorModelsRpc, _, _>(move |_, input| {
        let a = adapter.clone();
        async move {
            a.text_models(input).await.map_err(|error| {
                Failure::Typed(WsServerListTextGeneratorModelsError::TextGeneratorError(
                    Box::new(error),
                ))
            })
        }
    })?;
    Ok(())
}

#[cfg(feature = "providers")]
fn register_mutations(
    registry: &mut HandlerRegistry,
    adapter: std::sync::Arc<crate::adapters::providers::ProvidersAdapter>,
) -> crate::Result<()> {
    use crate::{adapters::providers::setup_error, registry::Failure};
    macro_rules! auth {
        ($rpc:ty,$operation:literal,|$a:ident,$ctx:ident,$input:ident|$call:expr) => {{
            let a = adapter.clone();
            registry.register::<$rpc, _, _>(move |$ctx, $input| {
                let $a = a.clone();
                async move {
                    if $ctx.services.lifecycle.admit_command().is_err() {
                        return Err(Failure::Typed(ProviderSetupRpcError::ProviderSetupError(
                            setup_error(
                                &$input.instance_id,
                                $operation,
                                "The server is not accepting provider changes.",
                            ),
                        )));
                    }
                    $call.await.map_err(|error| {
                        Failure::Typed(ProviderSetupRpcError::ProviderSetupError(error))
                    })
                }
            })?;
        }};
    }
    auth!(WsProviderAuthStartRpc, "start", |a, ctx, input| a
        .start_auth(input, &ctx.session_id.0));
    auth!(WsProviderAuthRespondRpc, "respond", |a, ctx, input| a
        .respond_auth(input, &ctx.session_id.0));
    auth!(WsProviderAuthCompleteRpc, "complete", |a, ctx, input| a
        .complete_auth(input, &ctx.session_id.0));
    auth!(WsProviderAuthCancelRpc, "cancel", |a, ctx, input| a
        .cancel_auth(input, &ctx.session_id.0));
    auth!(
        WsProviderAuthRemoveMethodRpc,
        "removeMethod",
        |a, ctx, input| a.remove_auth(input)
    );
    auth!(WsProviderAuthLogoutRpc, "logout", |a, ctx, input| a
        .logout_auth(input));
    auth!(WsChatGptImportProfileRpc, "import", |a, ctx, input| a
        .import_auth(input));
    auth!(WsProviderInstallStartRpc, "install", |a, ctx, input| a
        .start_install(input));
    auth!(
        WsProviderInstallCancelRpc,
        "cancel-install",
        |a, ctx, input| a.cancel_install(input)
    );
    auth!(
        WsProviderInstallRemoveRpc,
        "remove-install",
        |a, ctx, input| a.remove_install(input)
    );
    let a = adapter.clone();
    registry.register::<WsServerUpdateProviderRpc,_,_>(move |ctx,input|{
        let a=a.clone();async move {
            if ctx.services.lifecycle.admit_command().is_err(){return Err(Failure::Typed(WsServerUpdateProviderError::ServerProviderUpdateError(Box::new(t3_contracts::server::ServerProviderUpdateError{
                tag:t3_contracts::server::ServerProviderUpdateErrorTag::ServerProviderUpdateError,provider:input.provider,reason:"The server is not accepting provider updates.".to_string().into(),cause:None}))));}
            a.update_provider(input).await.map_err(|error|Failure::Typed(WsServerUpdateProviderError::ServerProviderUpdateError(Box::new(error))))
        }
    })?;
    let a = adapter.clone();
    registry.register::<WsServerPrepareAcpRegistryAgentRpc,_,_>(move |ctx,input|{
        let a=a.clone();async move {
            if ctx.services.lifecycle.admit_command().is_err(){return Err(Failure::Typed(WsServerSearchAcpRegistryError::AcpRegistryOperationError(Box::new(t3_provider::acp_service::AcpError::new(t3_contracts::acp_registry::AcpRegistryOperationErrorReason::InstallFailed,"The server is not accepting provider changes.").wire()))));}
            a.domain.acp.prepare(input).await.map_err(|error|Failure::Typed(WsServerSearchAcpRegistryError::AcpRegistryOperationError(Box::new(error.wire()))))
        }
    })?;
    registry.register::<WsServerUninstallAcpRegistryManagedBinaryRpc,_,_>(move |ctx,input|{
        let a=adapter.clone();async move {
            if ctx.services.lifecycle.admit_command().is_err(){return Err(Failure::Typed(WsServerSearchAcpRegistryError::AcpRegistryOperationError(Box::new(t3_provider::acp_service::AcpError::new(t3_contracts::acp_registry::AcpRegistryOperationErrorReason::InstallFailed,"The server is not accepting provider changes.").wire()))));}
            a.domain.acp.uninstall_managed_binary(input).await.map_err(|error|Failure::Typed(WsServerSearchAcpRegistryError::AcpRegistryOperationError(Box::new(error.wire()))))
        }
    })?;
    Ok(())
}
