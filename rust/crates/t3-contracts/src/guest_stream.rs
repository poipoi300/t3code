#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestThreadModel {
    #[serde(with = "crate::wire::nullable")]
    pub driver: Option<crate::provider_instance::ProviderDriverKind>,
    pub name: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub short_name: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub sub_provider: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestShareView {
    pub share_id: crate::base_schemas::GuestShareId,
    pub title: String,
    pub guest_name: crate::base_schemas::TrimmedNonEmptyString,
    pub expires_at: crate::base_schemas::IsoDateTime,
    pub messages: Vec<crate::guest_share::ThreadGuestMessage>,
    pub model: GuestThreadModel,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum GuestStreamEvent {
    #[serde(rename = "view")]
    View { view: Box<GuestShareView> },
    #[serde(rename = "thread")]
    Thread {
        item: Box<crate::orchestration_v2::OrchestrationV2ThreadStreamItem>,
    },
    #[serde(rename = "ended")]
    Ended,
}
