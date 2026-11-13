use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("provider replay NDJSON did not contain any records")]
    Empty,
    #[error("provider replay NDJSON requires a transcript_start header")]
    MissingHeader,
    #[error("invalid replay record on line {line}: {detail}")]
    Record { line: usize, detail: String },
    #[error("replay mismatch at entry {cursor}: {detail}")]
    Mismatch {
        cursor: usize,
        detail: String,
        expected: Box<Value>,
        actual: Box<Value>,
    },
    #[error("duplicate provider replay gate label {0}")]
    DuplicateGate(String),
    #[error("invalid ACP transcript: expected provider {expected}, received {actual} ({protocol})")]
    Protocol {
        expected: String,
        actual: String,
        protocol: String,
    },
    #[error("replay fixture {fixture}: {detail}")]
    Fixture { fixture: String, detail: String },
}

use t3_contracts::orchestration_v2::ProviderReplayEntryRuntimeExitStatus;
pub use t3_contracts::orchestration_v2::{
    ProviderReplayEntry as ReplayEntry, ProviderReplayTranscript as ReplayTranscript,
};

pub fn decode_ndjson(input: &str) -> Result<ReplayTranscript, ReplayError> {
    let mut records = input
        .lines()
        .enumerate()
        .map(|(index, line)| (index, t3_contracts::wire::trim_js(line)))
        .filter(|(_, line)| !line.is_empty());
    let (index, line) = records.next().ok_or(ReplayError::Empty)?;
    let mut header: Value = serde_json::from_str(line).map_err(|e| ReplayError::Record {
        line: index + 1,
        detail: e.to_string(),
    })?;
    if header["type"] != "transcript_start" {
        return Err(ReplayError::MissingHeader);
    }
    let mut entries: Vec<ReplayEntry> = Vec::new();
    for (index, line) in records {
        let entry = serde_json::from_str(line).map_err(|e| ReplayError::Record {
            line: index + 1,
            detail: e.to_string(),
        })?;
        entries.push(entry);
    }
    header["entries"] = serde_json::to_value(entries).map_err(|e| ReplayError::Record {
        line: 1,
        detail: e.to_string(),
    })?;
    serde_json::from_value(header).map_err(|e| ReplayError::Record {
        line: 1,
        detail: e.to_string(),
    })
}

pub fn decode_acp(
    mut transcript: ReplayTranscript,
    expected: &str,
    retarget: bool,
) -> Result<ReplayTranscript, ReplayError> {
    if retarget {
        transcript.provider.0 = expected.into();
    }
    if transcript.provider.0 != expected || transcript.protocol.0 != "acp.ndjson-jsonrpc" {
        return Err(ReplayError::Protocol {
            expected: expected.into(),
            actual: transcript.provider.0,
            protocol: transcript.protocol.0,
        });
    }
    Ok(transcript)
}

pub fn matches_expected(expected: &Value, actual: &Value, workspace: &str) -> bool {
    if expected == "<any>" {
        return true;
    }
    match (expected, actual) {
        (Value::Number(expected), Value::Number(actual)) => expected
            .as_f64()
            .zip(actual.as_f64())
            .is_some_and(|(expected, actual)| {
                expected == actual
                    && (expected != 0.0 || expected.is_sign_negative() == actual.is_sign_negative())
            }),
        (Value::String(expected), Value::String(actual)) => {
            let expected = expected.replace("<workspace>", workspace);
            if !expected.contains("<any>") {
                return expected == *actual;
            }
            let mut offset = 0;
            for part in expected.split("<any>") {
                let Some(index) = actual[offset..].find(part) else {
                    return false;
                };
                offset += index + part.len();
            }
            true
        }
        (Value::Array(expected), Value::Array(actual)) => {
            expected.len() == actual.len()
                && expected
                    .iter()
                    .zip(actual)
                    .all(|(e, a)| matches_expected(e, a, workspace))
        }
        (Value::Object(expected), Value::Object(actual)) => {
            expected.len() == actual.len()
                && expected.iter().all(|(key, e)| {
                    actual
                        .get(key)
                        .is_some_and(|a| matches_expected(e, a, workspace))
                })
        }
        _ => expected == actual,
    }
}

