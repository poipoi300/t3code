use super::super::{Registry, register};
use t3_contracts::relay_client::*;

pub fn register_module(registry: &mut Registry) {
    register::<RelayClientInstallFailedError>(registry, "RelayClientInstallFailedError");
    register::<RelayClientInstallFailureReasonSchema>(
        registry,
        "RelayClientInstallFailureReasonSchema",
    );
    register::<RelayClientInstallProgressEventSchema>(
        registry,
        "RelayClientInstallProgressEventSchema",
    );
    register::<RelayClientInstallProgressStageSchema>(
        registry,
        "RelayClientInstallProgressStageSchema",
    );
    register::<RelayClientStatusSchema>(registry, "RelayClientStatusSchema");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("relay_client", register_module)
}
