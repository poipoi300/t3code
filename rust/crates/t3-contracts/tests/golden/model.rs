use super::super::{Registry, register};
use t3_contracts::model::*;

pub fn register_module(registry: &mut Registry) {
    register::<ProviderOptionDescriptorType>(registry, "ProviderOptionDescriptorType");
    register::<ProviderOptionChoice>(registry, "ProviderOptionChoice");
    register::<SelectProviderOptionDescriptor>(registry, "SelectProviderOptionDescriptor");
    register::<BooleanProviderOptionDescriptor>(registry, "BooleanProviderOptionDescriptor");
    register::<ProviderOptionDescriptor>(registry, "ProviderOptionDescriptor");
    register::<ProviderOptionSelectionValue>(registry, "ProviderOptionSelectionValue");
    register::<ProviderOptionSelection>(registry, "ProviderOptionSelection");
    register::<ProviderOptionSelections>(registry, "ProviderOptionSelections");
    register::<ModelCapabilities>(registry, "ModelCapabilities");
    register::<CustomModelEntry>(registry, "CustomModelEntry");
    register::<CustomModelSetting>(registry, "CustomModelSetting");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("model", register_module)
}
