#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct ComposerContextClipboardFragmentVersion;
impl TryFrom<u64> for ComposerContextClipboardFragmentVersion {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value == 1 {
            Ok(Self)
        } else {
            Err("unexpected literal")
        }
    }
}
impl From<ComposerContextClipboardFragmentVersion> for u64 {
    fn from(_: ComposerContextClipboardFragmentVersion) -> Self {
        1
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerContextClipboardFragmentSource {
    pub environment_id: crate::base_schemas::EnvironmentId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub thread_id: Option<Option<crate::base_schemas::ThreadId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub message_id: Option<Option<crate::base_schemas::MessageId>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerContextClipboardFragment {
    pub version: ComposerContextClipboardFragmentVersion,
    pub source: ComposerContextClipboardFragmentSource,
    #[serde(deserialize_with = "crate::composer_context::deserialize_context_records")]
    pub records: Vec<crate::composer_context::ComposerContextRecord>,
}

pub const COMPOSER_CONTEXT_CLIPBOARD_MIME: &str = "web application/x-t3-context-fragment+json";
