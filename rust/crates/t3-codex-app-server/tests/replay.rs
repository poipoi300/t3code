#![allow(clippy::unwrap_used)]
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
};
#[path = "support/audit.rs"]
mod audit;
use audit::{Audit, numbers};
use t3_codex_app_server::{
    wire::{Message, Params, RequestId},
    *,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

struct Session {
    client: Client,
    events: Events,
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
    requests: HashMap<String, tokio::task::JoinHandle<Result<Value>>>,
    ids: HashMap<String, Value>,
    approvals: HashMap<RequestId, ServerRequest>,
}
impl Session {
    fn new() -> Self {
        let (local, peer) = tokio::io::duplex(1024 * 1024);
        let (read, write) = tokio::io::split(local);
        let (client, events) = Client::connect(read, write, Default::default());
        let (read, writer) = tokio::io::split(peer);
        Self {
            client,
            events,
            reader: BufReader::new(read),
            writer,
            requests: HashMap::new(),
            ids: HashMap::new(),
            approvals: HashMap::new(),
        }
    }
    async fn outbound(&mut self, expected: Value) {
        let message = Message::decode(&serde_json::to_vec(&expected).unwrap()).unwrap();
        let completion = match message {
            Message::Request { id, method, params } => {
                let client = self.client.clone();
                let pending =
                    tokio::spawn(async move { typed_request(&client, &method, &params).await });
                self.requests.insert(id.to_string(), pending);
                None
            }
            Message::Notification(n) => {
                let client = self.client.clone();
                Some(tokio::spawn(async move {
                    client.notify(&n.method, n.params).await
                }))
            }
            Message::Response { id, result } => {
                let request = self
                    .approvals
                    .remove(&id)
                    .expect("recorded reply must have a live request");
                Some(tokio::spawn(async move {
                    match result {
                        Ok(value) => respond(request, value).await,
                        Err(error) => request.reject(error).await,
                    }
                }))
            }
        };
        let mut line = String::new();
        assert!(self.reader.read_line(&mut line).await.unwrap() > 0);
        let mut actual: Value = serde_json::from_str(&line).unwrap();
        if expected.get("method").is_some()
            && let Some(id) = expected.get("id")
        {
            self.ids.insert(id_key(id), actual["id"].clone());
            actual["id"] = id.clone();
        }
        assert_eq!(actual, expected, "outbound wire frame");
        if let Some(completion) = completion {
            completion.await.unwrap().unwrap();
        }
    }
    async fn inbound(&mut self, mut expected: Value) {
        let original = expected.clone();
        let message = Message::decode(&serde_json::to_vec(&expected).unwrap()).unwrap();
        if let Message::Response { id, .. } = &message {
            expected["id"] = self
                .ids
                .get(&id.to_string())
                .expect("response id must match a request")
                .clone();
        }
        self.writer
            .write_all(format!("{expected}\n").as_bytes())
            .await
            .unwrap();
        match message {
            Message::Notification(n) => {
                match self
                    .events
                    .recv()
                    .await
                    .expect("notification stream ended")
                    .unwrap()
                {
                    Event::Notification { raw, decoded } => {
                        assert_eq!(raw, n, "notification sequence");
                        assert_eq!(
                            numbers(notification_payload(&decoded)),
                            numbers(param_value(&n.params)),
                            "typed notification round trip: {}",
                            n.method
                        );
                    }
                    event => panic!(
                        "Expected notification {}, got {event:?}",
                        original["method"]
                    ),
                }
            }
            Message::Request { id, method, params } => {
                match self
                    .events
                    .recv()
                    .await
                    .expect("request stream ended")
                    .unwrap()
                {
                    Event::Request(request) => {
                        assert_eq!(*request.id(), id);
                        assert_eq!(request.method(), method);
                        assert_eq!(
                            numbers(request_payload(request.params())),
                            numbers(param_value(&params)),
                            "typed server request round trip: {method}"
                        );
                        self.approvals.insert(id, *request);
                    }
                    event => panic!("Expected request, got {event:?}"),
                }
            }
            Message::Response { id, result } => {
                let actual = self
                    .requests
                    .remove(&id.to_string())
                    .expect("response waiter")
                    .await
                    .unwrap();
                match result {
                    Ok(value) => assert_eq!(actual.unwrap(), value),
                    Err(error) => assert!(
                        matches!(actual, Err(Error::Rpc { error: actual, .. }) if actual == error)
                    ),
                }
            }
        }
        self.events.acknowledge();
    }
    async fn finish(mut self) {
        self.writer.shutdown().await.unwrap();
        assert_eq!(self.client.await_termination().await, Error::Closed);
        for (_, pending) in self.requests {
            assert_eq!(pending.await.unwrap(), Err(Error::Closed));
        }
        for request in self.approvals.values() {
            assert!(request.is_resolved());
        }
        assert_eq!(
            self.events.recv().await.unwrap().unwrap_err(),
            Error::Closed
        );
        assert!(self.events.recv().await.is_none());
    }
}

fn id_key(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        _ => value.to_string(),
    }
}
fn decode<T: DeserializeOwned + Serialize + Audit>(method: &str, params: &Params) {
    let typed = params.decode::<T>(method).unwrap_or_else(|e| panic!("{e}"));
    assert_extras(&typed);
    if let Params::Present(expected) = params {
        assert_eq!(
            numbers(serde_json::to_value(typed).unwrap()),
            numbers(expected.clone()),
            "typed round trip: {method}"
        );
    }
}
async fn typed_request(client: &Client, method: &str, params: &Params) -> Result<Value> {
    macro_rules! send {
        ($t:ty) => {{
            let typed = params.decode::<$t>(method).unwrap();
            assert_extras(&typed);
            client.request(method, &typed).await
        }};
    }
    match method {
        "initialize" => send!(InitializeParams),
        "thread/start" => send!(ThreadStartParams),
        "thread/resume" => send!(ThreadResumeParams),
        "thread/fork" => send!(ThreadForkParams),
        "thread/read" => send!(ThreadReadParams),
        "thread/revert" => send!(ThreadRevertParams),
        "thread/turns/list" => send!(ThreadTurnsListParams),
        "thread/backgroundTerminals/terminate" => send!(BackgroundTerminalsTerminateParams),
        "turn/start" => send!(TurnStartParams),
        "turn/steer" => send!(TurnSteerParams),
        "turn/interrupt" => send!(TurnInterruptParams),
        "config/read" => send!(ConfigReadParams),
        _ => panic!("Add the recorded method to the typed replay boundary: {method}"),
    }
}

