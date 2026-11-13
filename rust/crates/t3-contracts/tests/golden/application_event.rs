use super::super::{Registry, register};
use t3_contracts::application_event::*;

pub fn register_module(registry: &mut Registry) {
    register::<ApplicationEventMetadata>(registry, "ApplicationEventMetadata");
    register::<ApplicationProjectCreatedEvent>(registry, "ApplicationProjectCreatedEvent");
    register::<ApplicationProjectCreatedPayload>(registry, "ApplicationProjectCreatedPayload");
    register::<ApplicationProjectDeletedEvent>(registry, "ApplicationProjectDeletedEvent");
    register::<ApplicationProjectDeletedPayload>(registry, "ApplicationProjectDeletedPayload");
    register::<ApplicationProjectEvent>(registry, "ApplicationProjectEvent");
    register::<ApplicationProjectEvent>(registry, "ApplicationProjectEvent.project.created");
    register::<ApplicationProjectEvent>(registry, "ApplicationProjectEvent.project.deleted");
    register::<ApplicationProjectEvent>(registry, "ApplicationProjectEvent.project.meta-updated");
    register::<ApplicationProjectMetaUpdatedEvent>(registry, "ApplicationProjectMetaUpdatedEvent");
    register::<ApplicationProjectMetaUpdatedPayload>(
        registry,
        "ApplicationProjectMetaUpdatedPayload",
    );
    register::<OrchestrationClientOrigin>(registry, "OrchestrationClientOrigin");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("application_event", register_module)
}
