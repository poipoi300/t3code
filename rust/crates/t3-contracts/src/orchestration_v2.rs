//! Encoded orchestration contracts; compact envelopes retain ID table indices.
use serde::{Deserialize, Serialize};
#[path = "orchestration_v2/api.rs"]
mod api;
#[path = "orchestration_v2/commands.rs"]
mod commands;
#[path = "orchestration_v2/core.rs"]
mod core;
#[path = "orchestration_v2/events.rs"]
mod events;
#[path = "orchestration_v2/projections.rs"]
mod projections;
#[path = "orchestration_v2/turn_items.rs"]
mod turn_items;
pub use self::{api::*, commands::*, core::*, events::*, projections::*, turn_items::*};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CompactId {
    String(String),
    Number(crate::wire::JsNumber),
}

fn provider_capabilities_runtime_policy_default() -> OrchestrationV2RuntimePolicyCapabilities {
    OrchestrationV2RuntimePolicyCapabilities {
        enforcement: RuntimePolicyCapabilitiesEnforcement::ClientBoundary,
    }
}
fn app_thread_settled_override_default() -> Option<AppThreadSettledOverride> {
    None
}
fn app_thread_settled_at_default() -> Option<crate::wire::WireDateTime> {
    None
}
fn app_thread_last_visited_at_default() -> Option<crate::wire::WireDateTime> {
    None
}

#[derive(Deserialize)]
struct ProjectedRowHeader {
    #[serde(rename = "position")]
    _position: u64,
    #[serde(rename = "visibility")]
    _visibility: ProjectedTurnItemVisibility,
    #[serde(rename = "sourceThreadId")]
    _source_thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "sourceItemId")]
    _source_item_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    item: crate::wire::ForwardCompatible<OrchestrationV2TurnItem>,
}

fn deserialize_projected_items<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let rows = Vec::<serde_json::Value>::deserialize(deserializer)?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let header: ProjectedRowHeader =
            serde_json::from_value(row.clone()).map_err(serde::de::Error::custom)?;
        if let crate::wire::ForwardCompatible::Known(_) = header.item {
            items.push(serde_json::from_value(row).map_err(serde::de::Error::custom)?);
        }
    }
    Ok(items)
}

mod optional_lenient_opens {
    use super::*;
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Option<OrchestrationV2ThreadOpens>>, D::Error> {
        let raw = serde_json::Value::deserialize(deserializer)?;
        Ok(Some(serde_json::from_value(raw).ok().flatten()))
    }
    pub use crate::wire::optional::serialize;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OrchestrationV2ServerCommand {
    Client(Box<OrchestrationV2Command>),
    Internal(Box<OrchestrationV2InternalCommand>),
}
