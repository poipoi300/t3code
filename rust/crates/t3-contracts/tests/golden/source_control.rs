use super::super::{Registry, register};
use t3_contracts::source_control::*;
pub fn register_module(registry: &mut Registry) {
    register::<ChangeRequest>(registry, "ChangeRequest");
    register::<ChangeRequestState>(registry, "ChangeRequestState");
    register::<SourceControlAppAccess>(registry, "SourceControlAppAccess");
    register::<SourceControlAppAccessInput>(registry, "SourceControlAppAccessInput");
    register::<SourceControlCloneProtocol>(registry, "SourceControlCloneProtocol");
    register::<SourceControlCloneRepositoryInput>(registry, "SourceControlCloneRepositoryInput");
    register::<SourceControlCloneRepositoryResult>(registry, "SourceControlCloneRepositoryResult");
    register::<SourceControlDiscoveryResult>(registry, "SourceControlDiscoveryResult");
    register::<SourceControlDiscoveryStatus>(registry, "SourceControlDiscoveryStatus");
    register::<SourceControlProviderAuth>(registry, "SourceControlProviderAuth");
    register::<SourceControlProviderAuthStatus>(registry, "SourceControlProviderAuthStatus");
    register::<SourceControlProviderDiscoveryItem>(registry, "SourceControlProviderDiscoveryItem");
    register::<SourceControlProviderError>(registry, "SourceControlProviderError");
    register::<SourceControlProviderInfo>(registry, "SourceControlProviderInfo");
    register::<SourceControlProviderKind>(registry, "SourceControlProviderKind");
    register::<SourceControlPublishRepositoryInput>(
        registry,
        "SourceControlPublishRepositoryInput",
    );
    register::<SourceControlPublishRepositoryResult>(
        registry,
        "SourceControlPublishRepositoryResult",
    );
    register::<SourceControlPublishStatus>(registry, "SourceControlPublishStatus");
    register::<SourceControlRepositoryCloneUrls>(registry, "SourceControlRepositoryCloneUrls");
    register::<SourceControlRepositoryError>(registry, "SourceControlRepositoryError");
    register::<SourceControlRepositoryInfo>(registry, "SourceControlRepositoryInfo");
    register::<SourceControlRepositoryLookupInput>(registry, "SourceControlRepositoryLookupInput");
    register::<SourceControlRepositoryVisibility>(registry, "SourceControlRepositoryVisibility");
    register::<SourceControlSignInError>(registry, "SourceControlSignInError");
    register::<SourceControlSignInFailure>(registry, "SourceControlSignInFailure");
    register::<SourceControlSignInInput>(registry, "SourceControlSignInInput");
    register::<SourceControlSignInPrompt>(registry, "SourceControlSignInPrompt");
    register::<SourceControlSignInResult>(registry, "SourceControlSignInResult");
    register::<SourceControlStarredInput>(registry, "SourceControlStarredInput");
    register::<VcsDiscoveryItem>(registry, "VcsDiscoveryItem");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("source_control", register_module)
}
