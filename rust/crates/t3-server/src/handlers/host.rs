//! Package A: host-service RPCs (`server.reportClientActivity`).
#[cfg(feature = "host")]
use crate::registry::{Failure, ServerDefect};
use crate::{registry::HandlerRegistry, services::Domains};
use t3_contracts::rpc::*;

pub fn register_with(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    #[cfg(feature = "host")]
    if let Some(host) = domains.get::<crate::adapters::host::HostAdapter>() {
        registry.register::<WsServerReportClientActivityRpc, _, _>(move |ctx, input| {
            let host = host.clone();
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
        return Ok(());
    }
    let _ = domains;
    registry.unimplemented::<WsServerReportClientActivityRpc>()
}
