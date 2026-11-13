use crate::testing::replay::{ReplayEntry, ReplayTranscript};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub const PI_RPC_REPLAY_PROTOCOL: &str = "pi.rpc-jsonl";
pub const PI_REPLAY_ANY: &str = "<any>";
#[derive(Debug, Clone, thiserror::Error)]
#[error("JSONL replay at entry {cursor} in {scenario}, process {process}: {detail}")]
pub struct JsonlReplayError {
    pub scenario: String,
    pub cursor: usize,
    pub process: usize,
    pub detail: String,
    pub expected: Value,
    pub actual: Value,
}
pub type JsonlReplayResult<T> = Result<T, Box<JsonlReplayError>>;
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JsonlFlavor {
    Pi,
    Muse,
}
impl JsonlFlavor {
    fn driver(self) -> &'static str {
        match self {
            Self::Pi => "pi",
            Self::Muse => "muse",
        }
    }
    fn protocol(self) -> &'static str {
        match self {
            Self::Pi => PI_RPC_REPLAY_PROTOCOL,
            Self::Muse => "muse.msp-jsonl",
        }
    }
    fn marker(self) -> &'static str {
        match self {
            Self::Pi => "@p",
            Self::Muse => "@h",
        }
    }
    fn start(self) -> &'static str {
        match self {
            Self::Pi => "process_start",
            Self::Muse => "host_start",
        }
    }
    fn starts_turn(self, frame: &Value) -> bool {
        match self {
            Self::Pi => matches!(
                frame["type"].as_str(),
                Some("prompt" | "compact" | "process_start")
            ),
            Self::Muse => matches!(
                frame.get("method").unwrap_or(&frame["type"]).as_str(),
                Some("turn/start" | "session/compact" | "host_start")
            ),
        }
    }
    fn request_id(self, frame: &Value) -> Option<String> {
        match self {
            Self::Pi => frame["id"]
                .as_str()
                .filter(|id| {
                    id.strip_prefix("t3-")
                        .and_then(|id| id.strip_prefix("prompt-").or(Some(id)))
                        .is_some_and(|digits| {
                            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
                        })
                })
                .map(str::to_owned),
            Self::Muse => frame["method"]
                .as_str()
                .and_then(|_| number_key(&frame["id"])),
        }
    }
}
fn number_key(value: &Value) -> Option<String> {
    value.as_f64().map(|value| {
        if value == 0.0 {
            "0".into()
        } else {
            value.to_string()
        }
    })
}
pub fn replay_value_matches(expected: &Value, actual: &Value) -> bool {
    if expected == PI_REPLAY_ANY {
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
        (Value::Object(expected), Value::Object(actual)) => {
            expected.len() == actual.len()
                && expected.iter().all(|(key, value)| {
                    actual
                        .get(key)
                        .is_some_and(|actual| replay_value_matches(value, actual))
                })
        }
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
fn label(entry: &ReplayEntry) -> Option<&str> {
    match entry {
        ReplayEntry::ExpectOutbound(entry) => entry
            .label
            .as_ref()
            .and_then(Option::as_ref)
            .map(|label| label.0.as_str()),
        ReplayEntry::EmitInbound(entry) => entry
            .label
            .as_ref()
            .and_then(Option::as_ref)
            .map(|label| label.0.as_str()),
        ReplayEntry::RuntimeExit(_) => None,
    }
}
fn ordinal(entry: &ReplayEntry, flavor: JsonlFlavor) -> usize {
    label(entry)
        .and_then(|label| label.rsplit_once(flavor.marker()))
        .and_then(|(_, digits)| {
            (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| digits.parse().ok())
                .flatten()
        })
        .unwrap_or(1)
}
struct Host {
    sender: Option<mpsc::UnboundedSender<Value>>,
    ids: BTreeMap<String, Value>,
    minted: VecDeque<String>,
    exit: CancellationToken,
}
struct State {
    cursor: usize,
    failure: Option<Box<JsonlReplayError>>,
    consumed: BTreeSet<usize>,
    held: VecDeque<(usize, Value)>,
    hosts: Vec<Host>,
}
pub struct JsonlReplayController {
    transcript: ReplayTranscript,
    flavor: JsonlFlavor,
    state: Mutex<State>,
}
pub struct JsonlReplayProcess {
    pub ordinal: usize,
    pub incoming: mpsc::UnboundedReceiver<Value>,
    pub exited: CancellationToken,
}
impl JsonlReplayController {
    pub fn new(transcript: ReplayTranscript, flavor: JsonlFlavor) -> JsonlReplayResult<Arc<Self>> {
        let valid_metadata = flavor != JsonlFlavor::Muse
            || transcript
                .metadata
                .as_ref()
                .and_then(Option::as_ref)
                .and_then(|metadata| metadata.get("commandIds"))
                .and_then(Value::as_array)
                .is_some_and(|hosts| {
                    hosts.iter().all(|host| {
                        host.as_array()
                            .is_some_and(|ids| ids.iter().all(Value::is_string))
                    })
                });
        if transcript.provider.0 != flavor.driver()
            || transcript.protocol.0 != flavor.protocol()
            || !valid_metadata
        {
            return Err(Box::new(JsonlReplayError {
                scenario: transcript.scenario.0.clone(),
                cursor: 0,
                process: 0,
                detail: "invalid transcript header/command IDs".into(),
                expected: json!({"provider":flavor.driver(),"protocol":flavor.protocol()}),
                actual: json!({"provider":transcript.provider,"protocol":transcript.protocol,"metadata":transcript.metadata}),
            }));
        }
        Ok(Arc::new(Self {
            transcript,
            flavor,
            state: Mutex::new(State {
                cursor: 0,
                failure: None,
                consumed: BTreeSet::new(),
                held: VecDeque::new(),
                hosts: Vec::new(),
            }),
        }))
    }
    fn error(
        &self,
        state: &mut State,
        process: usize,
        detail: impl Into<String>,
        actual: Value,
    ) -> Box<JsonlReplayError> {
        let error = Box::new(JsonlReplayError {
            scenario: self.transcript.scenario.0.clone(),
            cursor: state.cursor,
            process,
            detail: detail.into(),
            expected: json!(self.transcript.entries.get(state.cursor)),
            actual,
        });
        if state.failure.is_none() {
            state.failure = Some(error.clone());
            for host in &mut state.hosts {
                host.sender = None;
            }
        }
        state.failure.clone().unwrap_or(error)
    }
    fn with_request_id(&self, expected: &Value, actual: &Value) -> Value {
        if self.flavor.request_id(actual).is_some()
            && (self.flavor.request_id(expected).is_some()
                || (self.flavor == JsonlFlavor::Pi
                    && expected["type"] == "prompt"
                    && expected.get("id").is_none()))
        {
            let mut frame = expected.clone();
            frame["id"] = actual["id"].clone();
            frame
        } else {
            expected.clone()
        }
    }
    pub fn spawn(&self, args: &[String]) -> JsonlReplayResult<JsonlReplayProcess> {
        let (sender, incoming) = mpsc::unbounded_channel();
        let exited = CancellationToken::new();
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        let index = state.hosts.len();
        let minted = self
            .transcript
            .metadata
            .as_ref()
            .and_then(Option::as_ref)
            .and_then(|metadata| metadata.get("commandIds"))
            .and_then(|ids| ids.get(index))
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        state.hosts.push(Host {
            sender: Some(sender),
            ids: BTreeMap::new(),
            minted,
            exit: exited.clone(),
        });
        let process = JsonlReplayProcess {
            ordinal: index + 1,
            incoming,
            exited,
        };
        self.receive_locked(
            &mut state,
            process.ordinal,
            json!({"type":self.flavor.start(),"args":args}),
        )?;
        Ok(process)
    }
    pub fn receive(&self, process: usize, frame: Value) -> JsonlReplayResult<()> {
        self.receive_locked(
            &mut self.state.lock().unwrap_or_else(|error| error.into_inner()),
            process,
            frame,
        )
    }
    fn receive_locked(
        &self,
        state: &mut State,
        process: usize,
        actual: Value,
    ) -> JsonlReplayResult<()> {
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        if process == 0 || process > state.hosts.len() {
            return Err(self.error(state, process, "unknown process", actual));
        }
        let mut possible = false;
        for (index, entry) in self
            .transcript
            .entries
            .iter()
            .enumerate()
            .skip(state.cursor)
        {
            if let ReplayEntry::ExpectOutbound(entry) = entry
                && !state.consumed.contains(&index)
            {
                possible = ordinal(&self.transcript.entries[index], self.flavor) == process
                    && replay_value_matches(&self.with_request_id(&entry.frame, &actual), &actual);
                if possible || self.flavor.starts_turn(&entry.frame) {
                    break;
                }
            }
        }
        if !possible {
            return Err(self.error(
                state,
                process,
                "outbound cannot match within this turn",
                actual,
            ));
        }
        state.held.push_back((process, actual));
        self.advance(state);
        Ok(())
    }
    fn advance(&self, state: &mut State) {
        loop {
            let mut progressed = false;
            loop {
                match self.transcript.entries.get(state.cursor) {
                    Some(ReplayEntry::RuntimeExit(_)) => {
                        state.cursor += 1;
                        progressed = true;
                        if let Some(host) = state.hosts.last_mut() {
                            host.sender = None;
                            host.exit.cancel();
                        }
                    }
                    Some(ReplayEntry::EmitInbound(entry)) => {
                        let process = ordinal(&self.transcript.entries[state.cursor], self.flavor);
                        let Some(host) = process
                            .checked_sub(1)
                            .and_then(|index| state.hosts.get_mut(index))
                        else {
                            break;
                        };
                        state.cursor += 1;
                        progressed = true;
                        let mut frame = entry.frame.clone();
                        let rebound = if self.flavor == JsonlFlavor::Pi {
                            frame["id"].as_str().map(str::to_owned)
                        } else if frame.get("method").is_none() {
                            number_key(&frame["id"])
                        } else {
                            None
                        };
                        if let Some(id) = rebound.and_then(|id| host.ids.get(&id)) {
                            frame["id"] = id.clone();
                        }
                        if let Some(sender) = &host.sender {
                            let _ = sender.send(frame);
                        }
                    }
                    _ => break,
                }
            }
            let mut consumed_write = None;
            'held: for (held_index, (process, actual)) in state.held.iter().enumerate() {
                for (index, entry) in self
                    .transcript
                    .entries
                    .iter()
                    .enumerate()
                    .skip(state.cursor)
                {
                    let ReplayEntry::ExpectOutbound(entry) = entry else {
                        break;
                    };
                    if state.consumed.contains(&index)
                        || ordinal(&self.transcript.entries[index], self.flavor) != *process
                    {
                        continue;
                    }
                    if !replay_value_matches(&self.with_request_id(&entry.frame, actual), actual) {
                        continue;
                    }
                    if let (Some(recorded), Some(_)) = (
                        self.flavor.request_id(&entry.frame),
                        self.flavor.request_id(actual),
                    ) {
                        state.hosts[*process - 1]
                            .ids
                            .insert(recorded, actual["id"].clone());
                    }
                    state.consumed.insert(index);
                    while state.consumed.contains(&state.cursor) {
                        state.cursor += 1;
                    }
                    consumed_write = Some(held_index);
                    break 'held;
                }
            }
            if let Some(index) = consumed_write {
                state.held.remove(index);
                progressed = true;
            }
            if !progressed {
                break;
            }
        }
    }
    pub fn close(&self, process: usize) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(host) = process
            .checked_sub(1)
            .and_then(|index| state.hosts.get_mut(index))
        {
            host.sender = None;
            host.exit.cancel();
        }
    }
    pub fn mint(&self, process: usize) -> JsonlReplayResult<String> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        process
            .checked_sub(1)
            .and_then(|index| state.hosts.get_mut(index))
            .and_then(|host| host.minted.pop_front())
            .ok_or_else(|| {
                Box::new(JsonlReplayError {
                    scenario: self.transcript.scenario.0.clone(),
                    cursor: state.cursor,
                    process,
                    detail: "minted more command IDs than recorded".into(),
                    expected: Value::Null,
                    actual: Value::Null,
                })
            })
    }
    pub fn assert_complete(&self) -> JsonlReplayResult<()> {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        if state.cursor == self.transcript.entries.len() && state.held.is_empty() {
            return Ok(());
        }
        Err(Box::new(JsonlReplayError {
            scenario: self.transcript.scenario.0.clone(),
            cursor: state.cursor,
            process: 0,
            detail: format!(
                "{} entries and {} held writes remain",
                self.transcript.entries.len() - state.cursor,
                state.held.len()
            ),
            expected: json!(self.transcript.entries.get(state.cursor)),
            actual: Value::Null,
        }))
    }
}

