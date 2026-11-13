use super::super::{Registry, register};
use t3_contracts::desktop_app_activation::*;

pub fn register_module(registry: &mut Registry) {
    register::<DesktopAppActivationErrorCode>(registry, "DesktopAppActivationErrorCode");
    register::<DesktopAppActivationFailure>(registry, "DesktopAppActivationFailure");
    register::<DesktopAppActivationPlatform>(registry, "DesktopAppActivationPlatform");
    register::<DesktopAppActivationRequest>(registry, "DesktopAppActivationRequest");
    register::<DesktopAppActivationResponse>(registry, "DesktopAppActivationResponse");
    register::<DesktopAppActivationSuccess>(registry, "DesktopAppActivationSuccess");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("desktop_app_activation", register_module)
}
