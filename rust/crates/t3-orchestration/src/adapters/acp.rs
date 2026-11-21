#![cfg(feature = "acp-adapter")]

use crate::{
    api::{Clock, IdGenerator, IdKind},
    provider::*,
    testing::replay::*,
};
use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use t3_acp::{Client, ClientHandler, ClientOptions, ClientRequest, RequestContext, RpcError};
use t3_contracts::{
    base_schemas::*,
    orchestration_v2::*,
    orchestration_v2_id_table::IdOrIndex,
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
    provider_policy::{ProviderApprovalDecision, ProviderApprovalOption, RuntimeMode},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot},
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}
fn protocol(detail: impl Into<String>) -> ProviderError {
    ProviderError::Protocol {
        reason: None,
        detail: detail.into(),
        payload: None,
    }
}
fn native_error(error: impl std::fmt::Display) -> ProviderError {
    protocol(error.to_string())
}
fn decode<T: DeserializeOwned>(value: Value) -> ProviderResult<T> {
    serde_json::from_value(value).map_err(native_error)
}
fn encode<T: Serialize>(value: &T) -> ProviderResult<Value> {
    serde_json::to_value(value).map_err(native_error)
}
fn expanded<T>(value: &IdOrIndex<T>) -> ProviderResult<&T> {
    match value {
        IdOrIndex::Id(id) => Ok(id),
        _ => Err(protocol("ACP requires expanded IDs")),
    }
}

pub struct AcpTransport {
    pub reader: Box<dyn AsyncRead + Unpin + Send>,
    pub writer: Box<dyn AsyncWrite + Unpin + Send>,
    pub cancellation: CancellationToken,
    pub completion: JoinHandle<ProviderResult<()>>,
}

#[async_trait]
pub trait AcpTransportFactory: Send + Sync {
    async fn connect(
        &self,
        input: &OpenSession,
        cancellation: CancellationToken,
    ) -> ProviderResult<AcpTransport>;
}

pub struct ProcessTransportFactory {
    pub options: t3_process::ProcessOptions,
}
#[async_trait]
impl AcpTransportFactory for ProcessTransportFactory {
    async fn connect(
        &self,
        input: &OpenSession,
        cancellation: CancellationToken,
    ) -> ProviderResult<AcpTransport> {
        let mut options = self.options.clone();
        options.cwd = input.runtime_policy.cwd.clone().or(options.cwd);
        options.cancellation = cancellation.clone();
        let mut process = tokio::task::spawn_blocking(move || t3_process::spawn(&options))
            .await
            .map_err(native_error)?
            .map_err(native_error)?;
        let reader = process
            .stdout
            .take()
            .ok_or_else(|| protocol("ACP stdout missing"))?;
        let writer = process
            .stdin
            .take()
            .ok_or_else(|| protocol("ACP stdin missing"))?;
        let mut stderr = process.stderr.take();
        let stopped = cancellation.clone();
        let completion = tokio::spawn(async move {
            let drain = async move {
                let mut buffer = [0; 8192];
                if let Some(stderr) = &mut stderr {
                    while stderr.read(&mut buffer).await.map_err(native_error)? != 0 {}
                }
                Ok::<_, ProviderError>(())
            };
            tokio::pin!(drain);
            let drained = tokio::select! {
                _ = stopped.cancelled() => Ok(()),
                result = &mut drain => { stopped.cancelled().await; result }
            };
            let terminated = process
                .terminate(Duration::from_secs(1))
                .await
                .map_err(native_error);
            drained?;
            terminated?;
            Ok(())
        });
        Ok(AcpTransport {
            reader: Box::new(reader),
            writer: Box::new(writer),
            cancellation,
            completion,
        })
    }
}

pub struct DevReplayTransportFactory {
    pub ids: Arc<dyn IdGenerator>,
    pub streaming_delays: bool,
}

pub struct StrictReplayTransportFactory {
    pub transcript: ReplayTranscript,
    pub gate: ReplayGate,
    pub status: tokio::sync::watch::Sender<ReplayStatus>,
}
#[async_trait]
impl AcpTransportFactory for StrictReplayTransportFactory {
    async fn connect(
        &self,
        input: &OpenSession,
        cancellation: CancellationToken,
    ) -> ProviderResult<AcpTransport> {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (reader, writer) = tokio::io::split(client);
        let workspace = input
            .runtime_policy
            .cwd
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".".into());
        let previous = self.status.borrow().clone();
        let mut cursor = ReplayCursor::new(self.transcript.clone(), workspace, Some(&previous));
        let status = self.status.clone();
        let gate = self.gate.clone();
        let stop = cancellation.clone();
        let completion = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server);
            let mut lines = BufReader::new(read).lines();
            let result = async {
                loop {
                    let messages = cursor.flush_inbound().map_err(native_error)?;
                    status.send_replace(cursor.status.clone());
                    for (label, message) in messages {
                        gate.before_emit(label.as_deref(), &stop).await;
                        if stop.is_cancelled() { return Ok(()); }
                        let mut bytes = serde_json::to_vec(&message).map_err(native_error)?;
                        bytes.push(b'\n');
                        tokio::select! { _ = stop.cancelled() => return Ok(()), result = write.write_all(&bytes) => result.map_err(native_error)? }
                    }
                    let line = tokio::select! { _ = stop.cancelled() => return Ok(()), result = lines.next_line() => result.map_err(native_error)? };
                    let Some(line) = line else { return cursor.close_input().map_err(native_error); };
                    if line.trim().is_empty() { continue; }
                    let message: Value = serde_json::from_str(&line).map_err(native_error)?;
                    cursor.accept(&message).map_err(native_error)?;
                }
            }.await;
            status.send_replace(cursor.status);
            result
        });
        Ok(AcpTransport {
            reader: Box::new(reader),
            writer: Box::new(writer),
            cancellation,
            completion,
        })
    }
}
#[derive(Clone)]
struct DevSession {
    cwd: String,
    model: String,
    prompts: usize,
    active: Option<CancellationToken>,
}

async fn send_wire(
    sender: &mpsc::Sender<Value>,
    value: Value,
    cancellation: &CancellationToken,
) -> ProviderResult<()> {
    tokio::select! { _ = cancellation.cancelled() => Err(ProviderError::Cancelled), result = sender.send(value) => result.map_err(|_| ProviderError::SessionStopped) }
}
async fn replay_delay(ms: u64, enabled: bool, cancellation: &CancellationToken) {
    if enabled {
        tokio::select! { _ = cancellation.cancelled() => {}, _ = tokio::time::sleep(Duration::from_millis(ms.min(2000))) => {} }
    }
}

struct DevPlayer {
    sender: mpsc::Sender<Value>,
    pending: Arc<Mutex<BTreeMap<String, oneshot::Sender<Value>>>>,
    ids: Arc<dyn IdGenerator>,
    delays: bool,
}
impl DevPlayer {
    async fn play(
        &self,
        session_id: &str,
        session: &DevSession,
        turn: Option<&ReplayTurn>,
        cancellation: &CancellationToken,
    ) -> ProviderResult<bool> {
        let Some(turn) = turn else {
            return Ok(false);
        };
        let turn_key = self.ids.next(IdKind::ProviderTurn);
        for recorded in &turn.frames {
            let frame = materialize(
                &recorded.frame,
                Some(session_id),
                &session.cwd,
                Some(&turn_key),
            );
            if frame["kind"] == "request" {
                replay_delay(recorded.after_ms.unwrap_or(250), self.delays, cancellation).await;
                if cancellation.is_cancelled() {
                    return Ok(true);
                }
                let id = self.ids.next(IdKind::RuntimeRequest);
                let (answer, receive) = oneshot::channel();
                lock(&self.pending).insert(id.clone(), answer);
                send_wire(&self.sender, json!({"jsonrpc":"2.0","id":id,"method":frame["method"],"params":frame["params"]}), cancellation).await?;
                let answer = tokio::select! { _ = cancellation.cancelled() => Value::Null, answer = receive => answer.unwrap_or(Value::Null) };
                lock(&self.pending).remove(&id);
                if cancellation.is_cancelled() || answer["outcome"]["outcome"] == "cancelled" {
                    return Ok(true);
                }
                if let Some(tool_call) = permission_rejected_tool_call(&frame, &answer) {
                    send_wire(&self.sender, json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session_id,"update":{"sessionUpdate":"tool_call_update","toolCallId":tool_call,"status":"failed"}}}), cancellation).await?;
                    return Ok(false);
                }
                continue;
            }
            let text = matches!(
                frame["params"]["update"]["sessionUpdate"].as_str(),
                Some("agent_message_chunk" | "agent_thought_chunk")
            )
            .then(|| &frame["params"]["update"]["content"])
            .filter(|content| content["type"] == "text")
            .and_then(|content| content["text"].as_str());
            let chunks = text
                .map(stream_chunks)
                .unwrap_or_else(|| vec![String::new()]);
            for (index, chunk) in chunks.iter().enumerate() {
                replay_delay(
                    if index == 0 {
                        recorded
                            .after_ms
                            .unwrap_or(if text.is_some() { 30 } else { 250 })
                    } else {
                        30
                    },
                    self.delays,
                    cancellation,
                )
                .await;
                if cancellation.is_cancelled() {
                    return Ok(true);
                }
                let mut params = frame["params"].clone();
                if text.is_some() {
                    params["update"]["content"]["text"] = json!(chunk);
                }
                send_wire(
                    &self.sender,
                    json!({"jsonrpc":"2.0","method":frame["method"],"params":params}),
                    cancellation,
                )
                .await?;
            }
        }
        Ok(cancellation.is_cancelled())
    }
}

#[async_trait]
impl AcpTransportFactory for DevReplayTransportFactory {
    async fn connect(
        &self,
        _: &OpenSession,
        cancellation: CancellationToken,
    ) -> ProviderResult<AcpTransport> {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (reader, writer) = tokio::io::split(client);
        let ids = self.ids.clone();
        let delays = self.streaming_delays;
        let stopped = cancellation.clone();
        let completion =
            tokio::spawn(async move { serve_dev_replay(server, ids, delays, stopped).await });
        Ok(AcpTransport {
            reader: Box::new(reader),
            writer: Box::new(writer),
            cancellation,
            completion,
        })
    }
}

async fn serve_dev_replay(
    stream: tokio::io::DuplexStream,
    ids: Arc<dyn IdGenerator>,
    delays: bool,
    cancellation: CancellationToken,
) -> ProviderResult<()> {
    let (read, mut write) = tokio::io::split(stream);
    let mut lines = BufReader::new(read).lines();
    let (sender, mut outgoing) = mpsc::channel::<Value>(64);
    let writer_stop = cancellation.clone();
    let writer = tokio::spawn(async move {
        loop {
            let message = tokio::select! { _ = writer_stop.cancelled() => break, message = outgoing.recv() => match message { Some(message) => message, None => break } };
            let mut bytes = serde_json::to_vec(&message).map_err(native_error)?;
            bytes.push(b'\n');
            tokio::select! { _ = writer_stop.cancelled() => break, result = write.write_all(&bytes) => result.map_err(native_error)? }
        }
        Ok::<_, ProviderError>(())
    });
    let pending = Arc::new(Mutex::new(BTreeMap::<String, oneshot::Sender<Value>>::new()));
    let sessions = Arc::new(Mutex::new(BTreeMap::<String, DevSession>::new()));
    let mut prompts = JoinSet::new();
    let result = async {
        loop {
            let line = tokio::select! {
                _ = cancellation.cancelled() => break,
                result = prompts.join_next(), if !prompts.is_empty() => { result.ok_or_else(|| protocol("replay prompt task missing"))?.map_err(native_error)??; continue; }
                line = lines.next_line() => line.map_err(native_error)?,
            };
            let Some(line) = line else { break; };
            if line.trim().is_empty() { continue; }
            let Ok(message) = serde_json::from_str::<Value>(&line) else { continue; };
            let Some(method) = message["method"].as_str() else {
                if let Some(id) = message["id"].as_str() && let Some(answer) = lock(&pending).remove(id) { let _ = answer.send(message["result"].clone()); }
                continue;
            };
            let params = &message["params"];
            let session_id = params["sessionId"].as_str().unwrap_or_default();
            if message.get("id").is_none_or(Value::is_null) {
                if method == "session/cancel" && let Some(active) = lock(&sessions).get(session_id).and_then(|s| s.active.as_ref()) { active.cancel(); }
                continue;
            }
            let response = match method {
                "initialize" => json!({"protocolVersion":1,"agentCapabilities":{"loadSession":true,"promptCapabilities":{"image":false,"audio":false,"embeddedContext":true}},"authMethods":[],"agentInfo":{"name":"t3-dev-replay","version":"1"}}),
                "session/new" | "session/load" => {
                    let session_id = if method == "session/load" { session_id.to_owned() } else { ids.next(IdKind::ProviderThread) };
                    lock(&sessions).insert(session_id.clone(), DevSession { cwd: params["cwd"].as_str().unwrap_or(".").into(), model: "showcase".into(), prompts: 0, active: None });
                    let mut response = json!({"models":{"currentModelId":"showcase","availableModels":DEV_SCENARIOS.iter().map(|(id,name,_)|json!({"modelId":id,"name":name})).collect::<Vec<_>>()}});
                    if method == "session/new" { response["sessionId"] = json!(session_id); }
                    response
                }
                "session/set_model" => {
                    let model = params["modelId"].as_str().unwrap_or_default();
                    if !DEV_SCENARIOS.iter().any(|s| s.0 == model) {
                        send_wire(&sender, json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32602,"message":format!("Unknown replay scenario {model}")}}), &cancellation).await?;
                        continue;
                    }
                    if let Some(session) = lock(&sessions).get_mut(session_id) && session.model != model { session.model = model.into(); session.prompts = 0; }
                    json!({})
                }
                "session/prompt" => {
                    let (session, turn) = {
                        let mut sessions = lock(&sessions);
                        let session = sessions.get_mut(session_id).ok_or_else(|| protocol("unknown replay session"))?;
                        if session.active.is_some() { return Err(protocol("replay prompt already active")); }
                        let turns = parse_replay_turns(DEV_SCENARIOS.iter().find(|s| s.0 == session.model).unwrap_or(&DEV_SCENARIOS[0]).2).map_err(native_error)?;
                        let turn = turns.get(session.prompts % turns.len().max(1)).cloned();
                        session.prompts += 1;
                        session.active = Some(cancellation.child_token());
                        (session.clone(), turn)
                    };
                    let player = DevPlayer { sender: sender.clone(), pending: pending.clone(), ids: ids.clone(), delays };
                    let session_id = session_id.to_owned();
                    let sessions = sessions.clone();
                    let id = message["id"].clone();
                    let stop = cancellation.clone();
                    prompts.spawn(async move {
                        let token = session.active.as_ref().ok_or_else(|| protocol("replay cancellation missing"))?;
                        let result = player.play(&session_id, &session, turn.as_ref(), token).await;
                        if let Some(session) = lock(&sessions).get_mut(&session_id) { session.active = None; }
                        let cancelled = result?;
                        let reason = if cancelled { "cancelled" } else { turn.as_ref().map_or("end_turn", |t| t.stop_reason.as_str()) };
                        send_wire(&player.sender, json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":reason}}), &stop).await
                    });
                    continue;
                }
                "session/close" => { if let Some(session) = lock(&sessions).remove(session_id) && let Some(active) = session.active { active.cancel(); } json!({}) }
                "authenticate" | "session/set_mode" => json!({}),
                _ => {
                    send_wire(&sender, json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32601,"message":format!("Method not found: {method}")}}), &cancellation).await?;
                    continue;
                }
            };
            send_wire(&sender, json!({"jsonrpc":"2.0","id":message["id"],"result":response}), &cancellation).await?;
        }
        Ok(())
    }.await;
    cancellation.cancel();
    lock(&pending).clear();
    while prompts.join_next().await.is_some() {}
    writer.await.map_err(native_error)??;
    result
}