#[tokio::test]
async fn replays_every_recorded_typescript_codex_transcript() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../apps/server/src/orchestration-v2/testkit/fixtures");
    let mut paths: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path().join("codex_transcript.ndjson"))
        .filter(|path| path.is_file())
        .collect();
    paths.sort();
    assert!(paths.len() >= 28, "recorded corpus must be available");
    let mut inbound_count = 0;
    let mut outbound_count = 0;
    for path in paths {
        eprintln!(
            "Replay {}",
            path.parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
        );
        let text = std::fs::read_to_string(&path).unwrap();
        let mut session = None;
        for (line_number, line) in text.lines().enumerate() {
            let row: Value = serde_json::from_str(line).unwrap();
            match row["type"].as_str().unwrap() {
                "transcript_start" => assert_eq!(row["protocol"], "codex.app-server"),
                "expect_outbound" => {
                    outbound_count += 1;
                    session
                        .get_or_insert_with(Session::new)
                        .outbound(row["frame"].clone())
                        .await;
                }
                "emit_inbound" => {
                    inbound_count += 1;
                    session
                        .as_mut()
                        .expect("session must be open")
                        .inbound(row["frame"].clone())
                        .await;
                }
                "runtime_exit" => session.take().expect("session must be open").finish().await,
                kind => panic!(
                    "{}:{}: unknown record {kind}",
                    path.display(),
                    line_number + 1
                ),
            }
        }
        if let Some(session) = session {
            session.finish().await;
        }
    }
    assert!(inbound_count >= 3553);
    assert!(outbound_count >= 216);
    eprintln!("Replayed {inbound_count} inbound and {outbound_count} outbound frames");
}

