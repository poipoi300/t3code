//! ACP Registry agent management (`ws.ts` `server.*AcpRegistry*`): sign-in consent, native
//! sessions, configurable model providers and logout, each one call into the provider domain's
//! `AcpRegistryService`. The RPC guard has already applied each method's scope.
//! `server.importAcpRegistrySession` stays unserved (registered in `handlers/providers.rs`).
use crate::{registry::HandlerRegistry, services::Domains};
use t3_contracts::rpc::*;

pub fn register_with(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    #[cfg(feature = "providers")]
    if let Some(adapter) = domains.get::<crate::adapters::providers::ProvidersAdapter>() {
        return live::register(registry, adapter);
    }
    let _ = domains;
    registry.unimplemented::<WsServerAcceptAcpRegistryUrlAuthRpc>()?;
    registry.unimplemented::<WsServerListAcpRegistrySessionsRpc>()?;
    registry.unimplemented::<WsServerDeleteAcpRegistrySessionRpc>()?;
    registry.unimplemented::<WsServerListAcpRegistryProvidersRpc>()?;
    registry.unimplemented::<WsServerSetAcpRegistryProviderRpc>()?;
    registry.unimplemented::<WsServerDisableAcpRegistryProviderRpc>()?;
    registry.unimplemented::<WsServerLogoutAcpRegistryRpc>()?;
    Ok(())
}

#[cfg(feature = "providers")]
mod live {
    use super::*;
    use crate::registry::Failure;
    use t3_provider::acp_service::AcpError;

    fn typed(error: AcpError) -> Failure<WsServerSearchAcpRegistryError> {
        Failure::Typed(WsServerSearchAcpRegistryError::AcpRegistryOperationError(
            Box::new(error.wire()),
        ))
    }

    pub fn register(
        registry: &mut HandlerRegistry,
        adapter: std::sync::Arc<crate::adapters::providers::ProvidersAdapter>,
    ) -> crate::Result<()> {
        macro_rules! unary {
            ($rpc:ty, |$acp:ident, $input:ident| $call:expr) => {{
                let a = adapter.clone();
                registry.register::<$rpc, _, _>(move |_, $input| {
                    let a = a.clone();
                    async move {
                        let $acp = &a.domain.acp;
                        $call.await.map_err(typed)
                    }
                })?;
            }};
        }
        let a = adapter.clone();
        registry.register::<WsServerAcceptAcpRegistryUrlAuthRpc, _, _>(move |_, input| {
            let a = a.clone();
            async move {
                Ok::<_, Failure<t3_contracts::auth::EnvironmentAuthorizationError>>(
                    a.domain.acp.accept_url_authentication(input),
                )
            }
        })?;
        unary!(WsServerListAcpRegistrySessionsRpc, |acp, input| acp
            .list_sessions(input));
        unary!(WsServerDeleteAcpRegistrySessionRpc, |acp, input| acp
            .delete_session(input));
        unary!(WsServerListAcpRegistryProvidersRpc, |acp, input| acp
            .list_providers(input));
        unary!(WsServerSetAcpRegistryProviderRpc, |acp, input| acp
            .set_provider(t3_provider::sensitive::Sensitive::new(input)));
        unary!(WsServerDisableAcpRegistryProviderRpc, |acp, input| acp
            .disable_provider(input));
        unary!(WsServerLogoutAcpRegistryRpc, |acp, input| acp.logout(input));
        Ok(())
    }
}