#[derive(Clone)]
pub struct AcpEffectContext {
    pub request: RequestContext,
    pub policy: RuntimePolicy,
    pub tool_kind: Option<String>,
    pub turn_key: Option<String>,
    pub execute_approved: bool,
}

#[async_trait]
pub trait AcpClientEffects: Send + Sync {
    async fn request(
        &self,
        request: ClientRequest,
        context: AcpEffectContext,
    ) -> Result<Value, RpcError>;
}
pub struct RejectClientEffects;
#[async_trait]
impl AcpClientEffects for RejectClientEffects {
    async fn request(
        &self,
        _: ClientRequest,
        context: AcpEffectContext,
    ) -> Result<Value, RpcError> {
        Err(RpcError {
            code: -32000,
            message: format!("Client operation {} is not enabled", context.request.method),
            data: None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDisposition {
    Allow,
    Ask,
    Deny,
}

pub fn approval_options(driver: &str, request: &Value) -> Option<Vec<ProviderApprovalOption>> {
    let native = request["options"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let has = |kind: &str| {
        native.iter().any(|option| {
            option["kind"] == kind
                && (driver != "kiro"
                    || option["optionId"]
                        .as_str()
                        .is_some_and(|id| !t3_contracts::wire::trim_js(id).is_empty()))
        })
    };
    let option = |decision, label: &str| ProviderApprovalOption {
        decision,
        label: label.into(),
        warning: None,
    };
    use ProviderApprovalDecision::*;
    match driver {
        "kiro" => {
            let mut choices = Vec::new();
            if has("allow_once") {
                choices.push(option(Accept, "Allow once"));
            } else if has("allow_always") {
                choices.push(option(AcceptForSession, "Always allow"));
            }
            if has("reject_once") {
                choices.push(option(Decline, "Deny"));
            }
            choices.push(option(Cancel, "Cancel"));
            Some(choices)
        }
        "grok" => {
            let mut choices = vec![option(Cancel, "Cancel")];
            if has("reject_once") {
                choices.push(option(Decline, "Decline"));
            }
            if native.iter().any(|option| {
                option["kind"] == "allow_always"
                    && option["optionId"]
                        .as_str()
                        .is_some_and(|id| t3_contracts::wire::trim_js(id) == "allow-edits-session")
            }) {
                choices.push(option(AcceptForSession, "Allow all edits this session"));
            }
            if has("allow_once") {
                choices.push(option(Accept, "Approve"));
            }
            Some(choices)
        }
        _ => None,
    }
}

fn permission_policy(policy: &RuntimePolicy, modes: Option<&[RuntimeMode]>) -> RuntimePolicy {
    let mut policy = policy.clone();
    if !modes.is_some_and(|modes| modes.contains(&policy.runtime_mode)) {
        policy.runtime_mode = RuntimeMode::ApprovalRequired;
        policy.approval_policy = None;
    }
    policy
}

pub fn permission_disposition_for_driver(
    driver: &str,
    policy: &RuntimePolicy,
    request: &Value,
    modes: Option<&[RuntimeMode]>,
) -> PermissionDisposition {
    if !modes.is_some_and(|modes| modes.contains(&policy.runtime_mode))
        && permission_disposition(
            policy,
            request["toolCall"]["kind"].as_str().unwrap_or("other"),
            request["toolCall"].get("locations"),
        ) == PermissionDisposition::Deny
    {
        return PermissionDisposition::Deny;
    }
    let mut policy = permission_policy(policy, modes);
    let automatic = policy.runtime_mode == RuntimeMode::Auto
        && policy.approval_policy.is_none()
        && policy.sandbox_policy.is_none();
    if matches!(driver, "grok" | "fx") && automatic {
        return PermissionDisposition::Ask;
    }
    if driver == "fx" {
        policy.runtime_mode = RuntimeMode::ApprovalRequired;
    }
    let disposition = permission_disposition(
        &policy,
        request["toolCall"]["kind"].as_str().unwrap_or("other"),
        request["toolCall"].get("locations"),
    );
    if driver == "kiro"
        && disposition == PermissionDisposition::Allow
        && !request["options"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|option| {
                option["kind"] == "allow_once"
                    && option["optionId"]
                        .as_str()
                        .is_some_and(|id| !t3_contracts::wire::trim_js(id).is_empty())
            })
    {
        PermissionDisposition::Ask
    } else {
        disposition
    }
}

pub fn mcp_tool_approval_disposition(
    policy: &RuntimePolicy,
    request: &Value,
    native_id: Option<&str>,
) -> Option<PermissionDisposition> {
    if request["mode"] != "form"
        || (request["_meta"]["codex_approval_kind"] != "mcp_tool_call"
            && !native_id.is_some_and(|id| id.starts_with("mcp_tool_call_approval_")))
    {
        return None;
    }
    Some(
        if policy.approval_policy.as_ref().map_or(
            policy.runtime_mode == RuntimeMode::ApprovalRequired,
            |approval| approval != "never",
        ) {
            PermissionDisposition::Ask
        } else {
            PermissionDisposition::Allow
        },
    )
}

pub fn permission_disposition(
    policy: &RuntimePolicy,
    kind: &str,
    locations: Option<&Value>,
) -> PermissionDisposition {
    use PermissionDisposition::*;
    let sandbox_type = policy.sandbox_policy.as_ref().and_then(|s| s.get("type"));
    let sandbox = sandbox_type.and_then(Value::as_str);
    let valid_sandbox_type = sandbox_type.is_none() || sandbox.is_some();
    if matches!(kind, "read" | "search" | "think") {
        return if valid_sandbox_type
            && matches!(
                sandbox,
                None | Some("readOnly" | "workspaceWrite" | "dangerFullAccess" | "externalSandbox")
            ) {
            Allow
        } else {
            Deny
        };
    }
    let requires_approval = policy
        .approval_policy
        .as_ref()
        .map_or(policy.runtime_mode == RuntimeMode::ApprovalRequired, |p| {
            p != "never"
        });
    if requires_approval {
        return Ask;
    }
    if !valid_sandbox_type {
        return Deny;
    }
    match sandbox {
        Some("readOnly") => Deny,
        Some("workspaceWrite") => {
            if !matches!(kind, "edit" | "delete" | "move") {
                return Deny;
            }
            let cwd = policy
                .cwd
                .as_ref()
                .filter(|path| !path.as_os_str().is_empty());
            let mut roots: Vec<_> = cwd
                .and_then(|path| canonical_containment_path(path))
                .into_iter()
                .collect();
            if let Some(writable) = policy
                .sandbox_policy
                .as_ref()
                .and_then(|s| s["writableRoots"].as_array())
            {
                roots.extend(
                    writable
                        .iter()
                        .filter_map(Value::as_str)
                        .filter_map(|path| resolve_permission_path(path, cwd.map(PathBuf::as_path)))
                        .filter_map(|path| canonical_containment_path(&path)),
                );
            }
            let Some(locations) = locations
                .and_then(Value::as_array)
                .filter(|l| !l.is_empty())
            else {
                return Deny;
            };
            if locations.iter().all(|location| {
                location["path"]
                    .as_str()
                    .and_then(|path| resolve_permission_path(path, cwd.map(PathBuf::as_path)))
                    .and_then(|path| canonical_containment_path(&path))
                    .is_some_and(|path| roots.iter().any(|root| path.starts_with(root)))
            }) {
                Allow
            } else {
                Deny
            }
        }
        Some("dangerFullAccess" | "externalSandbox") => Allow,
        None if policy.runtime_mode == RuntimeMode::ApprovalRequired => Deny,
        None if policy.runtime_mode == RuntimeMode::AutoAcceptEdits
            && policy.approval_policy.is_none()
            && !matches!(kind, "edit" | "delete" | "move") =>
        {
            Ask
        }
        None => Allow,
        _ => Deny,
    }
}

fn resolve_permission_path(path: &str, cwd: Option<&std::path::Path>) -> Option<PathBuf> {
    let path = std::path::Path::new(path.trim());
    if path.as_os_str().is_empty() {
        return None;
    }
    if path.is_absolute() {
        Some(path.to_owned())
    } else {
        cwd.map(|cwd| cwd.join(path))
    }
}

fn canonical_containment_path(path: &std::path::Path) -> Option<PathBuf> {
    let mut candidate = path.to_path_buf();
    let mut missing = Vec::new();
    loop {
        match std::fs::canonicalize(&candidate) {
            Ok(mut resolved) => {
                for part in missing.iter().rev() {
                    resolved.push(part);
                }
                return Some(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if std::fs::symlink_metadata(&candidate).is_ok() {
                    return None;
                }
                missing.push(candidate.file_name()?.to_os_string());
                candidate = candidate.parent()?.to_path_buf();
            }
            Err(_) => return None,
        }
    }
}

fn encoded_part(value: &str) -> String {
    let mut result = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            result.push(char::from(byte));
        } else {
            result.push('%');
            result.push(char::from(b"0123456789ABCDEF"[(byte >> 4) as usize]));
            result.push(char::from(b"0123456789ABCDEF"[(byte & 15) as usize]));
        }
    }
    result
}
fn derived(prefix: &str, parts: &[&str]) -> String {
    format!(
        "{prefix}:{}",
        parts
            .iter()
            .map(|p| encoded_part(p))
            .collect::<Vec<_>>()
            .join(":")
    )
}

type Subscribers = Vec<(
    mpsc::Sender<ProviderResult<ProviderEvent>>,
    CancellationToken,
)>;
struct PendingRequest {
    reply: Option<oneshot::Sender<RuntimeResponse>>,
    flushed: tokio::sync::watch::Sender<bool>,
    request: OrchestrationV2RuntimeRequest,
}
struct TurnContext {
    input: TurnInput,
    provider_turn: OrchestrationV2ProviderTurn,
    cancellation: CancellationToken,
    /// Open while the turn runs; closed when it settles, which withdraws its pending consent.
    window: CancellationToken,
    tools: BTreeMap<String, Value>,
    ordinals: BTreeMap<String, u64>,
    next_ordinal: u64,
}
struct MappingState {
    threads: BTreeMap<String, OrchestrationV2ProviderThread>,
    selected_models: BTreeMap<String, String>,
    contexts: BTreeMap<String, TurnContext>,
    active: Option<String>,
    messages: Vec<OrchestrationV2ConversationMessage>,
    turns: Vec<OrchestrationV2ProviderTurn>,
    requests: BTreeMap<String, PendingRequest>,
    response_acks: BTreeMap<String, tokio::sync::watch::Sender<bool>>,
    execute_session: bool,
    execute_turn: Option<String>,
    failed: bool,
    /// Thread setup in progress (`session/new` or `session/load`): sign-in consent is admitted
    /// until the token closes with the setup.
    setting_up: Option<CancellationToken>,
    /// The native session being loaded and the app thread it belongs to; the agent replays its
    /// history meanwhile.
    loading: Option<(String, ThreadId)>,
    history: BTreeMap<String, LoadedHistory>,
}

/// `initialize` and `session/new` are bounded like `t3-acp`'s `session/load` (90 s), so an agent
/// that never answers cannot hold a turn, its session's operation lock and the startup guard.
/// TypeScript bounds only `session/load`.
const SETUP_TIMEOUT: Duration = Duration::from_secs(90);

/// Bounds on a replayed history: it is agent content, kept in memory and imported as messages.
const HISTORY_MAX_MESSAGES: usize = 2_000;
const HISTORY_MAX_MESSAGE_BYTES: usize = 1 << 20;
const HISTORY_MAX_BYTES: usize = 16 << 20;
/// TypeScript `MAX_ACP_CONTENT_TEXT_LENGTH`, in UTF-16 units.
const CONTENT_TEXT_LIMIT: usize = 65_536;

/// TypeScript `appendLoadedHistory`'s snapshot state for one native session.
#[derive(Default)]
struct LoadedHistory {
    order: Vec<String>,
    messages: BTreeMap<String, OrchestrationV2ConversationMessage>,
    reasoning: Vec<MessageId>,
    role: Option<&'static str>,
    message_id: Option<String>,
    index: u64,
    bytes: usize,
    truncated: bool,
}

struct Shared {
    driver: ProviderDriverKind,
    instance: ProviderInstanceId,
    session_id: ProviderSessionId,
    item_identity_version: Mutex<u8>,
    policy: Mutex<RuntimePolicy>,
    supported_runtime_modes: Option<Vec<RuntimeMode>>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    state: Mutex<MappingState>,
    subscribers: Mutex<Subscribers>,
    cancellation: CancellationToken,
    hooks: Option<Arc<dyn AcpRuntimeHooks>>,
    reports_per_turn_prompt_usage: bool,
}
/// The first `limit` UTF-16 units of `text` (JavaScript `slice`), whole characters only.
fn utf16_prefix(text: &str, limit: usize) -> &str {
    let mut units = 0;
    for (index, character) in text.char_indices() {
        units += character.len_utf16();
        if units > limit {
            return &text[..index];
        }
    }
    text
}
fn bounded_metadata(value: &Value, limit: usize) -> &str {
    utf16_prefix(value.as_str().unwrap_or_default().trim(), limit)
}
fn bounded_uri(value: &Value) -> &str {
    let uri = bounded_metadata(value, 4_096);
    if uri.to_ascii_lowercase().starts_with("data:") {
        ""
    } else {
        uri
    }
}
/// TypeScript `acpContentBlockDisplayText`.
pub fn acp_content_display_text(content: &Value) -> Option<String> {
    let or = |text: &str, fallback: &str| {
        if text.is_empty() {
            fallback.to_owned()
        } else {
            text.to_owned()
        }
    };
    Some(match content["type"].as_str()? {
        "text" => utf16_prefix(content["text"].as_str()?, CONTENT_TEXT_LIMIT).to_owned(),
        "resource_link" => {
            let title = bounded_metadata(&content["title"], 512);
            let label = if title.is_empty() {
                or(bounded_metadata(&content["name"], 512), "resource")
            } else {
                title.to_owned()
            };
            let description = bounded_metadata(&content["description"], 2_048);
            let uri = bounded_uri(&content["uri"]);
            let head = if description.is_empty() {
                label
            } else {
                format!("{label}: {description}")
            };
            if uri.is_empty() {
                head
            } else {
                format!("{head}\n{uri}")
            }
        }
        "resource" => {
            let resource = &content["resource"];
            if let Some(text) = resource["text"].as_str() {
                return Some(utf16_prefix(text, CONTENT_TEXT_LIMIT).to_owned());
            }
            let mime = bounded_metadata(&resource["mimeType"], 256);
            let uri = bounded_uri(&resource["uri"]);
            format!(
                "[ACP binary resource{}{}]",
                if mime.is_empty() {
                    String::new()
                } else {
                    format!(" ({mime})")
                },
                if uri.is_empty() {
                    String::new()
                } else {
                    format!(": {uri}")
                }
            )
        }
        "image" => {
            let mime = or(bounded_metadata(&content["mimeType"], 256), "unknown type");
            let uri = bounded_uri(&content["uri"]);
            format!(
                "[ACP image ({mime}){}]",
                if uri.is_empty() {
                    String::new()
                } else {
                    format!(": {uri}")
                }
            )
        }
        "audio" => format!(
            "[ACP audio ({})]",
            or(bounded_metadata(&content["mimeType"], 256), "unknown type")
        ),
        other => format!(
            "[Unsupported ACP content: {}]",
            or(utf16_prefix(other.trim(), 128), "unknown")
        ),
    })
}

impl Shared {
    /// TypeScript `appendLoadedHistory`: while a session loads, the agent replays its history as
    /// message updates; they become the session's snapshot messages, not live turn items.
    fn capture_history(&self, update: &t3_acp::SessionUpdate) {
        let kind = update.kind();
        let mut state = lock(&self.state);
        let Some((native, thread)) = state.loading.clone() else {
            return;
        };
        if update.session_id != native {
            return;
        }
        let (role, text, replace) = match kind {
            "user_message_chunk" | "agent_message_chunk" | "agent_thought_chunk" => {
                let Some(text) = acp_content_display_text(&update.update["content"]) else {
                    return;
                };
                if text.trim().is_empty() {
                    return;
                }
                (kind, text, false)
            }
            "user_message" | "agent_message" | "agent_thought" => {
                let Some(blocks) = update.update["content"].as_array() else {
                    return;
                };
                let text = blocks
                    .iter()
                    .filter_map(acp_content_display_text)
                    .collect::<Vec<_>>()
                    .join("\n");
                (kind, text, true)
            }
            "tool_call" | "tool_call_update" | "plan" | "plan_update" | "plan_removed" => {
                let history = state.history.entry(native).or_default();
                history.role = None;
                history.message_id = None;
                return;
            }
            _ => return,
        };
        let role = if role.starts_with("user") {
            "user"
        } else if role.contains("thought") {
            "thought"
        } else {
            "assistant"
        };
        if text.is_empty() && !replace {
            return;
        }
        let source = update.update["messageId"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(|id| utf16_prefix(id, 256).to_owned());
        let now = self.clock.now();
        let history = state.history.entry(native.clone()).or_default();
        if history.truncated {
            return;
        }
        let starts_new = history.role != Some(role)
            || source
                .as_ref()
                .is_some_and(|source| history.message_id.as_ref() != Some(source));
        if starts_new {
            history.index += 1;
        }
        let item = match &source {
            Some(source) => format!("{native}:history:{role}:message:{source}"),
            None => format!("{native}:history:{role}:{}", history.index),
        };
        let id = self.item_id("message", &item);
        let (existed, previous_text, created) = match history.messages.get(&id) {
            Some(message) => (true, message.text.clone(), json!(message.created_at)),
            None => (false, String::new(), json!(now)),
        };
        if !existed && history.order.len() >= HISTORY_MAX_MESSAGES {
            history.truncated = true;
            return;
        }
        let mut next = if replace {
            text
        } else {
            format!("{previous_text}{text}")
        };
        if next.len() > HISTORY_MAX_MESSAGE_BYTES {
            let mut end = HISTORY_MAX_MESSAGE_BYTES;
            while !next.is_char_boundary(end) {
                end -= 1;
            }
            next.truncate(end);
        }
        let bytes = history.bytes - previous_text.len() + next.len();
        if bytes > HISTORY_MAX_BYTES {
            history.truncated = true;
            return;
        }
        let Ok(message) = decode::<OrchestrationV2ConversationMessage>(json!({
            "id": id, "threadId": thread, "runId": null, "nodeId": null,
            "createdBy": if role == "user" { "user" } else { "agent" },
            "creationSource": "provider",
            "role": if role == "user" { "user" } else { "assistant" },
            "text": next, "attachments": [], "streaming": false,
            "createdAt": created, "updatedAt": now,
        })) else {
            return;
        };
        if role == "thought"
            && let Ok(id) = expanded(&message.id)
            && !history.reasoning.contains(id)
        {
            history.reasoning.push(id.clone());
        }
        if !existed {
            history.order.push(id.clone());
        }
        history.bytes = bytes;
        history.role = Some(role);
        history.message_id = source.or(if starts_new {
            None
        } else {
            history.message_id.take()
        });
        history.messages.insert(id, message);
    }

    async fn emit(&self, kind: ProviderEventKind) -> ProviderResult<()> {
        let event = ProviderEvent {
            driver: self.driver.clone(),
            kind,
        };
        let subscribers = {
            let mut subscribers = lock(&self.subscribers);
            subscribers.retain(|(sender, stop)| !sender.is_closed() && !stop.is_cancelled());
            subscribers.clone()
        };
        for (sender, stop) in subscribers {
            tokio::select! { _ = self.cancellation.cancelled() => return Err(ProviderError::SessionStopped), _ = stop.cancelled() => {}, _ = sender.send(Ok(event.clone())) => {} }
        }
        Ok(())
    }
    async fn emit_many(&self, events: Vec<ProviderEventKind>) -> ProviderResult<()> {
        for event in events {
            self.emit(event).await?;
        }
        Ok(())
    }
    fn item_native_id(&self, id: &str) -> String {
        if *lock(&self.item_identity_version) == 2 {
            format!("provider-instance:{}:{id}", encoded_part(&self.instance.0))
        } else {
            id.into()
        }
    }
    fn item_id(&self, prefix: &str, native: &str) -> String {
        derived(
            prefix,
            &[
                "provider",
                &self.driver.0,
                "native-item",
                &self.item_native_id(native),
            ],
        )
    }
    fn reference(&self, native: &str, strength: &str) -> Value {
        json!({"driver":self.driver,"nativeId":native,"strength":strength})
    }
    fn item_base(&self, context: &mut TurnContext, native: &str, status: &str) -> Value {
        let ordinal = *context.ordinals.entry(native.into()).or_insert_with(|| {
            let ordinal = context.next_ordinal;
            context.next_ordinal += 1;
            ordinal
        });
        let now = self.clock.now();
        json!({"id":self.item_id("turn-item", native),"threadId":context.input.thread_id,"runId":context.input.run_id,"nodeId":self.item_id("node", native),"providerThreadId":context.input.provider_thread.id,"providerTurnId":context.provider_turn.id,"nativeItemRef":self.reference(native,"weak"),"parentItemId":null,"ordinal":ordinal,"status":status,"title":null,"startedAt":context.provider_turn.started_at,"completedAt":if matches!(status,"completed" | "failed" | "cancelled" | "interrupted") { Some(now.clone()) } else { None },"updatedAt":now})
    }
    fn node_from_item(
        &self,
        context: &TurnContext,
        item: &Value,
        kind: &str,
    ) -> ProviderResult<OrchestrationV2ExecutionNode> {
        decode(
            json!({"id":item["nodeId"],"threadId":context.input.thread_id,"runId":context.input.run_id,"parentNodeId":context.input.root_node_id,"rootNodeId":context.input.root_node_id,"kind":kind,"status":item["status"],"countsForRun":true,"providerThreadId":context.input.provider_thread.id,"providerTurnId":context.provider_turn.id,"nativeItemRef":item["nativeItemRef"],"runtimeRequestId":item.get("requestId").unwrap_or(&Value::Null),"checkpointScopeId":null,"startedAt":item["startedAt"],"completedAt":item["completedAt"]}),
        )
    }
    fn text_events(
        &self,
        snapshot: t3_acp::TextSnapshot,
    ) -> ProviderResult<Vec<ProviderEventKind>> {
        let mut state = lock(&self.state);
        let Some(context) = state.contexts.get_mut(&snapshot.session_id) else {
            return Ok(Vec::new());
        };
        let native = format!(
            "{}:text:{}",
            expanded(&context.provider_turn.id)?.0,
            snapshot.segment
        );
        let mut item = self.item_base(
            context,
            &native,
            if snapshot.complete {
                "completed"
            } else {
                "running"
            },
        );
        let reasoning = snapshot.kind.contains("thought");
        let user = snapshot.kind.starts_with("user");
        item["type"] = json!(if reasoning {
            "reasoning"
        } else if user {
            "user_message"
        } else {
            "assistant_message"
        });
        item["text"] = json!(snapshot.text);
        item["streaming"] = json!(!snapshot.complete);
        let mut events = Vec::new();
        let node = decode(
            json!({"id":self.item_id("node", &native),"threadId":context.input.thread_id,"runId":context.input.run_id,"parentNodeId":context.input.root_node_id,"rootNodeId":context.input.root_node_id,"kind":if reasoning {"reasoning"} else {"assistant_message"},"status":if snapshot.complete {"completed"} else {"running"},"countsForRun":false,"providerThreadId":context.input.provider_thread.id,"providerTurnId":context.provider_turn.id,"nativeItemRef":self.reference(&native,"weak"),"runtimeRequestId":null,"checkpointScopeId":null,"startedAt":context.provider_turn.started_at,"completedAt":item["completedAt"]}),
        )?;
        if !user {
            events.push(ProviderEventKind::Node(Box::new(node)));
        }
        if !reasoning {
            let id = self.item_id("message", &native);
            item["messageId"] = json!(id);
            if user {
                item["createdBy"] = json!("user");
                item["creationSource"] = json!("provider");
                item["inputIntent"] = json!("turn_start");
                item["attachments"] = json!([]);
            }
            let message: OrchestrationV2ConversationMessage = decode(
                json!({"id":id,"threadId":context.input.thread_id,"runId":context.input.run_id,"nodeId":item["nodeId"],"createdBy":if user {"user"} else {"agent"},"creationSource":"provider","role":if user {"user"} else {"assistant"},"text":snapshot.text,"attachments":[],"streaming":!snapshot.complete,"createdAt":context.provider_turn.started_at,"updatedAt":self.clock.now()}),
            )?;
            if let Some(existing) = state.messages.iter_mut().find(|m| m.id == message.id) {
                *existing = message.clone();
            } else {
                state.messages.push(message.clone());
            }
            events.push(ProviderEventKind::Message(Box::new(message)));
        }
        events.push(ProviderEventKind::TurnItem {
            item: Box::new(decode(item)?),
            session_scoped: None,
        });
        Ok(events)
    }
    fn update_events(
        &self,
        update: &t3_acp::SessionUpdate,
    ) -> ProviderResult<Vec<ProviderEventKind>> {
        let mut state = lock(&self.state);
        if update.kind() == "current_model_update" {
            if let Some(model) = update.update["modelId"].as_str() {
                state
                    .selected_models
                    .insert(update.session_id.clone(), model.into());
            }
            return Ok(Vec::new());
        }
        let Some(context) = state.contexts.get_mut(&update.session_id) else {
            return Ok(Vec::new());
        };
        let value = &update.update;
        match update.kind() {
            "tool_call" | "tool_call_update" => {
                let Some(id) = value["toolCallId"].as_str() else {
                    return Ok(Vec::new());
                };
                let tracked = context
                    .tools
                    .entry(id.into())
                    .or_insert_with(|| json!({"toolCallId":id}));
                if let (Some(tracked), Some(fields)) = (tracked.as_object_mut(), value.as_object())
                {
                    for (key, value) in fields {
                        tracked.insert(key.clone(), value.clone());
                    }
                }
                let tracked = tracked.clone();
                let kind = tracked["kind"].as_str().unwrap_or("other");
                let status = match tracked["status"].as_str() {
                    Some("completed") => "completed",
                    Some("failed") => "failed",
                    Some("pending") => "pending",
                    _ => "running",
                };
                let mut item = self.item_base(context, id, status);
                item["type"] = json!("command_execution");
                item["title"] = tracked["title"].clone();
                item["input"] = json!(
                    tracked["rawInput"]["command"]
                        .as_str()
                        .or_else(|| tracked["title"].as_str())
                        .unwrap_or(kind)
                );
                if kind != "execute" {
                    item["toolNonExecutionKind"] = json!(kind);
                }
                let output = tracked["content"].as_array().map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(|block| block["content"]["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                });
                if let Some(output) = output {
                    item["output"] = json!(output);
                }
                Ok(vec![
                    ProviderEventKind::Node(Box::new(self.node_from_item(
                        context,
                        &item,
                        "tool_call",
                    )?)),
                    ProviderEventKind::TurnItem {
                        item: Box::new(decode(item)?),
                        session_scoped: None,
                    },
                ])
            }
            "plan" | "plan_update" | "plan_removed" => {
                let steps: Vec<_> = value["entries"].as_array().into_iter().flatten().enumerate().map(|(index, entry)| json!({"id":format!("step-{index}"),"text":entry["content"],"status":if entry["status"] == "in_progress" { json!("running") } else { entry["status"].clone() }})).collect();
                let id = format!("plan:{}", expanded(&context.provider_turn.id)?.0);
                Ok(vec![ProviderEventKind::Plan(Box::new(decode(
                    json!({"id":id,"kind":"todo_list","threadId":context.input.thread_id,"runId":context.input.run_id,"nodeId":context.input.root_node_id,"status":"active","steps":steps}),
                )?))])
            }
            _ => Ok(Vec::new()),
        }
    }
}

struct RequestHandler {
    shared: Arc<Shared>,
    effects: Arc<dyn AcpClientEffects>,
}
#[async_trait]
impl ClientHandler for RequestHandler {
    async fn request(
        &self,
        request: ClientRequest,
        wire: RequestContext,
    ) -> Result<Value, RpcError> {
        match request {
            ClientRequest::Permission(params) => self.permission(params, wire).await,
            ClientRequest::Elicitation(params) => self.elicitation(params, wire).await,
            request => {
                let (turn_key, execute_approved) = {
                    let state = lock(&self.shared.state);
                    let key = state
                        .active
                        .as_ref()
                        .and_then(|native| state.contexts.get(native))
                        .and_then(|c| expanded(&c.provider_turn.id).ok())
                        .map(|id| id.0.clone());
                    let allowed =
                        state.execute_session || (key.is_some() && key == state.execute_turn);
                    (key, allowed)
                };
                let policy = permission_policy(
                    &lock(&self.shared.policy),
                    self.shared.supported_runtime_modes.as_deref(),
                );
                let context = AcpEffectContext {
                    request: wire,
                    policy,
                    tool_kind: None,
                    turn_key,
                    execute_approved,
                };
                tokio::select! { _ = self.shared.cancellation.cancelled() => Err(RpcError::internal()), result = self.effects.request(request, context) => result }
            }
        }
    }
}
impl RequestHandler {
    /// TypeScript `onUrlElicitation`, admitted only for this runtime's own session: while it
    /// sets up a thread (no session id yet, or the one being loaded) or for the session whose
    /// turn is running. The host binds the answer to this instance's pending attempt.
    async fn url_consent(&self, params: &Value) -> bool {
        let Some(hooks) = self.shared.hooks.clone() else {
            return false;
        };
        let (Some(id), Some(url)) = (params["elicitationId"].as_str(), params["url"].as_str())
        else {
            return false;
        };
        let session = params["sessionId"].as_str();
        // The window the consent belongs to: the setup in progress, or the running turn of the
        // session it names. When the window closes the consent is declined and withdrawn.
        let window = {
            let state = lock(&self.shared.state);
            let loading = state.loading.as_ref().map(|(native, _)| native.as_str());
            let setup = state
                .setting_up
                .clone()
                .filter(|_| session.is_none_or(|s| loading.is_none_or(|l| l == s)));
            let turn = state
                .active
                .as_deref()
                .filter(|active| session.is_none_or(|s| s == *active))
                .and_then(|active| state.contexts.get(active))
                .map(|context| context.window.clone());
            setup.or(turn).filter(|window| !window.is_cancelled())
        };
        let Some(window) = window else {
            return false;
        };
        let message = params["message"].as_str().unwrap_or_default();
        tokio::select! {
            _ = self.shared.cancellation.cancelled() => false,
            _ = window.cancelled() => false,
            accepted = hooks.url_consent(id, url, message) => accepted,
        }
    }

    async fn permission(&self, mut params: Value, wire: RequestContext) -> Result<Value, RpcError> {
        let native = params["sessionId"]
            .as_str()
            .ok_or_else(RpcError::invalid_params)?
            .to_owned();
        {
            let state = lock(&self.shared.state);
            let context = state
                .contexts
                .get(&native)
                .ok_or_else(RpcError::invalid_params)?;
            if let Some(tracked) = params["toolCall"]["toolCallId"]
                .as_str()
                .and_then(|id| context.tools.get(id))
            {
                for key in ["kind", "locations", "title", "rawInput"] {
                    if params["toolCall"].get(key).is_none_or(Value::is_null)
                        && let Some(value) = tracked.get(key)
                    {
                        params["toolCall"][key] = value.clone();
                    }
                }
            }
        }
        let kind = params["toolCall"]["kind"]
            .as_str()
            .unwrap_or("other")
            .to_owned();
        let policy = lock(&self.shared.policy).clone();
        let disposition = permission_disposition_for_driver(
            &self.shared.driver.0,
            &policy,
            &params,
            self.shared.supported_runtime_modes.as_deref(),
        );
        let decision = match disposition {
            PermissionDisposition::Allow => ProviderApprovalDecision::Accept,
            PermissionDisposition::Deny => ProviderApprovalDecision::Decline,
            PermissionDisposition::Ask => self
                .ask(&params, &wire, &kind, None)
                .await?
                .decision
                .ok_or_else(RpcError::invalid_params)?,
        };
        let decision = if decision == ProviderApprovalDecision::AcceptForSession
            && approval_options(&self.shared.driver.0, &params).is_some_and(|options| {
                !options
                    .iter()
                    .any(|option| option.decision == ProviderApprovalDecision::AcceptForSession)
            }) {
            ProviderApprovalDecision::Accept
        } else {
            decision
        };
        if decision == ProviderApprovalDecision::Cancel {
            return Ok(json!({"outcome":{"outcome":"cancelled"}}));
        }
        let preferred = match decision {
            ProviderApprovalDecision::Accept => "allow_once",
            ProviderApprovalDecision::AcceptForSession => "allow_always",
            _ => "reject_once",
        };
        let options = params["options"]
            .as_array()
            .ok_or_else(RpcError::invalid_params)?;
        let id_of_kind = |kind: &str| {
            options
                .iter()
                .find(|option| {
                    option["kind"] == kind
                        && !(decision == ProviderApprovalDecision::AcceptForSession
                            && self.shared.driver.0 == "grok"
                            && option["optionId"].as_str().is_none_or(|id| {
                                t3_contracts::wire::trim_js(id) != "allow-edits-session"
                            }))
                })
                .and_then(|option| option["optionId"].as_str())
                .map(t3_contracts::wire::trim_js)
                .filter(|id| !id.is_empty())
        };
        let option_id = id_of_kind(preferred).or_else(|| {
            (decision == ProviderApprovalDecision::AcceptForSession)
                .then(|| id_of_kind("allow_once"))
                .flatten()
        });
        let Some(option_id) = option_id else {
            return Ok(json!({"outcome":{"outcome":"cancelled"}}));
        };
        if kind == "execute"
            && matches!(
                decision,
                ProviderApprovalDecision::Accept | ProviderApprovalDecision::AcceptForSession
            )
        {
            let mut state = lock(&self.shared.state);
            if decision == ProviderApprovalDecision::AcceptForSession {
                state.execute_session = true;
            } else {
                state.execute_turn = state
                    .contexts
                    .get(&native)
                    .and_then(|c| expanded(&c.provider_turn.id).ok())
                    .map(|id| id.0.clone());
            }
        }
        Ok(json!({"outcome":{"outcome":"selected","optionId":option_id}}))
    }
    async fn elicitation(
        &self,
        mut params: Value,
        wire: RequestContext,
    ) -> Result<Value, RpcError> {
        if params["mode"] == "url" {
            return Ok(
                json!({"action": if self.url_consent(&params).await { "accept" } else { "decline" }}),
            );
        }
        {
            let state = lock(&self.shared.state);
            let native = state
                .active
                .as_deref()
                .ok_or_else(RpcError::invalid_params)?;
            let context = state
                .contexts
                .get(native)
                .ok_or_else(RpcError::invalid_params)?;
            if context.cancellation.is_cancelled()
                || params["sessionId"].as_str().is_some_and(|id| id != native)
            {
                return Err(RpcError::invalid_params());
            }
            params["sessionId"] = json!(native);
        }
        let policy = permission_policy(
            &lock(&self.shared.policy),
            self.shared.supported_runtime_modes.as_deref(),
        );
        if mcp_tool_approval_disposition(&policy, &params, Some(&wire.id.identity()))
            == Some(PermissionDisposition::Allow)
        {
            return Ok(json!({"action":"accept","content":{}}));
        }
        if params["mode"] != "form" {
            return Ok(json!({"action":"decline"}));
        }
        let Some(form) = ElicitationForm::parse(
            &params["requestedSchema"],
            params["message"].as_str().unwrap_or("Question"),
        ) else {
            return Ok(json!({"action":"decline"}));
        };
        let response = self
            .ask(&params, &wire, "user_input", Some(&form.questions))
            .await?;
        if response.decision == Some(ProviderApprovalDecision::Cancel) {
            return Ok(json!({"action":"cancel"}));
        }
        let Some(answers) = response
            .answers
            .and_then(|answers| serde_json::to_value(answers).ok())
        else {
            return Ok(json!({"action":"decline"}));
        };
        Ok(form.content(&answers).map_or_else(
            || json!({"action":"decline"}),
            |content| json!({"action":"accept","content":content}),
        ))
    }
    async fn ask(
        &self,
        params: &Value,
        wire: &RequestContext,
        tool_kind: &str,
        questions: Option<&Value>,
    ) -> Result<RuntimeResponse, RpcError> {
        let native = params["sessionId"]
            .as_str()
            .ok_or_else(RpcError::invalid_params)?;
        let id = self.shared.ids.next(IdKind::RuntimeRequest);
        let (reply, receive) = oneshot::channel();
        let (flushed, _) = tokio::sync::watch::channel(false);
        let (request, item, node, thread_id, stop) = {
            let mut state = lock(&self.shared.state);
            let context = state
                .contexts
                .get_mut(native)
                .ok_or_else(RpcError::invalid_params)?;
            if context.cancellation.is_cancelled() {
                return Ok(RuntimeResponse {
                    request_id: RuntimeRequestId(id),
                    decision: Some(ProviderApprovalDecision::Cancel),
                    feedback: None,
                    answers: None,
                });
            }
            let kind = if questions.is_some() {
                "user_input"
            } else {
                match tool_kind {
                    "read" | "search" => "file-read",
                    "edit" | "delete" | "move" => "file-change",
                    "execute" => "command",
                    _ => "permission",
                }
            };
            let node_id = derived("node", &["runtime-request", &id]);
            let request: OrchestrationV2RuntimeRequest = serde_json::from_value(json!({"id":id,"nodeId":node_id,"providerTurnId":context.provider_turn.id,"nativeRequestRef":self.shared.reference(&wire.id.identity(),"strong"),"kind":kind,"status":"pending","responseCapability":{"type":"live","providerSessionId":self.shared.session_id},"createdAt":self.shared.clock.now(),"resolvedAt":null})).map_err(|_| RpcError::internal())?;
            let mut item = self
                .shared
                .item_base(context, &format!("request:{id}"), "pending");
            item["id"] = json!(derived("turn-item", &["runtime-request", &id]));
            item["nodeId"] = json!(node_id);
            item["requestId"] = json!(id);
            if let Some(questions) = questions {
                item["type"] = json!("user_input_request");
                item["questions"] = questions.clone();
            } else {
                item["type"] = json!("approval_request");
                item["requestKind"] = json!(kind);
                item["prompt"] = params["toolCall"]["title"].clone();
                if let Some(options) = approval_options(&self.shared.driver.0, params) {
                    item["options"] = json!(options);
                }
            }
            let node = self
                .shared
                .node_from_item(
                    context,
                    &item,
                    if questions.is_some() {
                        "user_input_request"
                    } else {
                        "approval_request"
                    },
                )
                .map_err(|_| RpcError::internal())?;
            (
                request,
                item,
                node,
                context.input.thread_id.clone(),
                context.cancellation.clone(),
            )
        };
        let item: OrchestrationV2TurnItem =
            serde_json::from_value(item).map_err(|_| RpcError::internal())?;
        {
            let mut state = lock(&self.shared.state);
            state
                .response_acks
                .insert(wire.id.identity(), flushed.clone());
            state.requests.insert(
                id.clone(),
                PendingRequest {
                    reply: Some(reply),
                    flushed,
                    request: request.clone(),
                },
            );
        }
        let events = vec![
            ProviderEventKind::Node(Box::new(node.clone())),
            ProviderEventKind::RuntimeRequest {
                request: Box::new(request.clone()),
                thread_id: Some(thread_id.clone()),
            },
            ProviderEventKind::TurnItem {
                item: Box::new(item.clone()),
                session_scoped: None,
            },
        ];
        self.shared
            .emit_many(events)
            .await
            .map_err(|_| RpcError::internal())?;
        let response = tokio::select! {
            _ = self.shared.cancellation.cancelled() => RuntimeResponse { request_id: RuntimeRequestId(id.clone()), decision: Some(ProviderApprovalDecision::Cancel), feedback: None, answers: None },
            _ = stop.cancelled() => RuntimeResponse { request_id: RuntimeRequestId(id.clone()), decision: Some(ProviderApprovalDecision::Cancel), feedback: None, answers: None },
            result = receive => result.map_err(|_| RpcError::internal())?,
        };
        lock(&self.shared.state).requests.remove(&id);
        let mut resolved = encode(&request).map_err(|_| RpcError::internal())?;
        resolved["status"] = json!(
            if response.decision == Some(ProviderApprovalDecision::Cancel) {
                "cancelled"
            } else {
                "resolved"
            }
        );
        resolved["resolvedAt"] = json!(self.shared.clock.now());
        if let Some(decision) = response.decision {
            resolved["decision"] = json!(decision);
        }
        if let Some(answers) = &response.answers {
            resolved["answers"] = json!(answers);
        }
        self.shared
            .emit(ProviderEventKind::RuntimeRequest {
                request: Box::new(decode(resolved).map_err(|_| RpcError::internal())?),
                thread_id: Some(thread_id),
            })
            .await
            .map_err(|_| RpcError::internal())?;
        let mut item = encode(&item).map_err(|_| RpcError::internal())?;
        let mut node = encode(&node).map_err(|_| RpcError::internal())?;
        let status = if response.decision == Some(ProviderApprovalDecision::Cancel) {
            "cancelled"
        } else {
            "completed"
        };
        for value in [&mut item, &mut node] {
            value["status"] = json!(status);
            value["completedAt"] = json!(self.shared.clock.now());
        }
        item["updatedAt"] = json!(self.shared.clock.now());
        self.shared
            .emit_many(vec![
                ProviderEventKind::Node(Box::new(decode(node).map_err(|_| RpcError::internal())?)),
                ProviderEventKind::TurnItem {
                    item: Box::new(decode(item).map_err(|_| RpcError::internal())?),
                    session_scoped: None,
                },
            ])
            .await
            .map_err(|_| RpcError::internal())?;
        Ok(response)
    }
}

/// Host bindings a flavour gives the adapter (TypeScript `AcpAdapterV2Flavor`'s
/// `onUrlElicitation`, `onAvailableCommandsUpdate`, `onSessionConfigurationUpdate` and
/// `withRuntimeStartup`). Everything passed in comes from the agent and is untrusted.
#[async_trait]
pub trait AcpRuntimeHooks: Send + Sync {
    /// A sign-in URL the agent asks the user to open; `true` accepts. The adapter calls this only
    /// for its own session while it sets up a thread or runs a turn.
    async fn url_consent(&self, elicitation_id: &str, url: &str, message: &str) -> bool;
    /// The agent's raw `availableCommands`.
    fn available_commands(&self, commands: &[Value]);
    /// The session's `{configOptions, modes, models}` after setup or a configuration update.
    fn session_configuration(&self, configuration: &Value);
    /// Held while the agent process starts and initializes.
    fn startup(&self) -> Option<Box<dyn std::any::Any + Send>>;
}

/// Per-agent switches of the generic adapter (TypeScript `AcpAdapterV2Flavor` fields).
#[derive(Clone, Debug)]
pub struct AcpFlavor {
    /// `capabilities.turns.supportsSteeringByInterruptRestart`.
    pub steering_by_interrupt_restart: bool,
    /// `reportsPerTurnPromptUsage`: the prompt response's usage is the turn's own.
    pub reports_per_turn_prompt_usage: bool,
}
impl Default for AcpFlavor {
    fn default() -> Self {
        Self {
            steering_by_interrupt_restart: true,
            // The generic adapter has always read the prompt response's usage; a flavour that
            // has not proven per-turn usage turns it off (TypeScript's default).
            reports_per_turn_prompt_usage: true,
        }
    }
}

pub struct AcpAdapter {
    pub instance_id: ProviderInstanceId,
    pub driver: ProviderDriverKind,
    pub transport: Arc<dyn AcpTransportFactory>,
    pub effects: Arc<dyn AcpClientEffects>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
    pub initialize: Value,
    pub supported_runtime_modes: BTreeMap<ProviderDriverKind, Vec<RuntimeMode>>,
    pub hooks: Option<Arc<dyn AcpRuntimeHooks>>,
    pub flavor: AcpFlavor,
}

pub const DEV_REPLAY_INSTANCE_ID: &str = "devReplay";

pub fn dev_replay_registration(
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    streaming_delays: bool,
) -> crate::ports::sessions_recovery::provider_adapter_registry::AdapterRegistration {
    crate::ports::sessions_recovery::provider_adapter_registry::AdapterRegistration {
        adapter: Arc::new(AcpAdapter::replay(
            ProviderInstanceId(DEV_REPLAY_INSTANCE_ID.into()),
            clock,
            ids,
            streaming_delays,
        )),
        continuation_key: format!("acpRegistry:instance:{DEV_REPLAY_INSTANCE_ID}"),
        enabled: true,
        credential_access: Vec::new(),
    }
}

impl AcpAdapter {
    pub fn replay(
        instance_id: ProviderInstanceId,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
        streaming_delays: bool,
    ) -> Self {
        Self {
            instance_id,
            driver: ProviderDriverKind("acpRegistry".into()),
            transport: Arc::new(DevReplayTransportFactory {
                ids: ids.clone(),
                streaming_delays,
            }),
            effects: Arc::new(RejectClientEffects),
            clock,
            ids,
            initialize: json!({"protocolVersion":2,"clientInfo":{"name":"t3-code","version":"1"},"clientCapabilities":{}}),
            supported_runtime_modes: acp_supported_runtime_modes(),
            hooks: None,
            flavor: AcpFlavor::default(),
        }
    }
}

/// The runtime modes each built-in ACP flavour offers (TypeScript provider snapshots).
fn acp_supported_runtime_modes() -> BTreeMap<ProviderDriverKind, Vec<RuntimeMode>> {
    use RuntimeMode::*;
    [
        (
            "acpRegistry",
            vec![ApprovalRequired, AutoAcceptEdits, Auto, FullAccess],
        ),
        ("grok", vec![ApprovalRequired, Auto, FullAccess]),
        ("kiro", vec![ApprovalRequired, FullAccess]),
        ("muse", vec![ApprovalRequired, FullAccess]),
        ("pi", vec![ApprovalRequired, AutoAcceptEdits, FullAccess]),
        ("fx", vec![ApprovalRequired, Auto]),
    ]
    .into_iter()
    .map(|(driver, modes)| (ProviderDriverKind(driver.into()), modes))
    .collect()
}

/// Which registry agent an instance runs, as its settings name it (TypeScript reads
/// `settings.source`, `settings.agentId` and the local `settings.commandPath`'s file name).
#[derive(Clone, Debug, Default)]
pub struct AcpRegistryAgent {
    /// The registry id; empty for a local agent.
    pub agent_id: String,
    /// A local agent's command file name, lowercase, without its directory.
    pub local_command: Option<String>,
}
impl AcpRegistryAgent {
    fn local(&self, names: &[&str]) -> bool {
        self.local_command.as_deref().is_some_and(|command| {
            names.iter().any(|name| {
                [*name, &format!("{name}.exe"), &format!("{name}.cmd")].contains(&command)
            })
        })
    }
    /// GitHub Copilot CLI: cancelling its prompt also kills the background agents it started.
    pub fn is_copilot(&self) -> bool {
        self.agent_id == "github-copilot-cli" || self.local(&["copilot"])
    }
    /// Claude Agent over ACP.
    pub fn is_claude(&self) -> bool {
        self.agent_id == "claude-acp" || self.local(&["claude-agent-acp"])
    }
}

/// The ACP Registry flavour (TypeScript `makeAcpRegistryAdapterV2` over `makeAcpAdapterV2`):
/// the client capabilities a registry session advertises (no client file system or terminals,
/// form and URL elicitation), the host's coordinator hooks, and the per-agent switches this
/// adapter supports. `transport` launches the agent the instance's settings resolve to.
pub fn acp_registry_adapter(
    instance_id: ProviderInstanceId,
    agent: &AcpRegistryAgent,
    transport: Arc<dyn AcpTransportFactory>,
    hooks: Option<Arc<dyn AcpRuntimeHooks>>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
) -> AcpAdapter {
    AcpAdapter {
        instance_id,
        driver: ProviderDriverKind("acpRegistry".into()),
        transport,
        effects: Arc::new(RejectClientEffects),
        clock,
        ids,
        initialize: json!({
            "clientInfo": {"name": "t3-code", "version": "0.0.0"},
            "clientCapabilities": {
                "fs": {"readTextFile": false, "writeTextFile": false},
                "terminal": false,
                "elicitation": {"form": {}, "url": {}},
            },
        }),
        supported_runtime_modes: acp_supported_runtime_modes(),
        hooks,
        flavor: AcpFlavor {
            steering_by_interrupt_restart: !agent.is_copilot(),
            // glm-acp-agent is the one agent whose per-turn usage a live probe has verified.
            reports_per_turn_prompt_usage: agent.agent_id == "glm-acp-agent",
        },
    }
}

pub fn acp_capabilities() -> ProviderResult<OrchestrationV2ProviderCapabilities> {
    decode(json!({
        "sessions":{"supportsMultipleProviderThreadsPerSession":false,"supportsModelSwitchInSession":true,"supportsProviderSwitchingViaHandoff":true,"supportsRuntimeModeSwitchInSession":true,"pendingRequestsSurviveRestart":false},
        "threads":{"canCreateEmptyThread":true,"canReadThreadSnapshot":true,"canRollbackThread":false,"canForkThread":false,"canForkFromTurn":false,"canForkFromSubagentThread":false,"exposesNativeThreadId":true},
        "turns":{"exposesNativeTurnId":false,"emitsTurnStarted":true,"emitsTurnCompleted":true,"supportsInterrupt":true,"supportsActiveSteering":false,"supportsSteeringByInterruptRestart":true,"supportsQueuedMessages":true,"terminalStatusQuality":"strong"},
        "streaming":{"streamsAssistantText":true,"streamsReasoning":true,"streamsToolOutput":false,"streamsPlanText":false,"emitsMessageCompleted":true},
        "tools":{"exposesToolItemIds":true,"emitsToolStarted":true,"emitsToolCompleted":true,"emitsToolOutput":true,"supportsMcpTools":false,"supportsDynamicToolCallbacks":false},
        "approvals":{"supportsCommandApproval":true,"supportsFileReadApproval":true,"supportsFileChangeApproval":true,"supportsApplyPatchApproval":false,"approvalsHaveNativeRequestIds":true,"approvalCallbacksAreLiveOnly":true,"approvalsCanOriginateFromSubagents":false},
        "planning":{"emitsPlanUpdated":true,"emitsTodoList":true,"emitsProposedPlan":false,"supportsStructuredQuestions":true,"planDeltasHaveItemIds":false},
        "subagents":{"supportsSubagents":false,"exposesSubagentThreadIds":false,"emitsSubagentLifecycle":false,"canWaitForSubagents":false,"canCloseSubagents":false,"canForkSubagentThread":false},
        "context":{"acceptsSystemContext":false,"acceptsDeveloperContext":false,"acceptsSyntheticUserContext":true,"canGenerateSummaries":false,"canConsumeHandoffSummaries":true,"supportsDeltaHandoff":false,"supportsFullThreadHandoff":false,"maxRecommendedHandoffChars":null},
        "checkpointing":{"appCanCheckpointFilesystem":true,"supportsNestedCheckpointScopes":false,"providerCanRollbackConversation":false,"providerRollbackReturnsSnapshot":false,"providerCanReadConversationSnapshot":true},
        "identity":{"nativeThreadIds":"strong","nativeTurnIds":"none","nativeItemIds":"weak","nativeRequestIds":"strong"},"runtimePolicy":{"enforcement":"client-boundary"}
    }))
}

pub fn acp_per_turn_prompt_usage(
    usage: Option<&Value>,
    has_subagents: bool,
    terminal_status: &str,
    gemini_usage: bool,
) -> ProviderResult<t3_contracts::provider_runtime::TurnTokenUsage> {
    let mut result =
        json!({"usageScope":"main_agent","usageStatus":"unavailable","hasSubagents":has_subagents});
    let Some(usage) = usage.filter(|usage| !usage.is_null()) else {
        return decode(result);
    };
    fn counter(usage: &Value, name: &str) -> ProviderResult<Option<u64>> {
        let Some(value) = usage.get(name).filter(|value| !value.is_null()) else {
            return Ok(None);
        };
        value
            .as_u64()
            .or_else(|| {
                value
                    .as_f64()
                    .filter(|value| {
                        value.is_finite()
                            && *value >= 0.0
                            && value.fract() == 0.0
                            && *value < (u64::MAX as f64)
                    })
                    .map(|value| value as u64)
            })
            .map(Some)
            .ok_or_else(|| protocol(format!("invalid ACP usage counter {name}")))
    }
    let cached = counter(usage, "cachedReadTokens")?;
    let writes = counter(usage, "cachedWriteTokens")?;
    let thought = counter(usage, "thoughtTokens")?;
    let input =
        counter(usage, "inputTokens")?.ok_or_else(|| protocol("missing ACP inputTokens"))?;
    let output =
        counter(usage, "outputTokens")?.ok_or_else(|| protocol("missing ACP outputTokens"))?;
    let input = if gemini_usage {
        input
            .checked_add(cached.unwrap_or_default())
            .ok_or_else(|| protocol("ACP input usage exceeds the contract integer range"))?
    } else {
        input
    };
    let output = if gemini_usage {
        output
            .checked_add(thought.unwrap_or_default())
            .ok_or_else(|| protocol("ACP output usage exceeds the contract integer range"))?
    } else {
        output
    };
    result["usageStatus"] = json!(if terminal_status == "completed" {
        "complete"
    } else {
        "partial"
    });
    result["inputTokens"] = json!(input);
    result["outputTokens"] = json!(output);
    let cached = cached.map(|cached| cached.min(input));
    if let Some(cached) = cached {
        result["cachedInputTokens"] = json!(cached);
    }
    if let Some(writes) = writes {
        result["cacheCreationTokens"] = json!(writes.min(input - cached.unwrap_or_default()));
    }
    if let Some(thought) = thought {
        result["reasoningTokens"] = json!(thought.min(output));
    }
    decode(result)
}

pub fn acp_tool_call_snippet_edit(content: &Value) -> Option<Value> {
    let diffs: Vec<_> = content
        .as_array()?
        .iter()
        .filter(|entry| entry["type"] == "diff")
        .collect();
    let diff = match diffs.as_slice() {
        [diff] => *diff,
        _ => return None,
    };
    if diff["patch"]["format"] != "snippet" {
        return None;
    }
    Some(json!({"oldStr":diff["oldText"].as_str()?,"newStr":diff["newText"].as_str()?}))
}

// Local shim: the host supplies jsdiff-compatible, bounded unified-diff formatting.
pub trait AcpDiffFormatter {
    fn format(
        &self,
        old_path: &str,
        new_path: &str,
        old_text: &str,
        new_text: &str,
        context: usize,
        max_edits: Option<usize>,
    ) -> ProviderResult<Option<String>>;
}
pub fn acp_tool_call_diff_patch(
    content: &Value,
    formatter: &dyn AcpDiffFormatter,
) -> ProviderResult<Option<String>> {
    let Some(content) = content.as_array() else {
        return Ok(None);
    };
    if content.iter().any(|entry| {
        matches!(
            entry["patch"]["format"].as_str(),
            Some("omitted" | "snippet")
        )
    }) {
        return Ok(None);
    }
    let mut patches = Vec::<(String, Option<String>)>::new();
    for diff in content.iter().filter(|entry| entry["type"] == "diff") {
        if let Some(text) = diff["patch"]["text"].as_str() {
            let change = diff["changes"]
                .as_array()
                .filter(|changes| changes.len() == 1)
                .map(|changes| &changes[0]);
            if diff["patch"]["format"] == "git_patch"
                && text.trim_start().starts_with("@@ ")
                && let Some(change) = change
                && let Some(path) = change["path"].as_str()
            {
                let old = change["oldPath"].as_str().unwrap_or(path);
                let before = if change["operation"] == "add" {
                    "/dev/null".into()
                } else {
                    json!(format!("a/{old}")).to_string()
                };
                let after = if change["operation"] == "delete" {
                    "/dev/null".into()
                } else {
                    json!(format!("b/{path}")).to_string()
                };
                patches.push((
                    format!(
                        "diff --git {} {}\n--- {before}\n+++ {after}\n{}",
                        json!(format!("a/{old}")),
                        json!(format!("b/{path}")),
                        text.trim_start()
                    ),
                    None,
                ));
            } else {
                patches.push((text.into(), None));
            }
            continue;
        }
        let (Some(path), Some(new)) = (diff["path"].as_str(), diff["newText"].as_str()) else {
            continue;
        };
        let old = diff["oldText"].as_str();
        if let Some(patch) = formatter.format(
            if old.is_some() { path } else { "/dev/null" },
            path,
            old.unwrap_or_default(),
            new,
            3,
            (old.is_some_and(|old| !old.is_empty()) && !new.is_empty()).then_some(1000),
        )? {
            patches.push((patch, Some(path.into())));
        }
    }
    let has_git_header = patches
        .iter()
        .any(|(text, _)| text.lines().any(|line| line.starts_with("diff --git ")));
    if patches.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        patches
            .into_iter()
            .map(|(text, path)| match path {
                Some(path) if has_git_header => format!(
                    "diff --git {} {}\n{text}",
                    json!(format!("a/{path}")),
                    json!(format!("b/{path}"))
                ),
                _ => text,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    ))
}

enum Control {
    Finish {
        native: String,
        result: Result<Value, String>,
        done: oneshot::Sender<ProviderResult<()>>,
    },
}
pub struct AcpSessionRuntime {
    session: OrchestrationV2ProviderSession,
    client: Client,
    shared: Arc<Shared>,
    control: mpsc::Sender<Control>,
    tasks: tokio::sync::Mutex<Vec<JoinHandle<ProviderResult<()>>>>,
    operations: tokio::sync::Mutex<()>,
    transport_stop: CancellationToken,
    initial_native: Mutex<Option<String>>,
}
impl Drop for AcpSessionRuntime {
    fn drop(&mut self) {
        self.shared.cancellation.cancel();
        self.transport_stop.cancel();
    }
}

#[async_trait]
impl ProviderAdapter for AcpAdapter {
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.instance_id
    }
    fn driver(&self) -> ProviderDriverKind {
        self.driver.clone()
    }
    fn supported_runtime_modes(&self) -> Option<Vec<RuntimeMode>> {
        self.supported_runtime_modes.get(&self.driver).cloned()
    }
    async fn capabilities(&self) -> ProviderResult<OrchestrationV2ProviderCapabilities> {
        let mut capabilities = acp_capabilities()?;
        if self.driver.0 == "grok" || !self.flavor.steering_by_interrupt_restart {
            // Cancelling a Grok (or Copilot) prompt kills its running foreground work.
            capabilities.turns.supports_steering_by_interrupt_restart = false;
        }
        Ok(capabilities)
    }
    async fn plan_selection_transition(
        &self,
        input: SelectionTransitionInput,
    ) -> ProviderResult<SelectionTransitionPlan> {
        if input.current.instance_id != input.target.instance_id {
            return Ok(SelectionTransitionPlan::CreateWithHandoff);
        }
        Ok(
            if input
                .session_capabilities
                .sessions
                .supports_model_switch_in_session
            {
                SelectionTransitionPlan::ApplyOnNextTurn
            } else {
                SelectionTransitionPlan::RestartSession
            },
        )
    }
    async fn open_session(
        &self,
        input: OpenSession,
        cancellation: CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        if cancellation.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        if input
            .initial_provider_item_identity_version
            .is_some_and(|version| version != 2)
        {
            return Err(protocol("unsupported ACP item identity version"));
        }
        let _startup = self.hooks.as_ref().and_then(|hooks| hooks.startup());
        let transport_stop = cancellation.child_token();
        let transport = self
            .transport
            .connect(&input, transport_stop.clone())
            .await?;
        let shared = Arc::new(Shared {
            driver: self.driver.clone(),
            instance: self.instance_id.clone(),
            session_id: input.provider_session_id.clone(),
            item_identity_version: Mutex::new(
                input.initial_provider_item_identity_version.unwrap_or(
                    if input.initial_native_thread_id.is_some() {
                        1
                    } else {
                        2
                    },
                ),
            ),
            policy: Mutex::new(input.runtime_policy.clone()),
            supported_runtime_modes: self.supported_runtime_modes.get(&self.driver).cloned(),
            clock: self.clock.clone(),
            ids: self.ids.clone(),
            state: Mutex::new(MappingState {
                threads: BTreeMap::new(),
                selected_models: BTreeMap::new(),
                contexts: BTreeMap::new(),
                active: None,
                messages: Vec::new(),
                turns: Vec::new(),
                requests: BTreeMap::new(),
                response_acks: BTreeMap::new(),
                execute_session: false,
                execute_turn: None,
                failed: false,
                setting_up: None,
                loading: None,
                history: BTreeMap::new(),
            }),
            subscribers: Mutex::new(Vec::new()),
            cancellation: cancellation.child_token(),
            hooks: self.hooks.clone(),
            reports_per_turn_prompt_usage: self.flavor.reports_per_turn_prompt_usage,
        });
        let handler = Arc::new(RequestHandler {
            shared: shared.clone(),
            effects: self.effects.clone(),
        });
        let (client, events) = Client::connect(
            transport.reader,
            transport.writer,
            handler,
            ClientOptions::default(),
        );
        let (control, commands) = mpsc::channel(16);
        let receiver = tokio::spawn(receive_events(
            shared.clone(),
            client.clone(),
            events,
            commands,
        ));
        let initialized = tokio::select! {
            _ = cancellation.cancelled() => Err(ProviderError::Cancelled),
            result = tokio::time::timeout(SETUP_TIMEOUT, client.initialize(self.initialize.clone())) => match result {
                Ok(result) => result.map_err(native_error),
                Err(_) => Err(protocol("The ACP agent did not answer initialize in time")),
            },
        };
        if let Err(error) = initialized {
            shared.cancellation.cancel();
            transport_stop.cancel();
            client.shutdown().await;
            let _ = receiver.await;
            let _ = transport.completion.await;
            return Err(error);
        }
        let now = self.clock.now();
        let cwd = input
            .runtime_policy
            .cwd
            .as_ref()
            .map(|cwd| cwd.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".".into());
        let session = decode(
            json!({"id":input.provider_session_id,"driver":self.driver,"providerInstanceId":self.instance_id,"status":"ready","cwd":cwd,"model":input.model_selection.model,"capabilities":self.capabilities().await?,"createdAt":now,"updatedAt":now,"lastError":null}),
        )?;
        Ok(Arc::new(AcpSessionRuntime {
            session,
            client,
            shared,
            control,
            tasks: tokio::sync::Mutex::new(vec![receiver, transport.completion]),
            operations: tokio::sync::Mutex::new(()),
            transport_stop,
            initial_native: Mutex::new(input.initial_native_thread_id),
        }))
    }
}

/// TypeScript `onSessionConfigurationUpdate(configOptions, modeState, models)`.
fn publish_configuration(shared: &Shared, client: &Client, native: &str) {
    if let Some(hooks) = &shared.hooks {
        let state = client.session_state(native);
        hooks.session_configuration(&json!({
            "configOptions": state.config_options,
            "modes": state.modes,
            "models": state.models,
        }));
    }
}

async fn receive_events(
    shared: Arc<Shared>,
    client: Client,
    mut events: t3_acp::Events,
    mut commands: mpsc::Receiver<Control>,
) -> ProviderResult<()> {
    struct ListenerGuard(Arc<Shared>);
    impl Drop for ListenerGuard {
        fn drop(&mut self) {
            let mut state = lock(&self.0.state);
            state.failed = true;
            state.requests.clear();
            state.response_acks.clear();
            lock(&self.0.subscribers).clear();
        }
    }
    let _guard = ListenerGuard(shared.clone());
    let mut batcher = t3_acp::TextBatcher::default();
    loop {
        let deadline = batcher.next_deadline();
        tokio::select! {
            _ = shared.cancellation.cancelled() => break,
            _ = async { if let Some(deadline) = deadline { tokio::time::sleep_until(deadline).await; } else { std::future::pending::<()>().await; } } => {
                for snapshot in batcher.flush_due(tokio::time::Instant::now()) { shared.emit_many(shared.text_events(snapshot)?).await?; }
            }
            command = commands.recv() => {
                let Some(Control::Finish { native, result, done }) = command else { break; };
                for snapshot in batcher.end_turn(&native) { shared.emit_many(shared.text_events(snapshot)?).await?; }
                let result = finish_turn(&shared, &native, result).await;
                let _ = done.send(result);
            }
            event = events.next() => {
                let Some(event) = event else { break; };
                match event {
                    Ok(t3_acp::Event::SessionUpdate(update)) => {
                        match update.kind() {
                            "available_commands_update" => if let Some(hooks) = &shared.hooks {
                                hooks.available_commands(update.update["availableCommands"].as_array().map_or(&[], Vec::as_slice));
                            },
                            "config_option_update" | "current_mode_update" => publish_configuration(&shared, &client, &update.session_id),
                            _ => {}
                        }
                        shared.capture_history(&update);
                        for snapshot in batcher.push(&update, tokio::time::Instant::now()) { shared.emit_many(shared.text_events(snapshot)?).await?; }
                        shared.emit_many(shared.update_events(&update)?).await?;
                    }
                    Ok(t3_acp::Event::ResponseSent { id }) => {
                        if let Some(flushed) = lock(&shared.state).response_acks.remove(&id.identity()) { flushed.send_replace(true); }
                    }
                    Ok(_) => {},
                    Err(error) => {
                        lock(&shared.state).failed = true;
                        let native = lock(&shared.state).active.clone();
                        if let Some(native) = native {
                            for snapshot in batcher.end_turn(&native) { shared.emit_many(shared.text_events(snapshot)?).await?; }
                            finish_turn(&shared, &native, Err(error.to_string())).await?;
                        }
                        break;
                    }
                }
                events.acknowledge();
            }
        }
    }
    lock(&shared.state).failed = true;
    lock(&shared.state).requests.clear();
    lock(&shared.state).response_acks.clear();
    Ok(())
}

async fn finish_turn(
    shared: &Shared,
    native: &str,
    result: Result<Value, String>,
) -> ProviderResult<()> {
    let (turn, thread, ordinal, outcome, disposition) = {
        let mut state = lock(&shared.state);
        if state.active.as_deref() != Some(native) {
            return Ok(());
        }
        let context = state
            .contexts
            .get_mut(native)
            .ok_or_else(|| protocol("ACP active turn missing"))?;
        let cancelled = result
            .as_ref()
            .is_ok_and(|r| r["stopReason"] == "cancelled");
        let mut turn = encode(&context.provider_turn)?;
        turn["status"] = json!(if result.is_err() {
            "failed"
        } else if cancelled {
            "interrupted"
        } else {
            "completed"
        });
        turn["turnTokenUsage"] = encode(&acp_per_turn_prompt_usage(
            result
                .as_ref()
                .ok()
                .filter(|_| shared.reports_per_turn_prompt_usage)
                .and_then(|response| response.get("usage")),
            false,
            turn["status"].as_str().unwrap_or("failed"),
            false,
        )?)?;
        turn["completedAt"] = json!(shared.clock.now());
        context.provider_turn = decode(turn)?;
        let turn = context.provider_turn.clone();
        let ordinal = context.input.run_ordinal;
        let mut thread = encode(&context.input.provider_thread)?;
        thread["status"] = json!(if result.is_err() { "error" } else { "idle" });
        thread["lastRunOrdinal"] = json!(ordinal);
        thread["updatedAt"] = json!(shared.clock.now());
        let thread: OrchestrationV2ProviderThread = decode(thread)?;
        let (outcome, disposition) = match result {
            Ok(response) if cancelled => (
                TerminalOutcome::Interrupted {
                    reason: response["stopReason"].as_str().map(str::to_owned),
                },
                ThreadDisposition::Reusable,
            ),
            Ok(response) => (
                TerminalOutcome::Completed {
                    reason: response["stopReason"].as_str().map(str::to_owned),
                },
                ThreadDisposition::Reusable,
            ),
            Err(message) => (
                TerminalOutcome::Failed {
                    failure: Box::new(decode(
                        json!({"class":"provider_error","message":message,"code":null,"retryable":null}),
                    )?),
                    failure_item_ordinal: context.next_ordinal,
                    retry: None,
                    retry_started_at: None,
                    child_turns_continue: false,
                },
                ThreadDisposition::Broken,
            ),
        };
        if let Some(existing) = state.turns.iter_mut().find(|t| t.id == turn.id) {
            *existing = turn.clone();
        }
        state.threads.insert(native.into(), thread.clone());
        state.active = None;
        if let Some(context) = state.contexts.get(native) {
            context.window.cancel();
        }
        state.execute_turn = None;
        (turn, thread, ordinal, outcome, disposition)
    };
    shared
        .emit(ProviderEventKind::Turn {
            provider_turn: Box::new(turn.clone()),
            thread_id: thread
                .app_thread_id
                .as_ref()
                .map(expanded)
                .transpose()?
                .cloned(),
        })
        .await?;
    shared
        .emit(ProviderEventKind::Thread {
            provider_thread: Box::new(thread.clone()),
            pending_background_work: None,
            session_scoped: None,
            retained_native_turn_ids: None,
        })
        .await?;
    shared
        .emit(ProviderEventKind::Terminal {
            provider_thread_id: expanded(&thread.id)?.clone(),
            provider_turn_id: expanded(&turn.id)?.clone(),
            run_ordinal: ordinal,
            disposition,
            outcome,
        })
        .await
}

impl AcpSessionRuntime {
    fn available(&self) -> ProviderResult<()> {
        if self.shared.cancellation.is_cancelled() || lock(&self.shared.state).failed {
            return Err(ProviderError::SessionStopped);
        }
        Ok(())
    }
    fn native_id(thread: &OrchestrationV2ProviderThread) -> ProviderResult<String> {
        thread
            .native_thread_ref
            .as_ref()
            .and_then(|r| r.native_id.as_ref())
            .map(|id| id.0.clone())
            .ok_or_else(|| protocol("ACP provider thread has no native session ID"))
    }
    async fn setup_thread(
        &self,
        thread_id: ThreadId,
        existing: Option<OrchestrationV2ProviderThread>,
        force_new: bool,
        policy: RuntimePolicy,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.available()?;
        let _operation = self.operations.lock().await;
        self.available()?;
        let existing_native = existing.as_ref().and_then(|thread| {
            thread
                .native_thread_ref
                .as_ref()?
                .native_id
                .as_ref()
                .map(|id| id.0.clone())
        });
        {
            let state = lock(&self.shared.state);
            if state.active.is_some() {
                return Err(protocol("cannot load an ACP session during a turn"));
            }
            if !force_new {
                if let Some(existing) = &existing
                    && let Some(thread) = state
                        .threads
                        .values()
                        .find(|thread| thread.id == existing.id)
                {
                    return Ok(thread.clone());
                }
                if existing.is_none()
                    && let Some(thread) = state.threads.values().next()
                {
                    if thread
                        .app_thread_id
                        .as_ref()
                        .and_then(|id| expanded(id).ok())
                        == Some(&thread_id)
                    {
                        return Ok(thread.clone());
                    }
                    return Err(ProviderError::Unsupported(
                        "multiple ACP threads in one session",
                    ));
                }
            }
        }
        let cwd = policy
            .cwd
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.session.cwd.0.clone());
        let params = json!({"cwd":cwd,"mcpServers":[]});
        let resume_native = if force_new {
            None
        } else {
            existing_native
                .clone()
                .or_else(|| lock(&self.initial_native).clone())
        };
        let identity_version = if resume_native.is_none() {
            2
        } else if let Some(existing) = existing.as_ref().filter(|_| existing_native.is_some()) {
            if encode(existing)?["nativeMetadata"]["itemIdentityVersion"] == 2 {
                2
            } else {
                1
            }
        } else {
            *lock(&self.shared.item_identity_version)
        };
        struct Setup<'a>(&'a Shared);
        impl Drop for Setup<'_> {
            fn drop(&mut self) {
                let mut state = lock(&self.0.state);
                if let Some(window) = state.setting_up.take() {
                    window.cancel();
                }
                state.loading = None;
            }
        }
        let setup = Setup(&self.shared);
        lock(&self.shared.state).setting_up = Some(CancellationToken::new());
        let native = if let Some(native) = resume_native {
            if !self
                .client
                .negotiated()
                .is_some_and(|n| n.response["agentCapabilities"]["loadSession"] == true)
            {
                return Err(ProviderError::Unsupported("ACP session/load"));
            }
            {
                // The replay that follows replaces any earlier copy of this session's history.
                let mut state = lock(&self.shared.state);
                state.history.remove(&native);
                state.loading = Some((native.clone(), thread_id.clone()));
            }
            let mut params = params;
            params["sessionId"] = json!(native);
            self.client
                .load_session(params)
                .await
                .map_err(native_error)?;
            native
        } else {
            let result = tokio::time::timeout(SETUP_TIMEOUT, self.client.new_session(params))
                .await
                .map_err(|_| protocol("The ACP agent did not answer session/new in time"))?
                .map_err(native_error)?;
            result["sessionId"]
                .as_str()
                .ok_or_else(|| protocol("ACP session/new omitted sessionId"))?
                .to_owned()
        };
        drop(setup);
        publish_configuration(&self.shared, &self.client, &native);
        lock(&self.initial_native).take();
        *lock(&self.shared.item_identity_version) = identity_version;
        let native_metadata = (identity_version == 2).then(|| json!({"itemIdentityVersion":2}));
        let now = self.shared.clock.now();
        let id = if let Some(existing) = &existing {
            expanded(&existing.id)?.0.clone()
        } else {
            derived(
                "provider-thread",
                &[
                    "provider",
                    &self.shared.driver.0,
                    "provider-instance",
                    &self.shared.instance.0,
                    "native-thread",
                    &native,
                ],
            )
        };
        let thread: OrchestrationV2ProviderThread = decode(
            json!({"id":id,"driver":self.shared.driver,"providerInstanceId":self.shared.instance,"providerSessionId":self.shared.session_id,"appThreadId":thread_id,"ownerNodeId":null,"nativeThreadRef":self.shared.reference(&native,"strong"),"nativeConversationHeadRef":null,"status":"idle","firstRunOrdinal":null,"lastRunOrdinal":null,"handoffIds":[],"forkedFrom":null,"nativeMetadata":native_metadata,"createdAt":now,"updatedAt":now}),
        )?;
        {
            let mut state = lock(&self.shared.state);
            state.threads.retain(|_, previous| previous.id != thread.id);
            state.threads.insert(native, thread.clone());
        }
        *lock(&self.shared.policy) = policy;
        self.shared
            .emit(ProviderEventKind::Thread {
                provider_thread: Box::new(thread.clone()),
                pending_background_work: None,
                session_scoped: None,
                retained_native_turn_ids: None,
            })
            .await?;
        Ok(thread)
    }
    async fn select_model(&self, native: &str, model: &str) -> ProviderResult<()> {
        if model == "default" {
            return Ok(());
        }
        let state = self.client.session_state(native);
        let current = lock(&self.shared.state)
            .selected_models
            .get(native)
            .cloned()
            .or_else(|| {
                state
                    .models
                    .as_ref()
                    .and_then(|models| models["currentModelId"].as_str())
                    .map(str::to_owned)
            });
        if current.as_deref() == Some(model) {
            return Ok(());
        }
        if self.client.generation() == t3_acp::Generation::V1 {
            self.client
                .set_model(json!({"sessionId":native,"modelId":model}))
                .await
                .map_err(native_error)?;
        } else {
            self.client
                .set_config_option(json!({"sessionId":native,"configId":"model","value":model}))
                .await
                .map_err(native_error)?;
        }
        lock(&self.shared.state)
            .selected_models
            .insert(native.into(), model.into());
        publish_configuration(&self.shared, &self.client, native);
        Ok(())
    }
}

#[async_trait]
impl SessionRuntime for AcpSessionRuntime {
    fn session(&self) -> &OrchestrationV2ProviderSession {
        &self.session
    }
    fn can_resume_after_restart(&self) -> bool {
        self.client
            .negotiated()
            .is_some_and(|n| n.response["agentCapabilities"]["loadSession"] == true)
    }
    fn background_tasks_end_with_process(&self) -> bool {
        true
    }
    async fn subscribe(&self) -> ProviderResult<EventSubscription> {
        self.available()?;
        let (sender, events) = mpsc::channel(64);
        let cancellation = self.shared.cancellation.child_token();
        sender
            .send(Ok(ProviderEvent {
                driver: self.shared.driver.clone(),
                kind: ProviderEventKind::Session(Box::new(self.session.clone())),
            }))
            .await
            .map_err(|_| ProviderError::SubscriptionClosed)?;
        lock(&self.shared.subscribers).push((sender, cancellation.clone()));
        Ok(EventSubscription {
            events,
            cancellation,
        })
    }
    async fn close(&self) -> ProviderResult<()> {
        self.shared.cancellation.cancel();
        self.transport_stop.cancel();
        self.client.shutdown().await;
        lock(&self.shared.subscribers).clear();
        let tasks = std::mem::take(&mut *self.tasks.lock().await);
        let mut first_error = None;
        for task in tasks {
            if let Err(error) = task.await.map_err(native_error).and_then(|result| result)
                && first_error.is_none()
                && !matches!(
                    error,
                    ProviderError::SessionStopped | ProviderError::Cancelled
                )
            {
                first_error = Some(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
    async fn ensure_thread(
        &self,
        input: EnsureThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.setup_thread(
            input.thread_id,
            input.existing_provider_thread,
            input.force_new_native_thread,
            input.runtime_policy,
        )
        .await
    }
    async fn resume_thread(
        &self,
        input: ResumeThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        let thread_id = input
            .thread_id
            .or_else(|| {
                input
                    .provider_thread
                    .app_thread_id
                    .as_ref()
                    .and_then(|id| expanded(id).ok())
                    .cloned()
            })
            .ok_or_else(|| protocol("ACP resume requires app thread ID"))?;
        self.setup_thread(
            thread_id,
            Some(input.provider_thread),
            false,
            input
                .runtime_policy
                .unwrap_or_else(|| lock(&self.shared.policy).clone()),
        )
        .await
    }
    async fn start_turn(&self, input: TurnInput) -> ProviderResult<()> {
        self.available()?;
        if !input.message.attachments.is_empty() {
            return Err(ProviderError::Unsupported("ACP attachment encoding"));
        }
        let _operation = self.operations.lock().await;
        self.available()?;
        let native = Self::native_id(&input.provider_thread)?;
        {
            let state = lock(&self.shared.state);
            if state.active.is_some() {
                return Err(ProviderError::Protocol {
                    reason: Some(ProtocolReason::PreviousTurnActive),
                    detail: "ACP turn already active".into(),
                    payload: None,
                });
            }
            if !state
                .threads
                .get(&native)
                .is_some_and(|thread| thread.id == input.provider_thread.id)
            {
                return Err(protocol(
                    "ACP provider thread does not belong to this runtime",
                ));
            }
        }
        self.select_model(&native, &input.model_selection.model)
            .await?;
        *lock(&self.shared.policy) = input.runtime_policy.clone();
        let now = self.shared.clock.now();
        let native_turn = format!("{}:turn:{}", native, input.provider_turn_ordinal);
        let turn: OrchestrationV2ProviderTurn = decode(
            json!({"id":derived("provider-turn", &["provider",&self.shared.driver.0,"native-turn",&native_turn]),"providerThreadId":input.provider_thread.id,"nodeId":input.root_node_id,"runAttemptId":input.attempt_id,"nativeTurnRef":self.shared.reference(&native_turn,"weak"),"ordinal":input.provider_turn_ordinal,"status":"running","startedAt":now,"completedAt":null,"model":input.model_selection.model}),
        )?;
        let mut prompt = Vec::new();
        for (_, text) in &input.app_context {
            prompt.push(json!({"type":"text","text":text}));
        }
        prompt.push(json!({"type":"text","text":input.message.text}));
        {
            let mut state = lock(&self.shared.state);
            state.turns.push(turn.clone());
            state.active = Some(native.clone());
            state.contexts.insert(
                native.clone(),
                TurnContext {
                    input: input.clone(),
                    provider_turn: turn.clone(),
                    cancellation: self.shared.cancellation.child_token(),
                    window: CancellationToken::new(),
                    tools: BTreeMap::new(),
                    ordinals: BTreeMap::new(),
                    next_ordinal: 101,
                },
            );
        }
        self.shared
            .emit(ProviderEventKind::Turn {
                provider_turn: Box::new(turn),
                thread_id: Some(input.thread_id.clone()),
            })
            .await?;
        let client = self.client.clone();
        let sender = self.control.clone();
        let cancellation = self.shared.cancellation.clone();
        let task = tokio::spawn(async move {
            let result = tokio::select! { _ = cancellation.cancelled() => return Ok(()), result = client.prompt(json!({"sessionId":native,"prompt":prompt})) => result.map_err(|error| error.to_string()) };
            let (done, receiver) = oneshot::channel();
            tokio::select! { _ = cancellation.cancelled() => return Ok(()), result = sender.send(Control::Finish { native, result, done }) => result.map_err(|_| ProviderError::SessionStopped)? }
            tokio::select! { _ = cancellation.cancelled() => Ok(()), result = receiver => result.map_err(|_| ProviderError::SessionStopped)? }
        });
        self.tasks.lock().await.push(task);
        Ok(())
    }
    async fn steer_turn(&self, _: SteerInput) -> ProviderResult<()> {
        Err(ProviderError::Unsupported("ACP active steering"))
    }
    async fn interrupt_turn(&self, input: InterruptInput) -> ProviderResult<()> {
        self.available()?;
        let native = Self::native_id(&input.provider_thread)?;
        {
            let state = lock(&self.shared.state);
            let context = state
                .contexts
                .get(&native)
                .ok_or_else(|| protocol("no ACP turn to interrupt"))?;
            if expanded(&context.provider_turn.id)? != &input.provider_turn_id {
                return Err(protocol("ACP turn identity mismatch"));
            }
            context.cancellation.cancel();
        }
        self.client
            .cancel_and_wait(json!({"sessionId":native}))
            .await
            .map_err(|error| match error {
                t3_acp::Error::Timeout(_) => ProviderError::InterruptTimeout {
                    turn_id: input.provider_turn_id,
                    timeout_ms: 15000,
                },
                error => native_error(error),
            })
    }
    async fn respond(&self, input: RuntimeResponse) -> ProviderResult<()> {
        self.available()?;
        let mut flushed = {
            let mut state = lock(&self.shared.state);
            let pending = state
                .requests
                .get_mut(&input.request_id.0)
                .ok_or_else(|| protocol("no pending ACP runtime request"))?;
            let kind = encode(&pending.request)?["kind"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            if kind != "user_input" && input.decision.is_none() {
                return Err(protocol("ACP approval requires a decision"));
            }
            let reply = pending
                .reply
                .take()
                .ok_or_else(|| protocol("ACP runtime request already resolved"))?;
            let flushed = pending.flushed.subscribe();
            reply
                .send(input)
                .map_err(|_| ProviderError::SessionStopped)?;
            flushed
        };
        tokio::select! { _ = self.shared.cancellation.cancelled() => Err(ProviderError::SessionStopped), result = flushed.wait_for(|done| *done) => result.map(|_| ()).map_err(|_| ProviderError::SessionStopped) }
    }
    async fn snapshot(
        &self,
        thread: OrchestrationV2ProviderThread,
    ) -> ProviderResult<ThreadSnapshot> {
        self.available()?;
        let native = Self::native_id(&thread)?;
        let loaded = lock(&self.shared.state)
            .threads
            .get(&native)
            .is_some_and(|known| known.id == thread.id);
        if !loaded {
            // TypeScript `readThreadSnapshot` loads the session first; its replay is the history.
            let app_thread = thread
                .app_thread_id
                .as_ref()
                .map(expanded)
                .transpose()?
                .cloned()
                .ok_or_else(|| protocol("ACP snapshot requires app thread ID"))?;
            let policy = lock(&self.shared.policy).clone();
            self.setup_thread(app_thread, Some(thread.clone()), false, policy)
                .await?;
        }
        let state = lock(&self.shared.state);
        let history = state.history.get(&native);
        let mut messages: Vec<_> = history
            .map(|history| {
                history
                    .order
                    .iter()
                    .filter_map(|id| history.messages.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default();
        let reasoning = history.map_or_else(Vec::new, |history| history.reasoning.clone());
        let thread = state
            .threads
            .get(&native)
            .filter(|known| known.id == thread.id)
            .ok_or(ProviderError::Unsupported(
                "ACP snapshot of an unloaded session",
            ))?
            .clone();
        Ok(ThreadSnapshot {
            provider_thread: thread,
            provider_turns: state
                .turns
                .iter()
                .filter(|turn| turn.provider_thread_id == state.threads[&native].id)
                .cloned()
                .collect(),
            messages: {
                messages.extend(
                    state
                        .messages
                        .iter()
                        .filter(|message| {
                            Some(message.thread_id.clone()) == state.threads[&native].app_thread_id
                        })
                        .cloned(),
                );
                messages
            },
            reasoning_message_ids: Some(reasoning),
            runtime_requests: state
                .requests
                .values()
                .map(|pending| pending.request.clone())
                .collect(),
            provider_payload: Some(json!({"protocol":"acp.ndjson-jsonrpc","sessionId":native})),
        })
    }
    async fn rollback(&self, _: RollbackInput) -> ProviderResult<ThreadSnapshot> {
        Err(ProviderError::Unsupported("ACP conversation rollback"))
    }
    async fn fork(&self, _: ForkInput) -> ProviderResult<OrchestrationV2ProviderThread> {
        Err(ProviderError::Unsupported("ACP session/fork"))
    }
}

#[derive(Clone)]
enum FormField {
    Text,
    Number {
        integer: bool,
    },
    Boolean,
    Choice(Vec<String>),
    MultiChoice {
        values: Vec<String>,
        min: u64,
        max: u64,
    },
}
pub struct ElicitationForm {
    pub questions: Value,
    fields: Vec<(String, FormField, bool)>,
    companions: BTreeMap<String, String>,
}
fn nonempty(value: &Value) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
}
fn enum_options(values: &Value) -> Option<Vec<Value>> {
    let values = values.as_array().filter(|values| !values.is_empty())?;
    let values = values
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()?;
    let mut options: Vec<_> = values
        .iter()
        .map(|value| {
            let trimmed = value.trim();
            let label = if !trimmed.is_empty()
                && values.iter().filter(|v| v.trim() == trimmed).count() == 1
            {
                trimmed.to_owned()
            } else {
                json!(value).to_string()
            };
            json!({"label":label,"description":label,"value":value})
        })
        .collect();
    if options
        .iter()
        .enumerate()
        .any(|(i, option)| options[..i].iter().any(|o| o["label"] == option["label"]))
    {
        for option in &mut options {
            let label = option["value"].to_string();
            option["label"] = json!(label);
            option["description"] = json!(label);
        }
    }
    Some(options)
}
fn titled_options(values: &Value) -> Option<Vec<Value>> {
    values.as_array().filter(|v| !v.is_empty())?.iter().map(|entry| {
        let value = entry["const"].as_str()?;
        let label = nonempty(&entry["title"]).or_else(|| nonempty(&entry["const"]))?;
        Some(json!({"label":label,"description":nonempty(&entry["description"]).unwrap_or(label),"value":value}))
    }).collect()
}
impl ElicitationForm {
    pub fn parse(schema: &Value, message: &str) -> Option<Self> {
        let mut fields = Vec::new();
        let mut parsed = Vec::new();
        let properties = schema.get("properties").and_then(Value::as_object);
        for (id, property) in properties.into_iter().flatten() {
            if id.trim() != id || id.is_empty() || !property.is_object() {
                return None;
            }
            let (field, options) = match property["type"].as_str()? {
                "string"
                    if property.get("enum").is_none_or(Value::is_null)
                        && property.get("oneOf").is_none_or(Value::is_null) =>
                {
                    (FormField::Text, Vec::new())
                }
                "string" => {
                    let options = if property["oneOf"].is_array() {
                        titled_options(&property["oneOf"])
                    } else {
                        enum_options(&property["enum"])
                    }?;
                    (
                        FormField::Choice(
                            options
                                .iter()
                                .filter_map(|o| o["value"].as_str().map(str::to_owned))
                                .collect(),
                        ),
                        options,
                    )
                }
                "array" => {
                    let items = &property["items"];
                    let options = if items["type"] == "string" {
                        enum_options(&items["enum"])
                    } else if items.get("type").is_none() {
                        titled_options(&items["anyOf"])
                    } else {
                        None
                    }?;
                    let values: Vec<_> = options
                        .iter()
                        .filter_map(|o| o["value"].as_str().map(str::to_owned))
                        .collect();
                    let min = property["minItems"].as_u64().unwrap_or(0);
                    let max = property["maxItems"].as_u64().unwrap_or(u64::MAX);
                    let distinct = values
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len() as u64;
                    if max == 0 || min > max || min > distinct {
                        return None;
                    }
                    (FormField::MultiChoice { values, min, max }, options)
                }
                "boolean" => (
                    FormField::Boolean,
                    vec![
                        json!({"label":"true","description":"Yes","value":"true"}),
                        json!({"label":"false","description":"No","value":"false"}),
                    ],
                ),
                "number" | "integer" => (
                    FormField::Number {
                        integer: property["type"] == "integer",
                    },
                    Vec::new(),
                ),
                _ => return None,
            };
            let required = schema["required"]
                .as_array()
                .is_some_and(|keys| keys.iter().any(|key| key == id));
            fields.push((id.clone(), field, required));
            parsed.push((id.clone(), property, options));
        }
        let mut companions = BTreeMap::new();
        for (id, field, required) in &fields {
            let companion = format!("{id}__other");
            if matches!(field, FormField::Choice(_))
                && !required
                && fields.iter().any(|(id, field, required)| {
                    *id == companion && matches!(field, FormField::Text) && !required
                })
            {
                companions.insert(id.clone(), companion);
            }
        }
        let mut questions = Vec::new();
        for (id, property, options) in parsed {
            if companions.values().any(|companion| companion == &id) {
                continue;
            }
            let field = &fields.iter().find(|(key, _, _)| key == &id)?.1;
            let mut question = json!({"id":id,"header":nonempty(&property["title"]).map(str::to_owned).unwrap_or_else(||format!("Question {}", questions.len()+1)),"question":nonempty(&property["description"]).unwrap_or(message),"options":options});
            if !matches!(field, FormField::Text | FormField::Number { .. })
                && !companions.contains_key(&id)
            {
                question["allowCustomAnswer"] = json!(false);
            }
            if matches!(field, FormField::MultiChoice { .. }) {
                question["multiSelect"] = json!(true);
            }
            questions.push(question);
        }
        Some(Self {
            questions: json!(questions),
            fields,
            companions,
        })
    }
    pub fn content(&self, answers: &Value) -> Option<Value> {
        let mut content = serde_json::Map::new();
        for (id, field, required) in &self.fields {
            if self.companions.values().any(|companion| companion == id) {
                continue;
            }
            let answer = answers.get(id);
            let value = answer.and_then(|answer| match field {
                FormField::Text => answer.as_str().map(|s| json!(s)),
                FormField::Boolean => answer
                    .as_bool()
                    .or_else(|| match answer.as_str() {
                        Some("true") => Some(true),
                        Some("false") => Some(false),
                        _ => None,
                    })
                    .map(|b| json!(b)),
                FormField::Number { integer } => {
                    let value = answer.as_f64().or_else(|| {
                        answer
                            .as_str()
                            .filter(|s| !s.trim().is_empty())
                            .and_then(|s| s.trim().parse::<f64>().ok())
                    })?;
                    (value.is_finite() && (!integer || value.fract() == 0.0)).then(|| json!(value))
                }
                FormField::Choice(values) => answer
                    .as_str()
                    .filter(|s| values.iter().any(|v| v == s))
                    .map(|s| json!(s)),
                FormField::MultiChoice { values, min, max } => {
                    let answer = answer.as_array()?;
                    let mut selected = Vec::new();
                    for entry in answer
                        .iter()
                        .filter_map(Value::as_str)
                        .filter(|s| values.iter().any(|v| v == s))
                    {
                        if !selected.contains(&entry) {
                            selected.push(entry);
                        }
                    }
                    if selected.is_empty() && !answer.is_empty() {
                        return None;
                    }
                    ((selected.len() as u64) >= *min && (selected.len() as u64) <= *max)
                        .then(|| json!(selected))
                }
            });
            if let Some(value) = value {
                content.insert(id.clone(), value);
            } else if let Some((companion, answer)) =
                self.companions.get(id).zip(answer.and_then(Value::as_str))
            {
                content.insert(companion.clone(), json!(answer));
            } else if *required {
                return None;
            }
        }
        Some(Value::Object(content))
    }
}

#[cfg(test)]
mod startup_bounds {
    //! Review L1: an agent that never answers `initialize` or `session/new` cannot hold a turn
    //! (and the session's operation lock and startup guard) forever.
    use super::*;
    use crate::testing::{ManualClock, SequenceIds, TEST_TIME};
    use t3_contracts::{model_selection::ModelSelection, provider_policy::ProviderInteractionMode};

    /// A peer that answers `initialize` only when `answers_initialize`, and nothing else.
    struct Silent {
        answers_initialize: bool,
    }

    #[async_trait]
    impl AcpTransportFactory for Silent {
        async fn connect(
            &self,
            _: &OpenSession,
            cancellation: CancellationToken,
        ) -> ProviderResult<AcpTransport> {
            let (client, server) = tokio::io::duplex(64 * 1024);
            let (reader, writer) = tokio::io::split(client);
            let answers = self.answers_initialize;
            let stop = cancellation.clone();
            let completion = tokio::spawn(async move {
                let (read, mut write) = tokio::io::split(server);
                let mut lines = BufReader::new(read).lines();
                loop {
                    let line = tokio::select! {
                        _ = stop.cancelled() => break,
                        line = lines.next_line() => line.map_err(native_error)?,
                    };
                    let Some(line) = line else { break };
                    let Ok(message) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    if answers && message["method"] == "initialize" {
                        let reply = json!({"jsonrpc":"2.0","id":message["id"],"result":{"protocolVersion":1,"agentCapabilities":{}}});
                        let mut bytes = serde_json::to_vec(&reply).map_err(native_error)?;
                        bytes.push(b'\n');
                        write.write_all(&bytes).await.map_err(native_error)?;
                    }
                }
                Ok(())
            });
            Ok(AcpTransport {
                reader: Box::new(reader),
                writer: Box::new(writer),
                cancellation,
                completion,
            })
        }
    }

    fn adapter(answers_initialize: bool) -> AcpAdapter {
        let mut adapter = AcpAdapter::replay(
            ProviderInstanceId("registry".into()),
            Arc::new(ManualClock::new(TEST_TIME)),
            Arc::new(SequenceIds::new("bounds")),
            false,
        );
        adapter.transport = Arc::new(Silent { answers_initialize });
        adapter
    }

    fn policy() -> RuntimePolicy {
        RuntimePolicy {
            runtime_mode: RuntimeMode::ApprovalRequired,
            interaction_mode: ProviderInteractionMode::Default,
            cwd: Some(".".into()),
            approval_policy: None,
            sandbox_policy: None,
            max_runtime_mode: None,
        }
    }

    fn selection() -> ModelSelection {
        serde_json::from_value(json!({"instanceId":"registry","model":"model"}))
            .unwrap_or_else(|error| panic!("selection: {error}"))
    }

    fn open() -> OpenSession {
        OpenSession {
            thread_id: ThreadId("thread:bounds".into()),
            provider_session_id: ProviderSessionId("session:bounds".into()),
            model_selection: selection(),
            runtime_policy: policy(),
            prompt_suggestions: None,
            resume_from_session: None,
            initial_native_thread_id: None,
            initial_provider_item_identity_version: None,
        }
    }

    /// Ten minutes of virtual time: far past the bound, and a regression fails instead of hanging.
    const OUTER: Duration = Duration::from_secs(600);

    #[tokio::test(start_paused = true)]
    async fn an_agent_that_never_answers_initialize_fails_the_open() {
        let opened = tokio::time::timeout(
            OUTER,
            adapter(false).open_session(open(), CancellationToken::new()),
        )
        .await;
        assert!(
            matches!(opened, Ok(Err(_))),
            "the open did not fail within its bound"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_agent_that_never_answers_session_new_fails_the_thread_setup() {
        let runtime = adapter(true)
            .open_session(open(), CancellationToken::new())
            .await
            .unwrap_or_else(|error| panic!("open: {error}"));
        let setup = tokio::time::timeout(
            OUTER,
            runtime.ensure_thread(EnsureThread {
                thread_id: ThreadId("thread:bounds".into()),
                title: None,
                model_selection: selection(),
                runtime_policy: policy(),
                provider_session_id: None,
                existing_provider_thread: None,
                force_new_native_thread: false,
            }),
        )
        .await;
        assert!(
            matches!(setup, Ok(Err(_))),
            "the setup did not fail within its bound"
        );
        let _ = runtime.close().await;
    }
}
