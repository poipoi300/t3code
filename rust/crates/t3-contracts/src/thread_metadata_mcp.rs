#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ThreadMetadataMcpAction {
    #[serde(rename = "rename")]
    Rename,
    #[serde(rename = "regenerate_title")]
    RegenerateTitle,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", try_from = "RawThreadMetadataMcpUpdateInput")]
pub struct ThreadMetadataMcpUpdateInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub thread_id: Option<Option<crate::base_schemas::ThreadId>>,
    pub action: ThreadMetadataMcpAction,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub client_request_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawThreadMetadataMcpUpdateInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    thread_id: Option<Option<crate::base_schemas::ThreadId>>,
    action: ThreadMetadataMcpAction,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    title: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    client_request_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}

impl TryFrom<RawThreadMetadataMcpUpdateInput> for ThreadMetadataMcpUpdateInput {
    type Error = String;

    fn try_from(input: RawThreadMetadataMcpUpdateInput) -> Result<Self, Self::Error> {
        // Golden samples use a null title for regenerate_title.
        if (input.action == ThreadMetadataMcpAction::Rename)
            != input.title.as_ref().and_then(Option::as_ref).is_some()
        {
            return Err("rename requires title; regenerate_title does not accept one".into());
        }
        if input
            .title
            .as_ref()
            .and_then(Option::as_ref)
            .is_some_and(|s| s.0.encode_utf16().count() > 512)
        {
            return Err("title exceeds 512 UTF-16 code units".into());
        }
        if input
            .client_request_id
            .as_ref()
            .and_then(Option::as_ref)
            .is_some_and(|s| s.0.encode_utf16().count() > 256)
        {
            return Err("clientRequestId exceeds 256 UTF-16 code units".into());
        }
        Ok(Self {
            thread_id: input.thread_id,
            action: input.action,
            title: input.title,
            client_request_id: input.client_request_id,
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadMetadataMcpUpdateResult {
    pub thread_id: crate::base_schemas::ThreadId,
    pub action: ThreadMetadataMcpAction,
    pub command_id: crate::base_schemas::CommandId,
    pub sequence: u64,
    pub title: String,
    #[serde(with = "crate::wire::nullable")]
    pub title_regeneration: Option<crate::thread_title::ThreadTitleRegeneration>,
    #[serde(with = "crate::wire::nullable")]
    pub linked_pull_request: Option<crate::thread_pull_request::ThreadLinkedPullRequest>,
    pub updated_at: crate::base_schemas::IsoDateTime,
}
