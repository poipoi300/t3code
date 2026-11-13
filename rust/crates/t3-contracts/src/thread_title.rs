use crate::base_schemas::{CommandId, IsoDateTime};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreadTitleRegenerationStatus {
    #[serde(rename = "renamed")]
    Renamed,
    #[serde(rename = "unchanged")]
    Unchanged,
    #[serde(rename = "failed")]
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTitleRegeneration {
    pub request_id: CommandId,
    pub started_at: IsoDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTitleRegenerationOutcome {
    pub status: ThreadTitleRegenerationStatus,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub message: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastThreadTitleRegeneration {
    pub status: ThreadTitleRegenerationStatus,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub message: Option<Option<String>>,
    pub request_id: CommandId,
    pub completed_at: IsoDateTime,
}
