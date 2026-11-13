use super::super::{Registry, register};
use t3_contracts::provider_setup::*;
pub fn register_module(registry: &mut Registry) {
    register::<ChatGptHandoffInput>(registry, "ChatGptHandoffInput");
    register::<ChatGptHandoffState>(registry, "ChatGptHandoffState");
    register::<ChatGptImportProfileInput>(registry, "ChatGptImportProfileInput");
    register::<ChatGptReconnectProfile>(registry, "ChatGptReconnectProfile");
    register::<ChatGptReconnectProfileInput>(registry, "ChatGptReconnectProfileInput");
    register::<ChatGptTransferredProfile>(registry, "ChatGptTransferredProfile");
    register::<ProviderAuthCancelInput>(registry, "ProviderAuthCancelInput");
    register::<ProviderAuthCompleteInput>(registry, "ProviderAuthCompleteInput");
    register::<ProviderAuthInteraction>(registry, "ProviderAuthInteraction");
    register::<ProviderAuthMethod>(registry, "ProviderAuthMethod");
    register::<ProviderAuthRemoveMethodInput>(registry, "ProviderAuthRemoveMethodInput");
    register::<ProviderAuthRespondInput>(registry, "ProviderAuthRespondInput");
    register::<ProviderAuthResponse>(registry, "ProviderAuthResponse");
    register::<ProviderAuthStartInput>(registry, "ProviderAuthStartInput");
    register::<ProviderAuthState>(registry, "ProviderAuthState");
    register::<ProviderInstallCancelInput>(registry, "ProviderInstallCancelInput");
    register::<ProviderInstallState>(registry, "ProviderInstallState");
    register::<ProviderSetupError>(registry, "ProviderSetupError");
    register::<ProviderSetupInput>(registry, "ProviderSetupInput");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("provider_setup", register_module)
}

#[test]
fn future_setup_metadata_is_omitted_and_explicit_null_survives() -> Result<(), serde_json::Error> {
    use serde_json::json;
    let base = json!({"instanceId":"codex","phase":"idle","flowId":null,"authorizationUrl":null,"expiresAt":null,"message":null});
    let mut input = base.clone();
    input["methods"] =
        json!([{"id":"future","name":"Future","description":null,"type":"future"},null]);
    input["interaction"] = json!({"type":"future"});
    input["credentialOwner"] = json!("future");
    let decoded: ProviderAuthState = serde_json::from_value(input)?;
    let mut expected = base.clone();
    expected["methods"] = json!([]);
    assert_eq!(serde_json::to_value(decoded)?, expected);
    let mut input = base;
    input["interaction"] = json!(null);
    let decoded: ProviderAuthState = serde_json::from_value(input.clone())?;
    assert_eq!(serde_json::to_value(decoded)?, input);
    Ok(())
}
