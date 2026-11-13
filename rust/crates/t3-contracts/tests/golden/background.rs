use super::super::{Registry, register};
use t3_contracts::background::*;

pub fn register_module(registry: &mut Registry) {
    register::<BackgroundBooleanState>(registry, "BackgroundBooleanState");
    register::<HostPowerThermalState>(registry, "HostPowerThermalState");
    register::<HostPowerSource>(registry, "HostPowerSource");
    register::<HostPowerSnapshot>(registry, "HostPowerSnapshot");
    register::<BackgroundScope>(registry, "BackgroundScope");
    register::<ClientKind>(registry, "ClientKind");
    register::<ClientActivityClientId>(registry, "ClientActivityClientId");
    register::<ClientActivityReportInput>(registry, "ClientActivityReportInput");
    register::<ClientActivityLease>(registry, "ClientActivityLease");
    register::<BackgroundPolicySnapshot>(registry, "BackgroundPolicySnapshot");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("background", register_module)
}
