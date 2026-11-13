use super::super::{Registry, register};
use t3_contracts::thread_title::*;

pub fn register_module(registry: &mut Registry) {
    register::<ThreadTitleRegenerationStatus>(registry, "ThreadTitleRegenerationStatus");
    register::<ThreadTitleRegeneration>(registry, "ThreadTitleRegeneration");
    register::<ThreadTitleRegenerationOutcome>(registry, "ThreadTitleRegenerationOutcome");
    register::<LastThreadTitleRegeneration>(registry, "LastThreadTitleRegeneration");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("thread_title", register_module)
}
