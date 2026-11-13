use super::super::{Registry, register};
use t3_contracts::desktop_bootstrap::*;

pub fn register_module(registry: &mut Registry) {
    register::<DesktopBackendBootstrap>(registry, "DesktopBackendBootstrap");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("desktop_bootstrap", register_module)
}
