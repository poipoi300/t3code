use super::super::{Registry, register};
use t3_contracts::preview::*;

pub fn register_module(registry: &mut Registry) {
    register::<BrowserRecordingFrameRate>(registry, "BrowserRecordingFrameRate");
    register::<ConfiguredLocalServerUrls>(registry, "ConfiguredLocalServerUrls");
    register::<DiscoveredLocalServer>(registry, "DiscoveredLocalServer");
    register::<DiscoveredLocalServerList>(registry, "DiscoveredLocalServerList");
    register::<PreviewAdjustInput>(registry, "PreviewAdjustInput");
    register::<PreviewAppearancePreference>(registry, "PreviewAppearancePreference");
    register::<PreviewClearProfileError>(registry, "PreviewClearProfileError");
    register::<PreviewClearProfileInput>(registry, "PreviewClearProfileInput");
    register::<PreviewCloseInput>(registry, "PreviewCloseInput");
    register::<PreviewControlRequiredError>(registry, "PreviewControlRequiredError");
    register::<PreviewError>(registry, "PreviewError");
    register::<PreviewEvent>(registry, "PreviewEvent");
    register::<PreviewInvalidUrlError>(registry, "PreviewInvalidUrlError");
    register::<PreviewListInput>(registry, "PreviewListInput");
    register::<PreviewListResult>(registry, "PreviewListResult");
    register::<PreviewNavStatus>(registry, "PreviewNavStatus");
    register::<PreviewOpenInput>(registry, "PreviewOpenInput");
    register::<PreviewRenderedViewportSize>(registry, "PreviewRenderedViewportSize");
    register::<PreviewReportProfilesInput>(registry, "PreviewReportProfilesInput");
    register::<PreviewReportStatusInput>(registry, "PreviewReportStatusInput");
    register::<PreviewResizeInput>(registry, "PreviewResizeInput");
    register::<PreviewRuntime>(registry, "PreviewRuntime");
    register::<PreviewSessionLookupError>(registry, "PreviewSessionLookupError");
    register::<PreviewSessionSnapshot>(registry, "PreviewSessionSnapshot");
    register::<PreviewStopLocalServerError>(registry, "PreviewStopLocalServerError");
    register::<PreviewStopLocalServerInput>(registry, "PreviewStopLocalServerInput");
    register::<PreviewStreamHostSetup>(registry, "PreviewStreamHostSetup");
    register::<PreviewTabId>(registry, "PreviewTabId");
    register::<PreviewViewportPresetId>(registry, "PreviewViewportPresetId");
    register::<PreviewViewportSetting>(registry, "PreviewViewportSetting");
    register::<PreviewViewportSize>(registry, "PreviewViewportSize");
    register::<PreviewZoomFactor>(registry, "PreviewZoomFactor");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("preview", register_module)
}

#[test]
fn nullable_discovery_fields_are_required_and_optional_nulls_survive()
-> Result<(), serde_json::Error> {
    use serde_json::json;
    let value = json!({ "host": "localhost", "port": 5173, "url": "http://localhost:5173", "processName": null, "pid": null, "terminal": null, "agent": null });
    let decoded: DiscoveredLocalServer = serde_json::from_value(value.clone())?;
    assert_eq!(serde_json::to_value(decoded)?, value);
    let mut missing = value;
    missing.as_object_mut().expect("object").remove("terminal");
    assert!(serde_json::from_value::<DiscoveredLocalServer>(missing).is_err());
    // JSON.stringify(JSON.parse(raw)) in JavaScript.
    for (raw, javascript) in [("1", "1"), ("1.0", "1"), ("0.33", "0.33")] {
        let decoded: PreviewZoomFactor = serde_json::from_str(raw)?;
        assert_eq!(serde_json::to_string(&decoded)?, javascript);
    }
    Ok(())
}