pub fn materialize(
    value: &Value,
    session: Option<&str>,
    workspace: &str,
    turn_key: Option<&str>,
) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace("<workspace>", workspace)),
        Value::Array(values) => values
            .iter()
            .map(|v| materialize(v, session, workspace, turn_key))
            .collect(),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| {
                    let value = match (key.as_str(), session, turn_key) {
                        ("sessionId", Some(session), _) => json!(session),
                        ("toolCallId", _, Some(turn)) => json!(format!(
                            "{turn}-{}",
                            value
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| value.to_string())
                        )),
                        _ => materialize(value, session, workspace, turn_key),
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        _ => value.clone(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplayFrame {
    pub frame: Value,
    pub after_ms: Option<u64>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayTurn {
    pub frames: Vec<ReplayFrame>,
    pub stop_reason: String,
}

pub fn parse_replay_turns(input: &str) -> Result<Vec<ReplayTurn>, ReplayError> {
    let mut turns = Vec::new();
    let mut active: Option<usize> = None;
    for (index, line) in input
        .lines()
        .enumerate()
        .filter(|(_, line)| !t3_contracts::wire::trim_js(line).is_empty())
    {
        let record: Value = serde_json::from_str(line).map_err(|e| ReplayError::Record {
            line: index + 1,
            detail: e.to_string(),
        })?;
        let frame = &record["frame"];
        if frame.is_null() {
            continue;
        }
        if record["type"] == "expect_outbound"
            && frame["kind"] == "request"
            && frame["method"] == "session/prompt"
        {
            active = Some(turns.len());
            turns.push(ReplayTurn {
                frames: Vec::new(),
                stop_reason: "end_turn".into(),
            });
            continue;
        }
        let Some(turn) = active.map(|i| &mut turns[i]) else {
            continue;
        };
        if record["type"] != "emit_inbound" {
            continue;
        }
        if frame["kind"] == "response" && frame["method"] == "session/prompt" {
            if let Some(reason) = frame["result"]["stopReason"].as_str() {
                turn.stop_reason = reason.into();
            }
            active = None;
        } else if is_replayed_frame(frame) {
            turn.frames.push(ReplayFrame {
                frame: frame.clone(),
                after_ms: record["afterMs"].as_u64(),
            });
        }
    }
    Ok(turns)
}

pub fn is_replayed_frame(frame: &Value) -> bool {
    if frame["kind"] == "request" {
        return matches!(
            frame["method"].as_str(),
            Some("session/request_permission" | "elicitation/create")
        );
    }
    frame["kind"] == "notification"
        && frame["method"] == "session/update"
        && matches!(
            frame["params"]["update"]["sessionUpdate"].as_str(),
            Some(
                "agent_message_chunk"
                    | "agent_thought_chunk"
                    | "tool_call"
                    | "tool_call_update"
                    | "plan"
                    | "plan_update"
                    | "plan_removed"
                    | "session_info_update"
                    | "usage_update"
            )
        )
}

pub fn stream_chunks(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut start = 0;
    let mut saw_space = false;
    for (index, ch) in text.char_indices() {
        if !ch.is_whitespace() && saw_space {
            words.push(&text[start..index]);
            start = index;
        }
        saw_space = ch.is_whitespace();
    }
    if start < text.len() {
        words.push(&text[start..]);
    }
    let chunks: Vec<_> = words.chunks(3).map(|words| words.concat()).collect();
    if chunks.is_empty() {
        vec![text.into()]
    } else {
        chunks
    }
}

pub fn permission_rejected_tool_call(request: &Value, answer: &Value) -> Option<String> {
    if request["method"] != "session/request_permission" {
        return None;
    }
    let outcome = &answer["outcome"];
    let chosen = request["params"]["options"].as_array().and_then(|options| {
        options
            .iter()
            .find(|o| o["optionId"] == outcome["optionId"])
    });
    let rejected = outcome["outcome"] != "selected"
        || chosen
            .and_then(|o| o["kind"].as_str())
            .is_some_and(|k| k.starts_with("reject"));
    rejected
        .then(|| {
            request["params"]["toolCall"]["toolCallId"]
                .as_str()
                .map(str::to_owned)
        })
        .flatten()
}

#[derive(Clone)]
pub struct ReplayGate {
    states: Arc<BTreeMap<String, watch::Sender<(bool, bool)>>>,
    finish: Arc<Mutex<Vec<Duration>>>,
    finish_changed: Arc<watch::Sender<u64>>,
}
impl ReplayGate {
    pub fn new(labels: impl IntoIterator<Item = String>) -> Result<Self, ReplayError> {
        let mut states = BTreeMap::new();
        for label in labels {
            if states
                .insert(label.clone(), watch::channel((false, false)).0)
                .is_some()
            {
                return Err(ReplayError::DuplicateGate(label));
            }
        }
        Ok(Self {
            states: Arc::new(states),
            finish: Arc::default(),
            finish_changed: Arc::new(watch::channel(0).0),
        })
    }
    pub async fn before_emit(&self, label: Option<&str>, cancellation: &CancellationToken) {
        let Some(state) = label.and_then(|label| self.states.get(label)) else {
            return;
        };
        let mut changes = state.subscribe();
        state.send_modify(|state| state.0 = true);
        loop {
            if changes.borrow_and_update().1 {
                return;
            }
            tokio::select! { _ = cancellation.cancelled() => return, result = changes.changed() => if result.is_err() { return; } }
        }
    }
    pub async fn wait_for_reached(&self, label: &str) -> bool {
        let Some(state) = self.states.get(label) else {
            return false;
        };
        let mut changes = state.subscribe();
        loop {
            if changes.borrow_and_update().0 {
                return true;
            }
            if changes.changed().await.is_err() {
                return false;
            }
        }
    }
    pub fn has_reached(&self, label: &str) -> bool {
        self.states.get(label).is_some_and(|s| s.borrow().0)
    }
    pub fn release(&self, label: &str) -> bool {
        self.states.get(label).is_some_and(|state| {
            state.send_if_modified(|state| {
                if state.1 {
                    false
                } else {
                    state.1 = true;
                    true
                }
            })
        })
    }
    pub fn release_all(&self) {
        for label in self.states.keys() {
            self.release(label);
        }
    }
    pub fn record_finish_armed(&self, duration: Duration) {
        let count = {
            let mut finish = self.finish.lock().unwrap_or_else(|e| e.into_inner());
            finish.push(duration);
            finish.len() as u64
        };
        self.finish_changed.send_replace(count);
    }
    pub fn finish_armed_count(&self) -> usize {
        self.finish.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
    pub async fn wait_for_finish_armed(&self, seen: usize) -> Duration {
        let mut changes = self.finish_changed.subscribe();
        loop {
            {
                let finish = self.finish.lock().unwrap_or_else(|e| e.into_inner());
                if finish.len() > seen {
                    return finish[finish.len() - 1];
                }
            }
            if changes.changed().await.is_err() {
                return Duration::ZERO;
            }
        }
    }
}

impl Default for ReplayGate {
    fn default() -> Self {
        Self {
            states: Arc::default(),
            finish: Arc::default(),
            finish_changed: Arc::new(watch::channel(0).0),
        }
    }
}

pub struct RecordedFixture {
    pub name: &'static str,
    pub file: &'static str,
    pub ndjson: &'static str,
}

pub const DEV_SCENARIOS: &[(&str, &str, &str)] = &[
    (
        "showcase",
        "Showcase",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/scripts/dev-replay/showcase_transcript.ndjson"
        )),
    ),
    (
        "todo-list",
        "Plan and reads",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/todo_list/registry_transcript.ndjson"
        )),
    ),
    (
        "approval",
        "Write approval",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/tool_call_read_only_on_request/registry_transcript.ndjson"
        )),
    ),
    (
        "question",
        "Question",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/acp_elicitation/registry_transcript.ndjson"
        )),
    ),
    (
        "multi-turn",
        "Multi turn",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/multi_turn/registry_transcript.ndjson"
        )),
    ),
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayStatus {
    pub scenario: String,
    pub cursor: usize,
    pub total: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<Value>,
}
impl ReplayStatus {
    pub fn assert_complete(&self) -> Result<(), ReplayError> {
        if self.failure.is_none() && self.cursor == self.total {
            return Ok(());
        }
        Err(ReplayError::Mismatch {
            cursor: self.cursor,
            detail: "transcript not fully consumed".into(),
            expected: Box::new(json!(self.total)),
            actual: Box::new(serde_json::to_value(self).unwrap_or(Value::Null)),
        })
    }
}

