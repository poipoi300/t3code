use super::super::{Registry, register};
use t3_contracts::model_selection::*;

pub fn register_module(registry: &mut Registry) {
    register::<ModelSelection>(registry, "ModelSelection");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("model_selection", register_module)
}