#[derive(Default)]
pub struct JsonLineSplitter {
    buffer: Vec<u8>,
    started: bool,
}
impl JsonLineSplitter {
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut lines = Vec::new();
        let mut start = 0;
        for (index, byte) in self.buffer.iter().enumerate() {
            if *byte == b'\n' {
                let line = &self.buffer[start..index];
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                let line = if !self.started {
                    self.started = true;
                    line.strip_prefix(b"\xef\xbb\xbf").unwrap_or(line)
                } else {
                    line
                };
                if !line.is_empty() {
                    lines.push(String::from_utf8_lossy(line).into_owned());
                }
                start = index + 1;
            }
        }
        self.buffer.drain(..start);
        lines
    }
}
pub fn record_label(frame: &Value, process: usize) -> Option<String> {
    let base = if frame["type"] == "response" {
        frame["command"]
            .as_str()
            .map(|command| format!("response:{command}"))
            .or_else(|| frame["type"].as_str().map(str::to_owned))
    } else {
        frame["type"].as_str().map(str::to_owned)
    }?;
    Some(if process == 1 {
        base
    } else {
        format!("{base}@p{process}")
    })
}

// Local shim: Pi's native RPC adapter must use the in-memory process controller.
#[async_trait::async_trait]
pub trait PiAdapterFactory: Send + Sync {
    async fn create(
        &self,
        controller: Arc<JsonlReplayController>,
        launch_args: String,
        clock: Arc<dyn crate::api::Clock>,
        ids: Arc<dyn crate::api::IdGenerator>,
    ) -> crate::provider::ProviderResult<Arc<dyn crate::provider::ProviderAdapter>>;
}