#[test]
fn typed_recorded_responses_preserve_fields_and_check_extras() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../apps/server/src/orchestration-v2/testkit/fixtures");
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path().join("codex_transcript.ndjson");
        if !path.is_file() {
            continue;
        }
        let mut methods = HashMap::new();
        for line in std::fs::read_to_string(path).unwrap().lines() {
            let row: Value = serde_json::from_str(line).unwrap();
            let f = &row["frame"];
            if row["type"] == "expect_outbound"
                && f.get("id").is_some()
                && f.get("method").is_some()
            {
                methods.insert(id_key(&f["id"]), f["method"].as_str().unwrap().to_owned());
            }
            if row["type"] == "runtime_exit" {
                methods.clear();
            }
            if row["type"] != "emit_inbound" || f.get("result").is_none() {
                continue;
            }
            let Some(method) = methods.remove(&id_key(&f["id"])) else {
                continue;
            };
            let params = Params::Present(f["result"].clone());
            match method.as_str() {
                "initialize" => decode::<InitializeResponse>(&method, &params),
                "thread/start" | "thread/resume" | "thread/fork" | "thread/read"
                | "thread/revert" => decode::<ThreadResponse>(&method, &params),
                "turn/start" => decode::<TurnResponse>(&method, &params),
                "turn/steer" => decode::<TurnSteerResponse>(&method, &params),
                "config/read" => decode::<ConfigReadResponse>(&method, &params),
                "thread/turns/list" => decode::<ThreadTurnsListResponse>(&method, &params),
                "thread/backgroundTerminals/terminate" => {
                    decode::<BackgroundTerminalsTerminateResponse>(&method, &params)
                }
                "turn/interrupt" => decode::<Empty>(&method, &params),
                _ => panic!("unvalidated response: {method}: {}", json!(f["result"])),
            }
        }
    }
}

async fn respond(request: ServerRequest, value: Value) -> Result<()> {
    macro_rules! respond {
        ($request:expr) => {
            $request
                .respond(serde_json::from_value(value).unwrap())
                .await
        };
    }
    match request.into_typed() {
        TypedServerRequest::CommandApproval(r) => respond!(r),
        TypedServerRequest::FileChangeApproval(r) => respond!(r),
        TypedServerRequest::PermissionsApproval(r) => respond!(r),
        TypedServerRequest::UserInput(r) => respond!(r),
        TypedServerRequest::Elicitation(r) => respond!(r),
        TypedServerRequest::LegacyExecApproval(r) => respond!(r),
        TypedServerRequest::LegacyPatchApproval(r) => respond!(r),
        TypedServerRequest::Unknown(r) => r.respond(value).await,
    }
}

