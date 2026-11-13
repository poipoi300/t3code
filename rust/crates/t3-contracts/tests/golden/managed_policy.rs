use super::super::{Registry, register};
use t3_contracts::managed_policy::*;
pub fn register_module(registry: &mut Registry) {
    register::<ManagedPolicyClient>(registry, "ManagedPolicyClient");
    register::<ManagedPolicyDocument>(registry, "ManagedPolicyDocument");
    register::<ManagedPolicyLimits>(registry, "ManagedPolicyLimits");
    register::<ManagedPolicySetting>(registry, "ManagedPolicySetting");
    register::<ManagedPolicySummary>(registry, "ManagedPolicySummary");
    register::<ManagedPolicySummaryLimits>(registry, "ManagedPolicySummaryLimits");
    register::<ManagedProviderInstance>(registry, "ManagedProviderInstance");
    register::<ManagedProviderLimits>(registry, "ManagedProviderLimits");
    register::<ManagedUpdatesMode>(registry, "ManagedUpdatesMode");
    register::<ManagedUpdatesPolicy>(registry, "ManagedUpdatesPolicy");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("managed_policy", register_module)
}
