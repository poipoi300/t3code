use super::super::{Registry, register};
use t3_contracts::usage::*;
pub fn register_module(registry: &mut Registry) {
    register::<UsageBucket>(registry, "UsageBucket");
    register::<UsageCategoryCost>(registry, "UsageCategoryCost");
    register::<UsageCostSource>(registry, "UsageCostSource");
    register::<UsageDay>(registry, "UsageDay");
    register::<UsageModelFilter>(registry, "UsageModelFilter");
    register::<UsageModelSelector>(registry, "UsageModelSelector");
    register::<UsagePricing>(registry, "UsagePricing");
    register::<UsagePricingStatus>(registry, "UsagePricingStatus");
    register::<UsageProviderKind>(registry, "UsageProviderKind");
    register::<UsageReadError>(registry, "UsageReadError");
    register::<UsageResolution>(registry, "UsageResolution");
    register::<UsageSource>(registry, "UsageSource");
    register::<UsageSourceFingerprint>(registry, "UsageSourceFingerprint");
    register::<UsageSourceStatus>(registry, "UsageSourceStatus");
    register::<UsageSummary>(registry, "UsageSummary");
    register::<UsageSummaryInput>(registry, "UsageSummaryInput");
    register::<UsageTokenTotals>(registry, "UsageTokenTotals");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("usage", register_module)
}
