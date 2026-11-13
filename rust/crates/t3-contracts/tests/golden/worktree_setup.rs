use super::super::{Registry, register};
use t3_contracts::worktree_setup::*;

pub fn register_module(registry: &mut Registry) {
    register::<WorktreeSetupCancelInput>(registry, "WorktreeSetupCancelInput");
    register::<WorktreeSetupCancelResult>(registry, "WorktreeSetupCancelResult");
    register::<WorktreeSetupPhase>(registry, "WorktreeSetupPhase");
    register::<WorktreeSetupSnapshot>(registry, "WorktreeSetupSnapshot");
    register::<WorktreeSetupStage>(registry, "WorktreeSetupStage");
    register::<WorktreeSetupStageId>(registry, "WorktreeSetupStageId");
    register::<WorktreeSetupStageStatus>(registry, "WorktreeSetupStageStatus");
    register::<WorktreeSetupStreamEvent>(registry, "WorktreeSetupStreamEvent");
    register::<WorktreeSetupSubscribeInput>(registry, "WorktreeSetupSubscribeInput");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("worktree_setup", register_module)
}
