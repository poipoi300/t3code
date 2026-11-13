use super::super::{Registry, register};
use t3_contracts::provider_instance::*;

pub fn register_module(registry: &mut Registry) {
    register::<ProviderDriverKind>(registry, "ProviderDriverKind");
    register::<ProviderInstanceId>(registry, "ProviderInstanceId");
    register::<ProviderInstanceEnvironmentVariableName>(
        registry,
        "ProviderInstanceEnvironmentVariableName",
    );
    register::<ProviderInstanceEnvironmentVariable>(
        registry,
        "ProviderInstanceEnvironmentVariable",
    );
    register::<ProviderInstanceEnvironment>(registry, "ProviderInstanceEnvironment");
    register::<ProviderInstanceConfig>(registry, "ProviderInstanceConfig");
    register::<ProviderInstanceMutation>(registry, "ProviderInstanceMutation");
    register::<ProviderInstanceConfigMap>(registry, "ProviderInstanceConfigMap");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("provider_instance", register_module)
}
