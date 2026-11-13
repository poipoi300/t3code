use super::super::{Registry, register};
use t3_contracts::agent_sessions::*;
pub fn register_module(registry: &mut Registry) {
    register::<AgentSessionImportInput>(registry, "AgentSessionImportInput");
    register::<AgentSessionImportProjectChangedError>(
        registry,
        "AgentSessionImportProjectChangedError",
    );
    register::<AgentSessionImportProjectNotFoundError>(
        registry,
        "AgentSessionImportProjectNotFoundError",
    );
    register::<AgentSessionImportResult>(registry, "AgentSessionImportResult");
    register::<AgentSessionImportSource>(registry, "AgentSessionImportSource");
    register::<AgentSessionProjectCandidate>(registry, "AgentSessionProjectCandidate");
    register::<AgentSessionProjectGit>(registry, "AgentSessionProjectGit");
    register::<AgentSessionScanError>(registry, "AgentSessionScanError");
    register::<AgentSessionScanInput>(registry, "AgentSessionScanInput");
    register::<AgentSessionScanResult>(registry, "AgentSessionScanResult");
    register::<AgentSessionSource>(registry, "AgentSessionSource");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("agent_sessions", register_module)
}
