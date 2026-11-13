use super::super::{Registry, register};
use t3_contracts::browser_profile::*;

pub fn register_module(registry: &mut Registry) {
    register::<BrowserProfileId>(registry, "BrowserProfileId");
    register::<BrowserProfileName>(registry, "BrowserProfileName");
    register::<BrowserProfileKind>(registry, "BrowserProfileKind");
    register::<BrowserProfile>(registry, "BrowserProfile");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("browser_profile", register_module)
}
