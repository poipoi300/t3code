use super::super::{Registry, register};
use t3_contracts::assistant_citations::*;

pub fn register_module(registry: &mut Registry) {
    register::<AssistantCitation>(registry, "AssistantCitation");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("assistant_citations", register_module)
}
