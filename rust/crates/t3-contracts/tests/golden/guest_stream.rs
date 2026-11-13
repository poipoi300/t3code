use super::super::{Registry, register};
use t3_contracts::guest_stream::*;

pub fn register_module(registry: &mut Registry) {
    register::<GuestShareView>(registry, "GuestShareView");
    register::<GuestStreamEvent>(registry, "GuestStreamEvent");
    register::<GuestThreadModel>(registry, "GuestThreadModel");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("guest_stream", register_module)
}
