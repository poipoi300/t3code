use crate::testing::replay::{ReplayEntry, ReplayTranscript};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::Future,
    sync::{Arc, Mutex, MutexGuard},
};
use t3_contracts::orchestration_v2::ProviderReplayEntryRuntimeExitStatus;
use tokio::sync::{oneshot, watch};

pub const CURSOR_PROTOCOL: &str = "cursor-agent-sdk.local";

#[derive(Debug, Clone, thiserror::Error)]
#[error("Cursor SDK replay {kind:?} at entry {cursor} in {scenario}: {detail}")]
pub struct CursorReplayError {
    pub kind: CursorReplayErrorKind,
    pub scenario: String,
    pub cursor: usize,
    pub detail: String,
    pub expected: Option<Value>,
    pub actual: Option<Value>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorReplayErrorKind {
    Decode,
    Exhausted,
    Mismatch,
    Incomplete,
    Runtime,
}
pub type CursorReplayResult<T> = Result<T, Box<CursorReplayError>>;

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}
pub fn normalize_frame(value: &Value) -> Value {
    match value {
        Value::String(text) if text.starts_with("Context handoff (") => {
            let marker = text.find("\n\nUser message:\n");
            let header = text.find(":\n");
            match (header, marker) {
                (Some(header), Some(marker)) if header < marker => json!(format!(
                    "{}<dynamic-summary>{}",
                    &text[..header + 2],
                    &text[marker..]
                )),
                _ => value.clone(),
            }
        }
        Value::Array(values) => values.iter().map(normalize_frame).collect(),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| (key.clone(), normalize_frame(value)))
            .collect(),
        _ => value.clone(),
    }
}

pub fn sanitize_replay_value(value: &Value, replacements: &[(&str, &str)]) -> Value {
    fn text(value: &str, replacements: &[(&str, &str)]) -> String {
        replacements
            .iter()
            .filter(|(from, _)| !from.is_empty())
            .fold(value.to_owned(), |value, (from, to)| {
                value.replace(from, to)
            })
    }
    match value {
        Value::String(value) => json!(text(value, replacements)),
        Value::Array(values) => values
            .iter()
            .map(|value| sanitize_replay_value(value, replacements))
            .collect(),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| {
                (
                    text(key, replacements),
                    sanitize_replay_value(value, replacements),
                )
            })
            .collect(),
        _ => value.clone(),
    }
}

fn copy_fields(source: &Value, target: &mut Value, fields: &[&str]) {
    for field in fields {
        if let Some(value) = source.get(*field) {
            target[*field] = value.clone();
        }
    }
}
pub fn logged_agent_options(options: &Value) -> Value {
    let mut result = json!({});
    copy_fields(options, &mut result, &["model", "mode"]);
    if options.get("name").is_some() {
        result["hasName"] = json!(true);
    }
    if let Some(local) = options.get("local") {
        let mut logged = json!({});
        copy_fields(
            local,
            &mut logged,
            &["autoReview", "settingSources", "enableAgentRetries"],
        );
        if local.get("cwd").is_some() {
            logged["hasCwd"] = json!(true);
        }
        if local.get("customTools").is_some() {
            logged["hasCustomTools"] = json!(true);
        }
        if let Some(sandbox) = local
            .get("sandboxOptions")
            .and_then(|value| value.get("enabled"))
        {
            logged["sandboxEnabled"] = sandbox.clone();
        }
        result["local"] = logged;
    }
    if let Some(agents) = options.get("agents").and_then(Value::as_object) {
        let mut names: Vec<_> = agents.keys().collect();
        names.sort();
        result["agents"] = json!(names);
    }
    result
}
pub fn logged_send_options(options: &Value) -> Value {
    let mut result = json!({});
    copy_fields(options, &mut result, &["model", "mode", "idempotencyKey"]);
    if let Some(servers) = options.get("mcpServers").and_then(Value::as_object) {
        let mut names: Vec<_> = servers.keys().collect();
        names.sort();
        result["hasMcpServers"] = json!(true);
        result["mcpServerNames"] = json!(names);
    }
    if let Some(local) = options.get("local") {
        let mut logged = json!({});
        copy_fields(local, &mut logged, &["force"]);
        if local.get("customTools").is_some() {
            logged["hasCustomTools"] = json!(true);
        }
        result["local"] = logged;
    }
    result
}

