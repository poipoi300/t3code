use super::super::{Registry, register};
use t3_contracts::provider_policy::*;

pub fn register_module(registry: &mut Registry) {
    register::<ProviderApprovalPolicy>(registry, "ProviderApprovalPolicy");
    register::<ProviderSandboxMode>(registry, "ProviderSandboxMode");
    register::<RuntimeMode>(registry, "RuntimeMode");
    register::<ProviderInteractionMode>(registry, "ProviderInteractionMode");
    register::<ProviderRequestKind>(registry, "ProviderRequestKind");
    register::<ProviderApprovalDecision>(registry, "ProviderApprovalDecision");
    register::<ProviderApprovalFullAccessOverride>(registry, "ProviderApprovalFullAccessOverride");
    register::<ProviderApprovalOption>(registry, "ProviderApprovalOption");
    register::<ProviderUserInputAnswers>(registry, "ProviderUserInputAnswers");
    register::<UserInputAttachments>(registry, "UserInputAttachments");
    register::<UserInputAttachmentAnswerPayload>(registry, "UserInputAttachmentAnswerPayload");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("provider_policy", register_module)
}
