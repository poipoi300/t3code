use super::super::{Registry, register};
use t3_contracts::secret_reference::*;

pub fn register_module(registry: &mut Registry) {
    register::<SecretProviderId>(registry, "SecretProviderId");
    register::<SecretProviderSlug>(registry, "SecretProviderSlug");
    register::<SecretReferencePath>(registry, "SecretReferencePath");
    register::<SecretReference>(registry, "SecretReference");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("secret_reference", register_module)
}
