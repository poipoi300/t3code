use super::super::{Registry, register};
use t3_contracts::preview_capture::*;

pub fn register_module(registry: &mut Registry) {
    register::<PreviewCaptureFailure>(registry, "PreviewCaptureFailure");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("preview_capture", register_module)
}
