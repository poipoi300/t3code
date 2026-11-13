use super::open_code_adapter_v2_testkit::{OpenCodeReplayController, OpenCodeReplayResult};
use crate::testing::replay::{ReplayGate, ReplayTranscript};
use serde_json::{Map, Value, json};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub const OPENCODE2_HTTP_PROTOCOL: &str = "opencode2-http.sse";
pub const REPLAY_BASE_URL: &str = "http://opencode2.replay";

fn extend(target: &mut Map<String, Value>, value: &Value, omit_nulls: bool) {
    if let Some(fields) = value.as_object() {
        target.extend(
            fields
                .iter()
                .filter(|(_, value)| !omit_nulls || !value.is_null())
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
}
fn operation(kind: &str, input: Value) -> Value {
    json!({"type":kind,"input":input})
}
fn decode_component(value: &str) -> Result<String, String> {
    let mut bytes = Vec::new();
    let mut input = value.as_bytes().iter().copied();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let hi = input.next().and_then(|byte| char::from(byte).to_digit(16));
            let lo = input.next().and_then(|byte| char::from(byte).to_digit(16));
            let (Some(hi), Some(lo)) = (hi, lo) else {
                return Err("invalid percent escape".into());
            };
            bytes.push((hi * 16 + lo) as u8);
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).map_err(|error| error.to_string())
}
pub fn operation_of(
    method: &str,
    path: &str,
    query: &Value,
    body: &Value,
) -> Result<Value, String> {
    if method == "GET" && path == "/api/experimental/job" && query["kind"] == "image" {
        return Ok(operation("imageJobs", query.clone()));
    }
    if method == "GET" && path.starts_with("/api/fs/read/") {
        let mut input = json!({"path":path});
        extend(
            input
                .as_object_mut()
                .expect("fixture structure validated above"),
            query,
            false,
        );
        return Ok(operation("image.read", input));
    }
    let parts: Vec<_> = path.split('/').collect();
    if method == "POST"
        && parts.len() == 5
        && parts[1..3] == ["api", "rpc"]
        && !parts[3].is_empty()
        && !parts[4].is_empty()
    {
        let mut input = json!({"rpcID":parts[3],"method":parts[4]});
        extend(
            input
                .as_object_mut()
                .expect("fixture structure validated above"),
            query,
            false,
        );
        extend(
            input
                .as_object_mut()
                .expect("fixture structure validated above"),
            body,
            false,
        );
        return Ok(operation("rpc.call", input));
    }
    if method == "GET" && path == "/api/experimental/job" {
        return Ok(operation("job.list", query.clone()));
    }
    if parts.len() >= 3
        && parts[1..3] == ["api", "shell"]
        && (parts.len() == 3
            || (parts.len() == 4 && !parts[3].is_empty())
            || (parts.len() == 5 && !parts[3].is_empty() && parts[4] == "output"))
    {
        let mut input = json!({});
        if parts.len() > 3 {
            input["id"] = json!(parts[3]);
        }
        extend(
            input
                .as_object_mut()
                .expect("fixture structure validated above"),
            query,
            false,
        );
        let kind = if method == "DELETE" {
            "shell.remove"
        } else if parts.len() == 5 {
            "shell.output"
        } else if parts.len() == 4 {
            "shell.get"
        } else {
            "shell.list"
        };
        return Ok(operation(kind, input));
    }
    let direct = match (method, path) {
        ("GET", "/api/event") => Some(json!({"type":"event.subscribe"})),
        ("GET", "/api/session/active") => Some(json!({"type":"session.active"})),
        ("GET", "/api/model") => Some(operation("model.list", query.clone())),
        ("GET", "/api/agent") => Some(operation("agent.list", query.clone())),
        ("GET", "/api/command") => Some(operation("command.list", query.clone())),
        ("GET", "/api/skill") => Some(operation("skill.list", query.clone())),
        ("POST", "/api/session") => Some(operation("session.create", body.clone())),
        _ => None,
    };
    if let Some(operation) = direct {
        return Ok(operation);
    }
    if parts.len() == 5
        && parts[1..4] == ["api", "experimental", "mcp"]
        && !parts[4].is_empty()
        && matches!(method, "PUT" | "DELETE")
    {
        let mut input = json!({"server":parts[4]});
        extend(
            input
                .as_object_mut()
                .expect("fixture structure validated above"),
            query,
            false,
        );
        if method == "PUT" {
            extend(
                input
                    .as_object_mut()
                    .expect("fixture structure validated above"),
                body,
                false,
            );
        }
        return Ok(operation(
            if method == "PUT" {
                "mcp.add"
            } else {
                "mcp.remove"
            },
            input,
        ));
    }
    if method == "PUT"
        && parts.len() == 8
        && parts[1..4] == ["api", "experimental", "session"]
        && parts[5..7] == ["instructions", "entries"]
        && !parts[4].is_empty()
        && !parts[7].is_empty()
    {
        let mut input = json!({"sessionID":parts[4],"key":parts[7]});
        extend(
            input
                .as_object_mut()
                .expect("fixture structure validated above"),
            body,
            false,
        );
        return Ok(operation("session.instructions.entry.put", input));
    }
    if parts.len() >= 4 && parts[1..3] == ["api", "session"] && !parts[3].is_empty() {
        let mut input = json!({"sessionID":parts[3]});
        extend(
            input
                .as_object_mut()
                .expect("fixture structure validated above"),
            query,
            false,
        );
        extend(
            input
                .as_object_mut()
                .expect("fixture structure validated above"),
            body,
            true,
        );
        let rest = &parts[4..];
        let kind = match (method, rest) {
            ("GET", []) => Some("session.get"),
            ("DELETE", []) => Some("session.remove"),
            ("PATCH", []) => Some("session.update"),
            ("POST", ["prompt"]) => Some("session.prompt"),
            ("POST", ["command"]) => Some("session.command"),
            ("POST", ["compact"]) => Some("session.compact"),
            ("POST", ["interrupt"]) => Some("session.interrupt"),
            ("POST", ["model"]) => Some("session.switchModel"),
            ("POST", ["agent"]) => Some("session.switchAgent"),
            ("POST", ["move"]) => Some("session.move"),
            ("POST", ["synthetic"]) => Some("session.synthetic"),
            ("POST", ["fork"]) => Some("session.fork"),
            ("POST", ["revert", "stage"]) => Some("session.revert.stage"),
            ("POST", ["revert", "commit"]) => Some("session.revert.commit"),
            ("DELETE", ["revert"]) => Some("session.revert.clear"),
            ("GET", ["message"]) => Some("message.list"),
            ("GET", ["permission"]) => Some("permission.list"),
            ("GET", ["form"]) => Some("session.form.list"),
            ("DELETE", ["inbox", id]) if !id.is_empty() => {
                input["inboxID"] = json!(decode_component(id)?);
                Some("session.inbox.cancel")
            }
            ("POST", ["form", id, "reply"]) if !id.is_empty() => {
                input["formID"] = json!(id);
                Some("session.form.reply")
            }
            ("DELETE", ["form", id]) | ("DELETE", ["form", id, "reply"]) if !id.is_empty() => {
                input["formID"] = json!(id);
                Some("session.form.cancel")
            }
            ("POST", ["permission", id, "reply"]) if !id.is_empty() => {
                input["requestID"] = json!(id);
                Some("permission.reply")
            }
            _ => None,
        };
        if let Some(kind) = kind {
            return Ok(operation(kind, input));
        }
    }
    let mut input = Map::new();
    extend(&mut input, query, false);
    input.insert("body".into(), body.clone());
    Ok(operation(&format!("{method} {path}"), Value::Object(input)))
}

pub struct OpenCode2ReplayServer {
    pub controller: Arc<OpenCodeReplayController>,
}
impl OpenCode2ReplayServer {
    pub fn new(transcript: ReplayTranscript, gate: ReplayGate) -> OpenCodeReplayResult<Self> {
        Ok(Self {
            controller: OpenCodeReplayController::for_protocol(
                transcript,
                gate,
                OPENCODE2_HTTP_PROTOCOL,
            )?,
        })
    }
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        query: &Value,
        body: &Value,
        cancellation: &CancellationToken,
    ) -> Result<Value, String> {
        if self.controller.exited() && self.controller.finished() {
            return Err("The replayed OpenCode server has exited.".into());
        }
        let operation = operation_of(method, path, query, body)?;
        if operation["type"] == "event.subscribe" {
            return Err("use subscribe and next_event for the SSE stream".into());
        }
        self.controller
            .until_inbound_delivered(false)
            .await
            .map_err(|error| error.to_string())?;
        self.controller
            .expect_outbound(operation.clone())
            .map_err(|error| error.to_string())?;
        let body = self
            .controller
            .response(operation["type"].as_str().unwrap_or_default())
            .await
            .map_err(|error| error.to_string())?;
        if body == "<hang>" {
            cancellation.cancelled().await;
            return Err("request cancelled".into());
        }
        Ok(body)
    }
}

// Local shim: native v2 client connection/HTTP response conversion belongs to the OpenCode owner.
#[async_trait::async_trait]
pub trait OpenCode2AdapterFactory: Send + Sync {
    async fn create(
        &self,
        instance_id: t3_contracts::provider_instance::ProviderInstanceId,
        server: Arc<OpenCode2ReplayServer>,
        clock: Arc<dyn crate::api::Clock>,
        ids: Arc<dyn crate::api::IdGenerator>,
    ) -> crate::provider::ProviderResult<Arc<dyn crate::provider::ProviderAdapter>>;
}
