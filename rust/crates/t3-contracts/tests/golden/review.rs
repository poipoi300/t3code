use super::super::{Registry, register};
use t3_contracts::review::*;
pub fn register_module(registry: &mut Registry) {
    register::<ReviewBranchCommitsInput>(registry, "ReviewBranchCommitsInput");
    register::<ReviewBranchCommitsResult>(registry, "ReviewBranchCommitsResult");
    register::<ReviewDiffFileContentsInput>(registry, "ReviewDiffFileContentsInput");
    register::<ReviewDiffFileContentsResult>(registry, "ReviewDiffFileContentsResult");
    register::<ReviewDiffFileStat>(registry, "ReviewDiffFileStat");
    register::<ReviewDiffPreviewError>(registry, "ReviewDiffPreviewError");
    register::<ReviewDiffPreviewInput>(registry, "ReviewDiffPreviewInput");
    register::<ReviewDiffPreviewResult>(registry, "ReviewDiffPreviewResult");
    register::<ReviewDiffPreviewSource>(registry, "ReviewDiffPreviewSource");
    register::<ReviewDiffPreviewSourceKind>(registry, "ReviewDiffPreviewSourceKind");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("review", register_module)
}
