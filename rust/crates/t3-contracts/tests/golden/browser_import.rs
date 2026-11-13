use super::super::{Registry, register};
use t3_contracts::browser_import::*;

pub fn register_module(registry: &mut Registry) {
    register::<BrowserImportFailureReason>(registry, "BrowserImportFailureReason");
    register::<BrowserImportInput>(registry, "BrowserImportInput");
    register::<BrowserImportResult>(registry, "BrowserImportResult");
    register::<BrowserImportSource>(registry, "BrowserImportSource");
    register::<BrowserImportSourceId>(registry, "BrowserImportSourceId");
    register::<BrowserImportSourceProfile>(registry, "BrowserImportSourceProfile");
    register::<BrowserImportUnavailableReason>(registry, "BrowserImportUnavailableReason");
    register::<DesktopPreviewImportCookiesInputSchema>(
        registry,
        "DesktopPreviewImportCookiesInputSchema",
    );
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("browser_import", register_module)
}
