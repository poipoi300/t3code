use super::super::{Registry, register};
use t3_contracts::provider_usage_limits::*;
pub fn register_module(registry: &mut Registry) {
    register::<ProviderConsumeResetCreditInput>(registry, "ProviderConsumeResetCreditInput");
    register::<ProviderConsumeResetCreditOutcome>(registry, "ProviderConsumeResetCreditOutcome");
    register::<ProviderConsumeResetCreditResult>(registry, "ProviderConsumeResetCreditResult");
    register::<ProviderUsageLimitsUpdate>(registry, "ProviderUsageLimitsUpdate");
    register::<ServerProviderResetCreditOption>(registry, "ServerProviderResetCreditOption");
    register::<ServerProviderResetCredits>(registry, "ServerProviderResetCredits");
    register::<ServerProviderUsageLimits>(registry, "ServerProviderUsageLimits");
    register::<ServerProviderUsageWindow>(registry, "ServerProviderUsageWindow");
    register::<UsageLimitSourceAccount>(registry, "UsageLimitSourceAccount");
    register::<UsageLimitSourceConsumeResetCreditInput>(
        registry,
        "UsageLimitSourceConsumeResetCreditInput",
    );
    register::<UsageLimitSourceError>(registry, "UsageLimitSourceError");
    register::<UsageLimitSourceSnapshot>(registry, "UsageLimitSourceSnapshot");
    register::<UsageLimitSourceSnapshots>(registry, "UsageLimitSourceSnapshots");
    register::<UsageLimitsReport>(registry, "UsageLimitsReport");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("provider_usage_limits", register_module)
}
