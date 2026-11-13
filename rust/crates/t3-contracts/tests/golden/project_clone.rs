use super::super::{Registry, register};
use t3_contracts::project_clone::*;
pub fn register_module(registry: &mut Registry) {
    register::<ProjectCloneActionInput>(registry, "ProjectCloneActionInput");
    register::<ProjectCloneActionResult>(registry, "ProjectCloneActionResult");
    register::<ProjectCloneListEvent>(registry, "ProjectCloneListEvent");
    register::<ProjectClonePhase>(registry, "ProjectClonePhase");
    register::<ProjectCloneSnapshot>(registry, "ProjectCloneSnapshot");
    register::<ProjectCloneStage>(registry, "ProjectCloneStage");
    register::<ProjectCloneStartInput>(registry, "ProjectCloneStartInput");
    register::<ProjectCloneStartResult>(registry, "ProjectCloneStartResult");
    register::<ProjectCloneSubscribeInput>(registry, "ProjectCloneSubscribeInput");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("project_clone", register_module)
}
