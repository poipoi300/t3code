//! Package A: the relay client RPCs (`cloud.getRelayClientStatus`, `cloud.installRelayClient`).
//! The RPC guard checks `relay:read` / `relay:write` first; the cloud services check the
//! verified session's scopes again.
use crate::{registry::HandlerRegistry, services::Domains};
use t3_contracts::rpc::*;

pub fn register_with(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    #[cfg(feature = "cloud")]
    if let Some(cloud) = domains.get::<crate::adapters::cloud::CloudAdapter>() {
        use crate::registry::{Failure, ServerDefect};
        fn internal<E>(error: crate::adapters::cloud::CloudError) -> Failure<E> {
            Failure::Defect(ServerDefect::Internal {
                message: error.to_string(),
            })
        }
        let c = cloud.clone();
        registry.register::<WsCloudGetRelayClientStatusRpc, _, _>(move |ctx, _| {
            let c = c.clone();
            async move {
                let context = c.request_context(&ctx.session_id, &ctx.scopes);
                c.relay_client.status(&context).await.map_err(internal)
            }
        })?;
        let c = cloud;
        // Progress events, then completion; a terminal install failure fails the stream.
        // Dropping the stream cancels the install it owns.
        registry.register_stream::<WsCloudInstallRelayClientRpc, _, _, _>(move |ctx, _| {
            let c = c.clone();
            async move {
                use futures::StreamExt;
                let context = c.request_context(&ctx.session_id, &ctx.scopes);
                let install = c
                    .relay_client
                    .install_with_progress(&context)
                    .map_err(internal)?;
                Ok(install.map(|item| {
                    item.map_err(|error| {
                        Failure::Typed(
                            WsCloudInstallRelayClientError::RelayClientInstallFailedError(
                                Box::new(error),
                            ),
                        )
                    })
                }))
            }
        })?;
        return Ok(());
    }
    let _ = domains;
    registry.unimplemented::<WsCloudGetRelayClientStatusRpc>()?;
    registry.unimplemented::<WsCloudInstallRelayClientRpc>()
}
