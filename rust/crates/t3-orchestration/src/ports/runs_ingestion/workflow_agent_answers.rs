use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAgentMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(t3_contracts::wire::trim_js)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

pub fn merge_workflow_agent_metadata(
    lines: &[Value],
    mut metadata: WorkflowAgentMetadata,
) -> WorkflowAgentMetadata {
    for entry in lines {
        if entry.get("type").is_none_or(|kind| kind != "assistant")
            || entry.get("isMeta") == Some(&Value::Bool(true))
            || entry.get("isApiErrorMessage") == Some(&Value::Bool(true))
        {
            continue;
        }
        let message = entry.get("message");
        let message_id = text(message.and_then(|message| message.get("id")));
        if message_id.is_none() || message_id != metadata.message_id {
            metadata = WorkflowAgentMetadata::default();
        }
        if let Some(effort) = text(entry.get("effort")) {
            metadata.effort = Some(effort);
        }
        if let Some(speed) = text(
            message
                .and_then(|message| message.get("usage"))
                .and_then(|usage| usage.get("speed")),
        ) {
            metadata.speed = Some(speed);
        }
        metadata.message_id = message_id;
    }
    metadata
}
