use super::super::{Registry, register};
use t3_contracts::thread_tags::*;

pub fn register_module(registry: &mut Registry) {
    register::<ThreadTag>(registry, "ThreadTag");
    register::<StoredThreadTags>(registry, "StoredThreadTags");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("thread_tags", register_module)
}
