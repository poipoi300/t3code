use super::super::{Registry, register};
use t3_contracts::desktop_browser::*;

pub fn register_module(registry: &mut Registry) {
    register::<DesktopBrowserCommand>(registry, "DesktopBrowserCommand");
    register::<DesktopBrowserEvent>(registry, "DesktopBrowserEvent");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("desktop_browser", register_module)
}