struct State {
    cursor: usize,
    failure: Option<Box<CursorReplayError>>,
    steer_acks: BTreeMap<String, oneshot::Sender<Value>>,
}
pub struct CursorReplayRunner {
    transcript: ReplayTranscript,
    state: Mutex<State>,
    progress: watch::Sender<usize>,
}
impl CursorReplayRunner {
    pub fn new(transcript: ReplayTranscript) -> CursorReplayResult<Arc<Self>> {
        if transcript.provider.0 != "cursor" || transcript.protocol.0 != CURSOR_PROTOCOL {
            return Err(Box::new(CursorReplayError {
                kind: CursorReplayErrorKind::Decode,
                scenario: transcript.scenario.0,
                cursor: 0,
                detail: format!(
                    "expected cursor/{CURSOR_PROTOCOL}; received {}/{}",
                    transcript.provider.0, transcript.protocol.0
                ),
                expected: None,
                actual: None,
            }));
        }
        Ok(Arc::new(Self {
            transcript,
            state: Mutex::new(State {
                cursor: 0,
                failure: None,
                steer_acks: BTreeMap::new(),
            }),
            progress: watch::channel(0).0,
        }))
    }
    fn error(
        &self,
        state: &mut State,
        kind: CursorReplayErrorKind,
        detail: impl Into<String>,
        expected: Option<Value>,
        actual: Option<Value>,
    ) -> Box<CursorReplayError> {
        let error = Box::new(CursorReplayError {
            kind,
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
    fn outbound(&self, state: &mut State, actual: Value) -> CursorReplayResult<()> {
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        let Some(entry) = self.transcript.entries.get(state.cursor) else {
            return Err(self.error(
                state,
                CursorReplayErrorKind::Exhausted,
                "outbound after transcript end",
                None,
                Some(actual),
            ));
        };
        if let ReplayEntry::ExpectOutbound(entry) = entry
            && normalize_frame(&entry.frame) == normalize_frame(&actual)
        {
            self.advance(state);
            return Ok(());
        }
        let expected = match entry {
            ReplayEntry::ExpectOutbound(entry) => entry.frame.clone(),
            entry => json!(entry),
        };
        Err(self.error(
            state,
            CursorReplayErrorKind::Mismatch,
            "outbound frame",
            Some(expected),
            Some(actual),
        ))
    }
    fn inbound(&self, state: &mut State, kind: &str) -> CursorReplayResult<Value> {
        if let Some(ReplayEntry::EmitInbound(entry)) = self.transcript.entries.get(state.cursor)
            && entry.frame["type"] == kind
        {
            let frame = entry.frame.clone();
            self.advance(state);
            return Ok(frame);
        }
        let actual = self
            .transcript
            .entries
            .get(state.cursor)
            .map(|entry| json!(entry));
        Err(self.error(
            state,
            CursorReplayErrorKind::Mismatch,
            "inbound frame",
            Some(json!({"type":kind})),
            actual,
        ))
    }
    pub fn assert_complete(&self) -> CursorReplayResult<()> {
        let state = lock(&self.state);
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        if state.cursor == self.transcript.entries.len() {
            return Ok(());
        }
        Err(Box::new(CursorReplayError {
            kind: CursorReplayErrorKind::Incomplete,
            scenario: self.transcript.scenario.0.clone(),
            cursor: state.cursor,
            detail: format!(
                "{} entries remain",
                self.transcript.entries.len() - state.cursor
            ),
            expected: None,
            actual: None,
        }))
    }
    pub fn open(
        self: &Arc<Self>,
        operation: &str,
        agent_id: Option<&str>,
        options: &Value,
    ) -> CursorReplayResult<CursorReplaySession> {
        let mut state = lock(&self.state);
        let mut frame = json!({"type":"agent.open","operation":operation,"options":logged_agent_options(options)});
        if let Some(agent_id) = agent_id {
            frame["agentId"] = json!(agent_id);
        }
        self.outbound(&mut state, frame)?;
        let opened = self.inbound(&mut state, "agent.opened")?;
        let agent_id = self.string_field(&mut state, &opened, "agentId")?;
        Ok(CursorReplaySession {
            runner: self.clone(),
            agent_id,
        })
    }
    fn string_field(
        &self,
        state: &mut State,
        frame: &Value,
        key: &str,
    ) -> CursorReplayResult<String> {
        frame[key].as_str().map(str::to_owned).ok_or_else(|| {
            self.error(
                state,
                CursorReplayErrorKind::Mismatch,
                format!("missing string {key}"),
                None,
                Some(frame.clone()),
            )
        })
    }
}
pub struct CursorReplaySession {
    pub agent_id: String,
    runner: Arc<CursorReplayRunner>,
}
impl CursorReplaySession {
    pub fn send(&self, message: Value, options: &Value) -> CursorReplayResult<CursorReplayRun> {
        let mut state = lock(&self.runner.state);
        self.runner.outbound(
            &mut state,
            json!({"type":"run.start","message":message,"options":logged_send_options(options)}),
        )?;
        let frame = self.runner.inbound(&mut state, "run.started")?;
        let run_id = self.runner.string_field(&mut state, &frame, "runId")?;
        let agent_id = self.runner.string_field(&mut state, &frame, "agentId")?;
        Ok(CursorReplayRun {
            runner: self.runner.clone(),
            run_id,
            agent_id,
        })
    }
    pub fn list_messages(&self) -> CursorReplayResult<Value> {
        let mut state = lock(&self.runner.state);
        self.runner.outbound(
            &mut state,
            json!({"type":"agent.messages.list","agentId":self.agent_id}),
        )?;
        Ok(self.runner.inbound(&mut state, "agent.messages")?["messages"].clone())
    }
    pub fn close(&self) -> CursorReplayResult<()> {
        self.runner.outbound(
            &mut lock(&self.runner.state),
            json!({"type":"agent.close","agentId":self.agent_id}),
        )
    }
}
pub struct CursorReplayRun {
    pub run_id: String,
    pub agent_id: String,
    runner: Arc<CursorReplayRunner>,
}
enum RunStep {
    Wait,
    Update(Value),
    Done(Value),
    Continue,
}
impl CursorReplayRun {
    pub fn cancel(&self) -> CursorReplayResult<()> {
        self.runner.outbound(
            &mut lock(&self.runner.state),
            json!({"type":"run.cancel","runId":self.run_id}),
        )
    }
    pub async fn steer(&self, text: &str) -> CursorReplayResult<Value> {
        let (sender, receiver) = oneshot::channel();
        let mut progress = self.runner.progress.subscribe();
        {
            let mut state = lock(&self.runner.state);
            state.steer_acks.insert(self.run_id.clone(), sender);
            self.runner.outbound(
                &mut state,
                json!({"type":"run.steer","runId":self.run_id,"text":text}),
            )?;
        }
        tokio::pin!(receiver);
        loop {
            if let Some(error) = lock(&self.runner.state).failure.clone() {
                return Err(error);
            }
            tokio::select! {
                result=&mut receiver => return result.map_err(|_| {
                    let mut state=lock(&self.runner.state);
                    self.runner.error(&mut state,CursorReplayErrorKind::Runtime,"steer acknowledgment closed",None,None)
                }),
                _=progress.changed()=>{}
            }
        }
    }
    fn next(&self) -> CursorReplayResult<RunStep> {
        let mut state = lock(&self.runner.state);
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        let Some(entry) = self.runner.transcript.entries.get(state.cursor) else {
            return Err(self.runner.error(
                &mut state,
                CursorReplayErrorKind::Exhausted,
                "waiting for run.completed",
                Some(json!({"runId":self.run_id})),
                None,
            ));
        };
        match entry {
            ReplayEntry::ExpectOutbound(entry)
                if matches!(
                    entry.frame["type"].as_str(),
                    Some("run.cancel" | "run.steer")
                ) && entry.frame["runId"] == self.run_id =>
            {
                Ok(RunStep::Wait)
            }
            ReplayEntry::RuntimeExit(entry) => {
                self.runner.advance(&mut state);
                if entry.status == ProviderReplayEntryRuntimeExitStatus::Success {
                    return Ok(RunStep::Continue);
                }
                let actual = entry
                    .error
                    .clone()
                    .flatten()
                    .unwrap_or_else(|| json!(entry.status));
                let mut error = self.runner.error(
                    &mut state,
                    CursorReplayErrorKind::Runtime,
                    "recorded runtime exit",
                    None,
                    Some(actual),
                );
                error.cursor -= 1;
                state.failure = Some(error.clone());
                Err(error)
            }
            ReplayEntry::EmitInbound(entry) => {
                let frame = &entry.frame;
                match frame["type"].as_str() {
                    Some("interaction.update") if frame["runId"] == self.run_id => {
                        self.runner.advance(&mut state);
                        Ok(RunStep::Update(frame["update"].clone()))
                    }
                    Some("run.steered")
                        if frame["runId"] == self.run_id
                            && state.steer_acks.contains_key(&self.run_id) =>
                    {
                        let ack = state
                            .steer_acks
                            .remove(&self.run_id)
                            .expect("checked pending acknowledgment");
                        self.runner.advance(&mut state);
                        let _ = ack.send(frame["outcome"].clone());
                        Ok(RunStep::Continue)
                    }
                    Some("run.completed") if frame["result"]["id"] == self.run_id => {
                        self.runner.advance(&mut state);
                        Ok(RunStep::Done(frame["result"].clone()))
                    }
                    _ => Err(self.runner.error(
                        &mut state,
                        CursorReplayErrorKind::Mismatch,
                        "unexpected run event",
                        Some(json!({"runId":self.run_id})),
                        Some(frame.clone()),
                    )),
                }
            }
            entry => Err(self.runner.error(
                &mut state,
                CursorReplayErrorKind::Mismatch,
                "expected cancellation or steering",
                Some(json!({"type":"run.cancel | run.steer","runId":self.run_id})),
                Some(json!(entry)),
            )),
        }
    }
    pub async fn wait<F, Fut>(&self, mut on_delta: F) -> CursorReplayResult<Value>
    where
        F: FnMut(Value) -> Fut,
        Fut: Future<Output = Result<(), Value>>,
    {
        let mut progress = self.runner.progress.subscribe();
        loop {
            progress.borrow_and_update();
            match self.next()? {
                RunStep::Wait => {
                    let _ = progress.changed().await;
                }
                RunStep::Update(update) => {
                    if let Err(cause) = on_delta(update).await {
                        let mut state = lock(&self.runner.state);
                        let mut error = self.runner.error(
                            &mut state,
                            CursorReplayErrorKind::Runtime,
                            "delta callback",
                            None,
                            Some(cause),
                        );
                        error.cursor -= 1;
                        state.failure = Some(error.clone());
                        return Err(error);
                    }
                    tokio::task::yield_now().await;
                }
                RunStep::Done(result) => return Ok(result),
                RunStep::Continue => {}
            }
        }
    }
}

// Local shim: the integrator supplies the native Cursor adapter's SDK-runner injection.
#[async_trait::async_trait]
pub trait CursorAdapterFactory: Send + Sync {
    async fn create(
        &self,
        instance_id: t3_contracts::provider_instance::ProviderInstanceId,
        runner: Arc<CursorReplayRunner>,
        environment: BTreeMap<String, String>,
        clock: Arc<dyn crate::api::Clock>,
        ids: Arc<dyn crate::api::IdGenerator>,
    ) -> crate::provider::ProviderResult<Arc<dyn crate::provider::ProviderAdapter>>;
}