fn param_value(params: &Params) -> Value {
    match params {
        Params::Present(v) => v.clone(),
        Params::Missing => Value::Null,
    }
}
fn assert_extras<T: Audit>(typed: &T) {
    let mut found = BTreeSet::new();
    typed.audit(&mut found);
    // Every exception must name the exact type/field and explain its wire provenance.
    let allowed: &[(&str, &str)] = &[
        (
            "Thread.canAcceptDirectInput",
            "0.156.1 recordings advertise direct input; absent from the pinned Thread schema.",
        ),
        (
            "Thread.daybreakEnabled",
            "0.156.1 recording extension; absent from the pinned Thread schema.",
        ),
        (
            "Thread.environments",
            "Recorded multi-environment extension; the generator exports ThreadEnvironment separately but Thread has no environments field.",
        ),
        (
            "Thread.extra",
            "Recorded native extension bag; the generator exports ThreadExtra separately but Thread has no extra field.",
        ),
        (
            "ThreadResponse.activePermissionProfile",
            "Recorded sandbox profile provenance; not declared on the pinned thread response envelopes.",
        ),
        (
            "ThreadResponse.multiAgentMode",
            "Recorded delegation policy extension; not declared on the pinned thread response envelopes.",
        ),
        (
            "ThreadResponse.runtimeWorkspaceRoots",
            "Recorded workspace roots; not declared on the pinned thread response envelopes.",
        ),
        (
            "ThreadResponse.permissionProfile",
            "Older captured response profile field; absent from the pinned response envelopes.",
        ),
        (
            "CommandApprovalParams.availableDecisions",
            "Recorded approval choices extension; absent from CommandExecutionRequestApprovalParams in the pinned schema.",
        ),
        (
            "ThreadResponse.initialTurnsPage",
            "Recorded initial history page extension; not declared on the pinned response envelopes.",
        ),
        (
            "ThreadSettings.multiAgentMode",
            "Recorded delegation policy extension; not declared on the pinned ThreadSettings schema.",
        ),
    ];
    found.retain(|key| {
        !allowed.iter().any(|(field, reason)| {
            assert!(!reason.is_empty());
            key == field
        })
    });
    assert!(found.is_empty(), "Unrecognized known fields: {found:#?}");
}
fn typed_value<T: Serialize + Audit>(value: &T) -> Value {
    assert_extras(value);
    serde_json::to_value(value).unwrap()
}
fn notification_payload(n: &ServerNotification) -> Value {
    match n {
        ServerNotification::AgentMessageDelta(v)
        | ServerNotification::CommandOutputDelta(v)
        | ServerNotification::ReasoningSummaryDelta(v)
        | ServerNotification::ReasoningTextDelta(v)
        | ServerNotification::PlanDelta(v) => typed_value(v),
        ServerNotification::ItemStarted(v) | ServerNotification::ItemCompleted(v) => typed_value(v),
        ServerNotification::TurnStarted(v) | ServerNotification::TurnCompleted(v) => typed_value(v),
        ServerNotification::TurnPlanUpdated(v) => typed_value(v),
        ServerNotification::ThreadClosed(v) | ServerNotification::GoalCleared(v) => typed_value(v),
        ServerNotification::GoalUpdated(v) => typed_value(v),
        ServerNotification::TokenUsageUpdated(v) => typed_value(v),
        ServerNotification::SettingsUpdated(v) => typed_value(v),
        ServerNotification::ModelRerouted(v) => typed_value(v),
        ServerNotification::McpStartupStatusUpdated(v) => typed_value(v),
        ServerNotification::HookCompleted(v) => typed_value(v),
        ServerNotification::Error(v) => typed_value(v),
        ServerNotification::RateLimitsUpdated(v) => typed_value(v),
        ServerNotification::RequestResolved(v) => typed_value(v),
        ServerNotification::Unknown(v) => param_value(&v.params),
    }
}
fn request_payload(n: &ServerRequestParams) -> Value {
    match n {
        ServerRequestParams::CommandApproval(v) => typed_value(v),
        ServerRequestParams::FileChangeApproval(v) => typed_value(v),
        ServerRequestParams::PermissionsApproval(v) => typed_value(v),
        ServerRequestParams::UserInput(v) => typed_value(v),
        ServerRequestParams::Elicitation(v) => typed_value(v),
        ServerRequestParams::LegacyExecApproval(v) => typed_value(v),
        ServerRequestParams::LegacyPatchApproval(v) => typed_value(v),
        ServerRequestParams::Unknown { params, .. } => param_value(params),
    }
}
