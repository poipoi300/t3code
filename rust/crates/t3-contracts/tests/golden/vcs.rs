use super::super::{Registry, register};
use t3_contracts::vcs::*;
pub fn register_module(registry: &mut Registry) {
    register::<VcsDriverCapabilities>(registry, "VcsDriverCapabilities");
    register::<VcsDriverKind>(registry, "VcsDriverKind");
    register::<VcsError>(registry, "VcsError");
    register::<VcsFreshness>(registry, "VcsFreshness");
    register::<VcsFreshnessSource>(registry, "VcsFreshnessSource");
    register::<VcsListRemotesResult>(registry, "VcsListRemotesResult");
    register::<VcsListWorkspaceFilesResult>(registry, "VcsListWorkspaceFilesResult");
    register::<VcsProcessExitError>(registry, "VcsProcessExitError");
    register::<VcsProcessExitFailureKind>(registry, "VcsProcessExitFailureKind");
    register::<VcsProcessMissingExitCodeError>(registry, "VcsProcessMissingExitCodeError");
    register::<VcsProcessOutputLimitError>(registry, "VcsProcessOutputLimitError");
    register::<VcsProcessOutputReadError>(registry, "VcsProcessOutputReadError");
    register::<VcsProcessSpawnError>(registry, "VcsProcessSpawnError");
    register::<VcsProcessStdinWriteError>(registry, "VcsProcessStdinWriteError");
    register::<VcsProcessTimeoutError>(registry, "VcsProcessTimeoutError");
    register::<VcsRemote>(registry, "VcsRemote");
    register::<VcsRepositoryDetectionError>(registry, "VcsRepositoryDetectionError");
    register::<VcsRepositoryIdentity>(registry, "VcsRepositoryIdentity");
    register::<VcsUnsupportedOperationError>(registry, "VcsUnsupportedOperationError");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("vcs", register_module)
}
