use super::super::{Registry, register};
use t3_contracts::orchestration_project::*;
pub fn register_module(registry: &mut Registry) {
    register::<OrchestrationProjectShell>(registry, "OrchestrationProjectShell");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("orchestration_project", register_module)
}
