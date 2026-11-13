use super::super::{Registry, register};
use t3_contracts::provider_runtime::*;

pub fn register_module(registry: &mut Registry) {
    register::<ToolLifecycleItemType>(registry, "ToolLifecycleItemType");
    register::<ThreadTokenUsageSnapshot>(registry, "ThreadTokenUsageSnapshot");
    register::<TurnTokenUsage>(registry, "TurnTokenUsage");
    register::<ToolActivitySurface>(registry, "ToolActivitySurface");
    register::<ToolActivityNativeAppReference>(registry, "ToolActivityNativeAppReference");
    register::<ToolActivityIcon>(registry, "ToolActivityIcon");
    register::<ToolActivitySource>(registry, "ToolActivitySource");
    register::<UserInputQuestion>(registry, "UserInputQuestion");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("provider_runtime", register_module)
}
