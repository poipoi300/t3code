use super::super::{Registry, register};
use t3_contracts::worktree_mcp::*;

pub fn register_module(registry: &mut Registry) {
    register::<WorktreeMcpContinuationStatus>(registry, "WorktreeMcpContinuationStatus");
    register::<WorktreeMcpFailure>(registry, "WorktreeMcpFailure");
    register::<WorktreeMcpHandoffInput>(registry, "WorktreeMcpHandoffInput");
    register::<WorktreeMcpHandoffResult>(registry, "WorktreeMcpHandoffResult");
    register::<WorktreeMcpSetupScriptStatus>(registry, "WorktreeMcpSetupScriptStatus");
    register::<WorktreeMcpStatusResult>(registry, "WorktreeMcpStatusResult");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("worktree_mcp", register_module)
}
