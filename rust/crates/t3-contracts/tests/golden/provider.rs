use super::super::{Registry, register};
use t3_contracts::provider::*;

pub fn register_module(registry: &mut Registry) {
    register::<ProviderSendTurnInput>(registry, "ProviderSendTurnInput");
    register::<ProviderUploadFeedbackInput>(registry, "ProviderUploadFeedbackInput");
    register::<ProviderUploadFeedbackResult>(registry, "ProviderUploadFeedbackResult");
    register::<ProviderUploadFeedbackError>(registry, "ProviderUploadFeedbackError");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("provider", register_module)
}
