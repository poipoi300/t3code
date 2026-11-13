use super::pi_adapter_v2_testkit::{JsonlFlavor, JsonlReplayController, JsonlReplayResult};
use crate::testing::replay::{ReplayEntry, ReplayTranscript};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

pub const MUSE_PROVIDER_KIND: &str = "muse";
pub const MUSE_MSP_REPLAY_PROTOCOL: &str = "muse.msp-jsonl";
pub fn make_replay_controller(
    transcript: ReplayTranscript,
) -> JsonlReplayResult<Arc<JsonlReplayController>> {
    JsonlReplayController::new(transcript, JsonlFlavor::Muse)
}
pub fn muse_record_label(
    frame: &Value,
    inbound: bool,
    request_methods: &BTreeMap<String, String>,
    host: usize,
) -> Option<String> {
    let base = if frame["type"] == "host_start" {
        Some("host_start".into())
    } else if let Some(method) = frame["method"].as_str() {
        Some(if inbound && frame.get("id").is_some() {
            format!("request:{method}")
        } else {
            method.into()
        })
    } else {
        request_methods
            .get(&frame["id"].to_string())
            .map(|method| format!("{}:{method}", if inbound { "response" } else { "reply" }))
    }?;
    Some(if host == 1 {
        base
    } else {
        format!("{base}@h{host}")
    })
}
fn workspace_value(value: &Value, workspace: &str) -> Value {
    if value == "<workspace>" {
        return json!(workspace);
    }
    match value {
        Value::Array(values) => values
            .iter()
            .map(|value| workspace_value(value, workspace))
            .collect(),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| (key.clone(), workspace_value(value, workspace)))
            .collect(),
        _ => value.clone(),
    }
}
pub fn materialize_muse_replay_workspace(
    mut transcript: ReplayTranscript,
    canonical_workspace: &str,
    native_turn_root: &str,
) -> ReplayTranscript {
    for entry in &mut transcript.entries {
        if let ReplayEntry::ExpectOutbound(entry) = entry {
            let workspace = if entry.frame["method"] == "turn/start" {
                native_turn_root
            } else {
                canonical_workspace
            };
            entry.frame = workspace_value(&entry.frame, workspace);
        }
    }
    transcript
}

// Local shim: native Muse SDK initialize/connection and adapter mapping remain provider-owned.
#[async_trait::async_trait]
pub trait MuseAdapterFactory: Send + Sync {
    async fn create(
        &self,
        controller: Arc<JsonlReplayController>,
        environment: BTreeMap<String, String>,
        clock: Arc<dyn crate::api::Clock>,
        ids: Arc<dyn crate::api::IdGenerator>,
    ) -> crate::provider::ProviderResult<Arc<dyn crate::provider::ProviderAdapter>>;
}
