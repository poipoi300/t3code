use t3_contracts::{
    browser_profile::BrowserProfileId,
    preview::{
        PreviewAppearancePreference, PreviewOpenInput, PreviewViewportSetting, PreviewZoomFactor,
    },
};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserDefaults {
    pub viewport: PreviewViewportSetting,
    pub profile_id: BrowserProfileId,
    pub zoom_factor: PreviewZoomFactor,
    pub appearance: PreviewAppearancePreference,
}

#[derive(Clone, Copy)]
pub enum EntryPoint {
    Session,
    File,
    TerminalLink,
}

pub fn apply_defaults(
    mut input: PreviewOpenInput,
    defaults: &BrowserDefaults,
    entry: EntryPoint,
) -> PreviewOpenInput {
    use t3_contracts::preview::PreviewRuntime;
    if input.viewport.as_ref().and_then(Option::as_ref).is_none() {
        input.viewport = Some(Some(defaults.viewport.clone()));
    }
    if input.profile_id.as_ref().and_then(Option::as_ref).is_none() {
        input.profile_id = Some(Some(defaults.profile_id.clone()));
    }
    if matches!(entry, EntryPoint::Session) && input.runtime == Some(Some(PreviewRuntime::Desktop))
    {
        input.runtime = None;
    }
    if input.runtime.as_ref().and_then(Option::as_ref).is_some() {
        input.zoom_factor = Some(Some(defaults.zoom_factor.clone()));
        input.color_scheme = Some(Some(defaults.appearance));
    }
    input
}
