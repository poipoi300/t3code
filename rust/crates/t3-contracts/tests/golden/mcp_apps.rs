use super::super::{Registry, register};
use t3_contracts::mcp_apps::*;
pub fn register_module(registry: &mut Registry) {
    register::<McpAppCallToolInput>(registry, "McpAppCallToolInput");
    register::<McpAppCallToolResult>(registry, "McpAppCallToolResult");
    register::<McpAppReadResourceInput>(registry, "McpAppReadResourceInput");
    register::<McpAppReadResourceResult>(registry, "McpAppReadResourceResult");
    register::<McpAppRequestError>(registry, "McpAppRequestError");
    register::<McpAppRequestErrorReason>(registry, "McpAppRequestErrorReason");
    register::<McpAppToolInfo>(registry, "McpAppToolInfo");
    register::<McpAppToolInfoInput>(registry, "McpAppToolInfoInput");
    register::<McpAppUpdateModelContextInput>(registry, "McpAppUpdateModelContextInput");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("mcp_apps", register_module)
}
