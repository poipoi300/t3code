use crate::testing::replay::{ReplayEntry, ReplayGate, ReplayTranscript};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use t3_contracts::orchestration_v2::ProviderReplayEntryRuntimeExitStatus;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

pub const OPENCODE_PROTOCOL: &str = "opencode-sdk.sse";
#[derive(Debug, Clone, thiserror::Error)]
#[error("OpenCode replay at entry {cursor} in {scenario}: {detail}")]
pub struct OpenCodeReplayError {
    pub scenario: String,
    pub cursor: usize,
    pub detail: String,
    pub expected: Value,
    pub actual: Value,
}
pub type OpenCodeReplayResult<T> = Result<T, Box<OpenCodeReplayError>>;
fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}

pub fn replay_value_matches(expected: &Value, actual: &Value) -> bool {
    if expected == "<any>" || expected == "<workspace>" {
        return true;
    }
    match (expected, actual) {
        (Value::Array(expected), Value::Array(actual)) => {
            expected.len() == actual.len()
                && expected
                    .iter()
                    .zip(actual)
                    .all(|(expected, actual)| replay_value_matches(expected, actual))
        }
        (Value::Object(expected), Value::Object(actual)) => expected.iter().all(|(key, value)| {
            actual
                .get(key)
                .is_some_and(|actual| replay_value_matches(value, actual))
        }),
        (Value::Number(expected), Value::Number(actual)) => expected
            .as_f64()
            .zip(actual.as_f64())
            .is_some_and(|(expected, actual)| {
                expected == actual
                    && (expected != 0.0 || expected.is_sign_negative() == actual.is_sign_negative())
            }),
        _ => expected == actual,
    }
}
pub fn materialize_message_ids(value: &Value, ids: &BTreeMap<String, String>) -> Value {
    match value {
        Value::Array(values) => values
            .iter()
            .map(|value| materialize_message_ids(value, ids))
            .collect(),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| {
                let replacement = if matches!(
                    key.as_str(),
                    "id" | "messageID" | "parentID" | "inboxID" | "before"
                ) {
                    value
                        .as_str()
                        .and_then(|value| ids.get(value))
                        .map(|value| json!(value))
                } else {
                    None
                };
                (
                    key.clone(),
                    replacement.unwrap_or_else(|| materialize_message_ids(value, ids)),
                )
            })
            .collect(),
        _ => value.clone(),
    }
}
struct State {
    cursor: usize,
    exited: bool,
    failure: Option<Box<OpenCodeReplayError>>,
    message_ids: BTreeMap<String, String>,
}
pub struct OpenCodeReplayController {
    transcript: ReplayTranscript,
    state: Mutex<State>,
    progress: watch::Sender<usize>,
    gate: ReplayGate,
}
impl OpenCodeReplayController {
    pub fn new(transcript: ReplayTranscript, gate: ReplayGate) -> OpenCodeReplayResult<Arc<Self>> {
        Self::for_protocol(transcript, gate, OPENCODE_PROTOCOL)
    }
    pub fn for_protocol(
        transcript: ReplayTranscript,
        gate: ReplayGate,
        protocol: &str,
    ) -> OpenCodeReplayResult<Arc<Self>> {
        if transcript.provider.0 != "opencode"
            || transcript.protocol.0 != protocol
            || !matches!(protocol, OPENCODE_PROTOCOL | "opencode2-http.sse")
        {
            return Err(Box::new(OpenCodeReplayError {
                scenario: transcript.scenario.0.clone(),
                cursor: 0,
                detail: "invalid OpenCode SDK transcript".into(),
                expected: json!({"provider":"opencode","protocol":protocol}),
                actual: json!({"provider":transcript.provider,"protocol":transcript.protocol}),
            }));
        }
        Ok(Arc::new(Self {
            transcript,
            state: Mutex::new(State {
                cursor: 0,
                exited: false,
                failure: None,
                message_ids: BTreeMap::new(),
            }),
            progress: watch::channel(0).0,
            gate,
        }))
    }
    fn fail(
        &self,
        state: &mut State,
        detail: impl Into<String>,
        expected: Value,
        actual: Value,
    ) -> Box<OpenCodeReplayError> {
        let error = Box::new(OpenCodeReplayError {
            scenario: self.transcript.scenario.0.clone(),
            cursor: state.cursor,
            detail: detail.into(),
            expected,
            actual,
        });
        state.failure = Some(error.clone());
        self.progress.send_replace(state.cursor);
        error
    }
    fn advance(&self, state: &mut State) {
        state.cursor += 1;
        self.progress.send_replace(state.cursor);
    }
    pub fn finished(&self) -> bool {
        lock(&self.state).cursor >= self.transcript.entries.len()
    }
    pub fn exited(&self) -> bool {
        lock(&self.state).exited
    }
    pub fn expect_outbound(&self, actual: Value) -> OpenCodeReplayResult<()> {
        let mut state = lock(&self.state);
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        let entry = self.transcript.entries.get(state.cursor);
        let expected = match entry {
            Some(ReplayEntry::ExpectOutbound(entry)) => entry.frame.clone(),
            entry => json!(entry),
        };
        let mut ids = state.message_ids.clone();
        let mut conflict = false;
        if matches!(
            expected["type"].as_str(),
            Some("session.promptAsync" | "session.prompt")
        ) && actual["type"] == expected["type"]
        {
            let key = if expected["type"] == "session.promptAsync" {
                "messageID"
            } else {
                "id"
            };
            if let (Some(recorded), Some(actual)) = (
                expected["input"][key].as_str(),
                actual["input"][key].as_str(),
            ) && !matches!(recorded, "<any>" | "<workspace>")
                && !ids.contains_key(recorded)
            {
                conflict = ids.values().any(|value| value == actual);
                if !conflict {
                    ids.insert(recorded.into(), actual.into());
                }
            }
        }
        let expected = materialize_message_ids(&expected, &ids);
        if conflict
            || !matches!(entry, Some(ReplayEntry::ExpectOutbound(_)))
            || !replay_value_matches(&expected, &actual)
        {
            return Err(self.fail(&mut state, "outbound mismatch", expected, actual));
        }
        state.message_ids = ids;
        self.advance(&mut state);
        Ok(())
    }
    pub async fn response(&self, operation: &str) -> OpenCodeReplayResult<Value> {
        let mut progress = self.progress.subscribe();
        loop {
            progress.borrow_and_update();
            let pending = {
                let mut state = lock(&self.state);
                if let Some(error) = &state.failure {
                    return Err(error.clone());
                }
                match self.transcript.entries.get(state.cursor) {
                    Some(ReplayEntry::EmitInbound(entry))
                        if entry.frame["type"] == "sdk.response"
                            && entry.frame["operation"] == operation =>
                    {
                        Some((
                            state.cursor,
                            entry.after_ms.flatten().unwrap_or_default(),
                            entry.frame["data"].clone(),
                        ))
                    }
                    Some(ReplayEntry::RuntimeExit(_)) => {
                        let actual = json!(self.transcript.entries.get(state.cursor));
                        return Err(self.fail(
                            &mut state,
                            "response unavailable",
                            json!({"type":"sdk.response","operation":operation}),
                            actual,
                        ));
                    }
                    _ => None,
                }
            };
            if let Some((cursor, delay, data)) = pending {
                if delay > 0 {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                let mut state = lock(&self.state);
                if state.cursor != cursor {
                    continue;
                }
                if let Some(error) = &state.failure {
                    return Err(error.clone());
                }
                let data = materialize_message_ids(&data, &state.message_ids);
                self.advance(&mut state);
                return Ok(data);
            }
            let _ = progress.changed().await;
        }
    }
    pub async fn request(&self, operation: &str, input: Value) -> OpenCodeReplayResult<Value> {
        self.expect_outbound(json!({"type":operation,"input":input}))?;
        self.response(operation).await
    }
    pub async fn until_inbound_delivered(&self, events_only: bool) -> OpenCodeReplayResult<()> {
        let mut progress = self.progress.subscribe();
        loop {
            progress.borrow_and_update();
            {
                let state = lock(&self.state);
                if let Some(error) = &state.failure {
                    return Err(error.clone());
                }
                if !matches!(self.transcript.entries.get(state.cursor),Some(ReplayEntry::EmitInbound(entry)) if !events_only || entry.frame["type"]=="sdk.event")
                {
                    return Ok(());
                }
            }
            let _ = progress.changed().await;
        }
    }
    pub async fn next_event(
        &self,
        cancellation: &CancellationToken,
    ) -> OpenCodeReplayResult<Option<Value>> {
        let mut progress = self.progress.subscribe();
        loop {
            if cancellation.is_cancelled() {
                return Ok(None);
            }
            progress.borrow_and_update();
            let pending = {
                let mut state = lock(&self.state);
                if let Some(error) = &state.failure {
                    return Err(error.clone());
                }
                match self.transcript.entries.get(state.cursor) {
                    Some(ReplayEntry::EmitInbound(entry)) if entry.frame["type"] == "sdk.event" => {
                        Some((
                            state.cursor,
                            entry
                                .label
                                .as_ref()
                                .and_then(Option::as_ref)
                                .map(|label| label.0.clone()),
                            entry.after_ms.flatten().unwrap_or_default(),
                            entry.frame["event"].clone(),
                        ))
                    }
                    Some(ReplayEntry::RuntimeExit(entry)) => {
                        let success = entry.status == ProviderReplayEntryRuntimeExitStatus::Success;
                        let actual = json!(entry);
                        self.advance(&mut state);
                        if success {
                            state.exited = true;
                            return Ok(None);
                        }
                        let mut error = self.fail(
                            &mut state,
                            "runtime exit",
                            json!({"status":"success"}),
                            actual,
                        );
                        error.cursor -= 1;
                        state.failure = Some(error.clone());
                        return Err(error);
                    }
                    None => None,
                    _ => None,
                }
            };
            if let Some((cursor, label, delay, event)) = pending {
                self.gate.before_emit(label.as_deref(), cancellation).await;
                if delay > 0 {
                    tokio::select! {_=tokio::time::sleep(Duration::from_millis(delay))=>{},_=cancellation.cancelled()=>return Ok(None)}
                }
                if cancellation.is_cancelled() {
                    return Ok(None);
                }
                let mut state = lock(&self.state);
                if let Some(error) = &state.failure {
                    return Err(error.clone());
                }
                if state.cursor != cursor {
                    continue;
                }
                let event = materialize_message_ids(&event, &state.message_ids);
                self.advance(&mut state);
                return Ok(Some(event));
            }
            tokio::select! {_=progress.changed()=>{},_=cancellation.cancelled()=>return Ok(None)}
        }
    }
    pub fn subscribe(&self) -> OpenCodeReplayResult<()> {
        self.expect_outbound(json!({"type":"event.subscribe"}))
    }
    pub fn assert_complete(&self) -> OpenCodeReplayResult<()> {
        let mut state = lock(&self.state);
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        while matches!(self.transcript.entries.get(state.cursor),Some(ReplayEntry::RuntimeExit(entry)) if entry.status==ProviderReplayEntryRuntimeExitStatus::Success)
        {
            self.advance(&mut state);
        }
        if state.cursor == self.transcript.entries.len() {
            return Ok(());
        }
        Err(Box::new(OpenCodeReplayError {
            scenario: self.transcript.scenario.0.clone(),
            cursor: state.cursor,
            detail: format!(
                "{} unconsumed entries",
                self.transcript.entries.len() - state.cursor
            ),
            expected: Value::Null,
            actual: json!(self.transcript.entries.get(state.cursor)),
        }))
    }
}

// Local shim: the native OpenCode adapter must inject this SDK boundary instead of starting a server.
#[async_trait::async_trait]
pub trait OpenCodeAdapterFactory: Send + Sync {
    async fn create(
        &self,
        instance_id: t3_contracts::provider_instance::ProviderInstanceId,
        controller: Arc<OpenCodeReplayController>,
        clock: Arc<dyn crate::api::Clock>,
        ids: Arc<dyn crate::api::IdGenerator>,
    ) -> crate::provider::ProviderResult<Arc<dyn crate::provider::ProviderAdapter>>;
}
