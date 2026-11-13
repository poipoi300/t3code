use super::super::{Registry, register};
use t3_contracts::orchestration_dispatch::*;

pub fn register_module(registry: &mut Registry) {
    register::<OrchestrationDispatchCommandError>(registry, "OrchestrationDispatchCommandError");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("orchestration_dispatch", register_module)
}
