use super::super::Registry;

pub fn register_module(_registry: &mut Registry) {}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("orchestration_v2_id_table", register_module)
}
