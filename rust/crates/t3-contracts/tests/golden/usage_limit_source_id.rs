use super::super::{Registry, register};
use t3_contracts::usage_limit_source_id::*;

pub fn register_module(registry: &mut Registry) {
    register::<UsageLimitSourceId>(registry, "UsageLimitSourceId");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("usage_limit_source_id", register_module)
}
