use super::super::{Registry, register};
use t3_contracts::settings_secret_value::*;

pub fn register_module(registry: &mut Registry) {
    register::<SettingsSecretValue>(registry, "SettingsSecretValue");
    register::<SecretSourceCheckResult>(registry, "SecretSourceCheckResult");
    register::<SettingsSecretUnresolvedError>(registry, "SettingsSecretUnresolvedError");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("settings_secret_value", register_module)
}
