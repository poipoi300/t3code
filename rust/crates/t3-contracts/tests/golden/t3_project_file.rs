use super::super::{Registry, register};
use t3_contracts::t3_project_file::*;
pub fn register_module(registry: &mut Registry) {
    register::<T3ProjectFile>(registry, "T3ProjectFile");
    register::<T3ProjectFileScript>(registry, "T3ProjectFileScript");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("t3_project_file", register_module)
}