pub struct ReplayCursor {
    pub transcript: ReplayTranscript,
    pub status: ReplayStatus,
    workspace: String,
    pending_client: BTreeMap<String, Value>,
    pending_agent: BTreeMap<String, String>,
    next_request: u64,
    process_ended: bool,
}
impl ReplayCursor {
    pub fn new(
        transcript: ReplayTranscript,
        workspace: String,
        previous: Option<&ReplayStatus>,
    ) -> Self {
        let cursor = previous
            .filter(|s| {
                s.failure.is_none()
                    && s.cursor > 0
                    && s.cursor <= transcript.entries.len()
                    && matches!(
                        transcript.entries.get(s.cursor - 1),
                        Some(ReplayEntry::RuntimeExit(_))
                    )
            })
            .map_or(0, |s| s.cursor);
        let status = ReplayStatus {
            scenario: transcript.scenario.0.clone(),
            cursor,
            total: transcript.entries.len(),
            failure: None,
        };
        Self {
            transcript,
            status,
            workspace,
            pending_client: BTreeMap::new(),
            pending_agent: BTreeMap::new(),
            next_request: 1,
            process_ended: false,
        }
    }
    fn mismatch(&mut self, detail: &str, actual: Value) -> ReplayError {
        let expected = self
            .transcript
            .entries
            .get(self.status.cursor)
            .and_then(|entry| serde_json::to_value(entry).ok())
            .unwrap_or(Value::Null);
        self.status.failure = Some(
            json!({"detail":detail,"cursor":self.status.cursor,"expected":expected,"actual":actual}),
        );
        ReplayError::Mismatch {
            cursor: self.status.cursor,
            detail: detail.into(),
            expected: Box::new(expected),
            actual: Box::new(actual),
        }
    }
    pub fn accept(&mut self, message: &Value) -> Result<(), ReplayError> {
        let mut frame = json!({});
        if let Some(method) = message["method"].as_str() {
            frame["kind"] = json!(
                if message.get("id").is_none_or(|id| id.is_null() || id == "") {
                    "notification"
                } else {
                    "request"
                }
            );
            frame["method"] = json!(method);
            if let Some(params) = message.get("params") {
                frame["params"] = params.clone();
            }
        } else {
            let id = message
                .get("id")
                .filter(|id| !id.is_null())
                .ok_or_else(|| {
                    self.mismatch("could not identify outbound ACP frame", message.clone())
                })?;
            let method = self
                .pending_agent
                .get(&id.to_string())
                .cloned()
                .ok_or_else(|| self.mismatch("no pending agent request", message.clone()))?;
            frame["kind"] = json!("response");
            frame["method"] = json!(method);
            for key in ["result", "error"] {
                if let Some(value) = message.get(key) {
                    frame[key] = value.clone();
                }
            }
        }
        if self.process_ended
            || !matches!(self.transcript.entries.get(self.status.cursor), Some(ReplayEntry::ExpectOutbound(entry)) if matches_expected(&entry.frame, &frame, &self.workspace))
        {
            return Err(self.mismatch("unexpected outbound ACP frame", frame));
        }
        if let Some(id) = message.get("id") {
            if frame["kind"] == "request" {
                self.pending_client.insert(
                    frame["method"].as_str().unwrap_or_default().into(),
                    id.clone(),
                );
            }
            if frame["kind"] == "response" {
                self.pending_agent.remove(&id.to_string());
            }
        }
        self.status.cursor += 1;
        Ok(())
    }
    pub fn flush_inbound(&mut self) -> Result<Vec<(Option<String>, Value)>, ReplayError> {
        let mut messages = Vec::new();
        while !self.process_ended {
            let Some(entry) = self.transcript.entries.get(self.status.cursor).cloned() else {
                break;
            };
            match entry {
                ReplayEntry::ExpectOutbound(_) => break,
                ReplayEntry::RuntimeExit(entry) => {
                    if entry.status == ProviderReplayEntryRuntimeExitStatus::Error {
                        return Err(self.mismatch(
                            "recorded runtime exit was not successful or cancelled",
                            entry.error.flatten().unwrap_or(json!("error")),
                        ));
                    }
                    self.status.cursor += 1;
                    self.process_ended = self.status.cursor < self.transcript.entries.len();
                }
                ReplayEntry::EmitInbound(entry) => {
                    let label = entry.label.flatten().map(|s| s.0);
                    let frame = materialize(&entry.frame, None, &self.workspace, None);
                    let mut message = json!({"jsonrpc":"2.0"});
                    match frame["kind"].as_str() {
                        Some("request") => {
                            let id = self.next_request;
                            self.next_request += 1;
                            let Some(method) = frame["method"].as_str() else {
                                return Err(self.mismatch("invalid emit_inbound frame", frame));
                            };
                            self.pending_agent
                                .insert(json!(id).to_string(), method.into());
                            message["id"] = json!(id);
                            message["headers"] = json!([]);
                            message["method"] = frame["method"].clone();
                        }
                        Some("notification") => message["method"] = frame["method"].clone(),
                        Some("response") => {
                            let Some(id) = frame["method"]
                                .as_str()
                                .and_then(|method| self.pending_client.remove(method))
                            else {
                                return Err(self.mismatch("no pending client request", frame));
                            };
                            message["id"] = id;
                        }
                        _ => return Err(self.mismatch("invalid emit_inbound frame", frame)),
                    }
                    for key in ["params", "result", "error"] {
                        if let Some(value) = frame.get(key) {
                            message[key] = value.clone();
                        }
                    }
                    messages.push((label, message));
                    self.status.cursor += 1;
                }
            }
        }
        Ok(messages)
    }
    pub fn close_input(&mut self) -> Result<(), ReplayError> {
        if self.status.cursor == self.status.total
            || self
                .status
                .cursor
                .checked_sub(1)
                .and_then(|index| self.transcript.entries.get(index))
                .is_some_and(|entry| matches!(entry, ReplayEntry::RuntimeExit(_)))
        {
            return Ok(());
        }
        Err(self.mismatch("input closed before transcript completion", Value::Null))
    }
}
