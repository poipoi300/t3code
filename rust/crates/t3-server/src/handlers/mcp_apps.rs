//! `mcpApps.*` (`ws.ts`: `mcpAppRequests.callTool`, `toolInfo`, `updateModelContext`,
//! `readResource`). The RPC guard applies each method's scope (operate for calls and context,
//! read for tool info and resources) before the handler runs.
use crate::{registry::HandlerRegistry, services::Domains};
use t3_contracts::rpc::*;

#[cfg(feature = "mcp")]
mod live {
    use super::*;
    use crate::registry::{Failure, ServerDefect};
    use t3_mcp::McpError;

    /// `McpAppRequestError` with its reason; anything else is a defect, as in TypeScript.
    fn failure(error: McpError) -> Failure<WsMcpAppsCallToolError> {
        if let McpError::Domain { tag, fields, .. } = &error
            && tag == "McpAppRequestError"
        {
            let mut value = fields.clone();
            value["_tag"] = "McpAppRequestError".into();
            if let Ok(typed) = serde_json::from_value(value) {
                return Failure::Typed(WsMcpAppsCallToolError::McpAppRequestError(Box::new(
                    typed,
                )));
            }
        }
        Failure::Defect(ServerDefect::Internal {
            message: error.to_string(),
        })
    }

    pub fn register(
        registry: &mut HandlerRegistry,
        adapter: std::sync::Arc<crate::adapters::mcp::McpAdapter>,
    ) -> crate::Result<()> {
        macro_rules! unary {
            ($rpc:ty, $method:ident) => {{
                let a = adapter.clone();
                registry.register::<$rpc, _, _>(move |_, input| {
                    let a = a.clone();
                    async move { a.apps.$method(input).await.map_err(failure) }
                })?;
            }};
        }
        unary!(WsMcpAppsCallToolRpc, call_tool);
        unary!(WsMcpAppsToolInfoRpc, tool_info);
        unary!(WsMcpAppsUpdateModelContextRpc, update_model_context);
        unary!(WsMcpAppsReadResourceRpc, read_resource);
        Ok(())
    }
}

pub fn register_with(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    #[cfg(feature = "mcp")]
    if let Some(adapter) = domains.get::<crate::adapters::mcp::McpAdapter>() {
        return live::register(registry, adapter);
    }
    let _ = domains;
    registry.unimplemented::<WsMcpAppsCallToolRpc>()?;
    registry.unimplemented::<WsMcpAppsToolInfoRpc>()?;
    registry.unimplemented::<WsMcpAppsUpdateModelContextRpc>()?;
    registry.unimplemented::<WsMcpAppsReadResourceRpc>()?;
    Ok(())
}
