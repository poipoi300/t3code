use super::super::{Registry, register};
use t3_contracts::thread_search::*;

pub fn register_module(registry: &mut Registry) {
    register::<OrchestrationThreadSearchSource>(registry, "OrchestrationThreadSearchSource");
    register::<OrchestrationSearchThreadsInput>(registry, "OrchestrationSearchThreadsInput");
    register::<OrchestrationThreadSearchMatch>(registry, "OrchestrationThreadSearchMatch");
    register::<OrchestrationSearchThreadsResult>(registry, "OrchestrationSearchThreadsResult");
    register::<OrchestrationSearchThreadsError>(registry, "OrchestrationSearchThreadsError");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("thread_search", register_module)
}
