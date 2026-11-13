use super::super::Registry;

pub fn register_module(_registry: &mut Registry) {}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("index", register_module)
}
