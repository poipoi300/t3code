use super::super::{Registry, register};
use t3_contracts::checkpoint_diff::*;

pub fn register_module(registry: &mut Registry) {
    register::<TurnCountRange>(registry, "TurnCountRange");
    register::<ThreadTurnDiff>(registry, "ThreadTurnDiff");
    register::<OrchestrationGetTurnDiffInput>(registry, "OrchestrationGetTurnDiffInput");
    register::<OrchestrationGetFullThreadDiffInput>(
        registry,
        "OrchestrationGetFullThreadDiffInput",
    );
    register::<OrchestrationGetTurnDiffResult>(registry, "OrchestrationGetTurnDiffResult");
    register::<OrchestrationGetFullThreadDiffResult>(
        registry,
        "OrchestrationGetFullThreadDiffResult",
    );
    register::<OrchestrationGetTurnDiffError>(registry, "OrchestrationGetTurnDiffError");
    register::<OrchestrationGetFullThreadDiffError>(
        registry,
        "OrchestrationGetFullThreadDiffError",
    );
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("checkpoint_diff", register_module)
}
