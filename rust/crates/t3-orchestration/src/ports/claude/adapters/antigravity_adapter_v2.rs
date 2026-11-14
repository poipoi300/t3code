#![cfg(feature = "acp-adapter")]

use crate::adapters::acp::{AcpClientEffects, AcpEffectContext};
use crate::provider::{ProviderError, ProviderResult};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::{BTreeSet, VecDeque};
use std::{
    io::Read,
    path::{Component, Path, PathBuf},
};
use t3_acp::{ClientRequest, RpcError};
use t3_contracts::{orchestration_v2::*, provider_policy::RuntimeMode};

pub const ANTIGRAVITY_SUBAGENT_BATCH_TITLE: &str = "Antigravity subagent batch";
pub const ANTIGRAVITY_CLIENT_FILE_MAX_BYTES: u64 = 8 * 1024 * 1024;

fn file_error(code: i64, message: &str) -> RpcError {
    RpcError {
        code,
        message: message.into(),
        data: None,
    }
}

fn confined_file(roots: &[PathBuf], path: &Path) -> Result<PathBuf, RpcError> {
    let outside = || file_error(-32602, "Path is outside the session workspace");
    let absolute = std::path::absolute(path).map_err(|_| outside())?;
    let mut candidate = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                candidate.pop();
            }
            Component::CurDir => {}
            _ => candidate.push(component),
        }
    }
    let mut missing = Vec::new();
    let real = loop {
        match std::fs::canonicalize(&candidate) {
            Ok(mut real) => {
                for component in missing.into_iter().rev() {
                    real.push(component);
                }
                break real;
            }
            Err(_) => {
                if !matches!(std::fs::symlink_metadata(&candidate),Err(error) if error.kind() == std::io::ErrorKind::NotFound)
                {
                    return Err(outside());
                }
                let name = candidate.file_name().ok_or_else(outside)?.to_owned();
                missing.push(name);
                if !candidate.pop() {
                    return Err(outside());
                }
            }
        }
    };
    if !roots
        .iter()
        .any(|root| real.starts_with(std::fs::canonicalize(root).unwrap_or_else(|_| root.clone())))
    {
        return Err(outside());
    }
    Ok(real)
}

pub fn read_antigravity_client_text_file(
    roots: &[PathBuf],
    path: &Path,
    line: Option<u64>,
    limit: Option<u64>,
) -> Result<String, RpcError> {
    let file = confined_file(roots, path)?;
    let metadata = std::fs::metadata(&file).map_err(|_| file_error(-32002, "File not found"))?;
    if !metadata.is_file() || metadata.len() > ANTIGRAVITY_CLIENT_FILE_MAX_BYTES {
        return Err(file_error(
            -32602,
            "File is not a readable text file under 8388608 bytes",
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(file)
        .and_then(|file| {
            file.take(ANTIGRAVITY_CLIENT_FILE_MAX_BYTES + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|_| file_error(-32603, "Could not read text file"))?;
    if bytes.len() as u64 > ANTIGRAVITY_CLIENT_FILE_MAX_BYTES {
        return Err(file_error(-32602, "Text file is too large"));
    }
    let text =
        String::from_utf8(bytes).map_err(|_| file_error(-32602, "File is not UTF-8 text"))?;
    if text.contains('\0') {
        return Err(file_error(-32602, "File contains binary data"));
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    if line.is_none() && limit.is_none() {
        return Ok(text.into());
    }
    let start = usize::try_from(line.unwrap_or(1).saturating_sub(1)).unwrap_or(usize::MAX);
    let limit = limit
        .and_then(|limit| usize::try_from(limit).ok())
        .unwrap_or(usize::MAX);
    Ok(text
        .split('\n')
        .skip(start)
        .take(limit)
        .collect::<Vec<_>>()
        .join("\n"))
}

pub struct AntigravityClientFiles {
    pub attachments_directory: PathBuf,
}

#[async_trait]
impl AcpClientEffects for AntigravityClientFiles {
    async fn request(
        &self,
        request: ClientRequest,
        context: AcpEffectContext,
    ) -> Result<Value, RpcError> {
        let ClientRequest::ReadTextFile(request) = request else {
            return Err(file_error(-32601, "Client operation is not supported"));
        };
        let path = request["path"]
            .as_str()
            .ok_or_else(RpcError::invalid_params)?
            .to_owned();
        let mut roots = vec![self.attachments_directory.clone()];
        if let Some(cwd) = context.policy.cwd {
            roots.push(cwd);
        }
        let line = request["line"].as_u64();
        let limit = request["limit"].as_u64();
        tokio::task::spawn_blocking(move || {
            read_antigravity_client_text_file(&roots, Path::new(&path), line, limit)
        })
        .await
        .map_err(|_| RpcError::internal())?
        .map(|content| json!({"content":content}))
    }
}

pub fn antigravity_capabilities() -> ProviderResult<OrchestrationV2ProviderCapabilities> {
    let mut capabilities = crate::adapters::acp::acp_capabilities()?;
    capabilities.sessions.supports_model_switch_in_session = true;
    capabilities
        .sessions
        .supports_runtime_mode_switch_in_session = true;
    capabilities.tools.supports_mcp_tools = true;
    capabilities.subagents.supports_subagents = true;
    Ok(capabilities)
}

pub const ANTIGRAVITY_SUPPORTED_RUNTIME_MODES: [RuntimeMode; 3] = [
    RuntimeMode::ApprovalRequired,
    RuntimeMode::AutoAcceptEdits,
    RuntimeMode::FullAccess,
];

pub fn antigravity_permission_mode(mode: RuntimeMode) -> &'static str {
    match mode {
        RuntimeMode::FullAccess => "yolo",
        RuntimeMode::AutoAcceptEdits => "auto_edit",
        RuntimeMode::Auto | RuntimeMode::ApprovalRequired => "default",
    }
}

pub fn antigravity_cli_session_mode(mode: &str) -> &str {
    match mode {
        "yolo" | "auto_edit" => "default",
        mode => mode,
    }
}

pub fn antigravity_runtime_mode_for_session_mode(mode: &str) -> Option<RuntimeMode> {
    match mode {
        "yolo" => Some(RuntimeMode::FullAccess),
        "auto_edit" => Some(RuntimeMode::AutoAcceptEdits),
        "default" => Some(RuntimeMode::ApprovalRequired),
        _ => None,
    }
}

fn bounded(text: &str, limit: usize) -> String {
    if text.encode_utf16().count() <= limit {
        return text.into();
    }
    let mut units = 0;
    let prefix = text
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= limit - 3
        })
        .collect::<String>();
    format!("{prefix}...")
}

fn question_label(option: &Value) -> String {
    bounded(
        option["name"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| option["optionId"].as_str())
            .unwrap_or_default(),
        512,
    )
}

pub fn antigravity_user_input_question(
    request: &Value,
) -> Option<OrchestrationV2UserInputQuestion> {
    let id = request["toolCall"]["toolCallId"].as_str()?;
    if !id.starts_with("interaction_") {
        return None;
    }
    let options = request["options"].as_array().filter(|v| !v.is_empty())?;
    let mut ids = BTreeSet::new();
    for option in options {
        let id = option["optionId"]
            .as_str()
            .filter(|s| !s.trim().is_empty())?;
        if !ids.insert(id) {
            return None;
        }
    }
    let question = request["toolCall"]["title"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Choose an option.");
    serde_json::from_value(json!({"id":id,"header":"Question","question":bounded(question,8_000),
        "multiSelect":false,"allowCustomAnswer":false,
        "options":options.iter().map(|option| json!({"value":option["optionId"],
            "label":question_label(option),"description":question_label(option)})).collect::<Vec<_>>()
    })).ok()
}

pub fn antigravity_user_input_response(request: &Value, answers: &Value) -> Option<Value> {
    antigravity_user_input_question(request)?;
    let id = request["toolCall"]["toolCallId"].as_str()?;
    let value = match &answers[id] {
        Value::String(s) => s.as_str(),
        Value::Array(values) if values.len() == 1 => values[0].as_str()?,
        _ => return None,
    };
    let options = request["options"].as_array()?;
    let exact = options.iter().find(|option| option["optionId"] == value);
    let selected = exact.or_else(|| {
        let mut labels = options
            .iter()
            .filter(|option| question_label(option) == value);
        let first = labels.next()?;
        labels.next().is_none().then_some(first)
    })?;
    Some(json!({"outcome":{"outcome":"selected","optionId":selected["optionId"]}}))
}

pub fn antigravity_subagent_update(tool: &Value) -> Option<Value> {
    if tool.get("kind").is_some_and(|kind| kind != "other")
        || !matches!(
            tool["title"].as_str(),
            Some(
                "Running start_subagent"
                    | "Run start_subagent?"
                    | "Running invoke_subagent"
                    | "Run invoke_subagent?"
            )
        )
        || tool["data"]["meta"]["is_mcp_tool_call"] == true
    {
        return None;
    }
    let launches = tool["data"]["rawInput"]["Subagents"]
        .as_array()
        .filter(|v| !v.is_empty())
        .and_then(|entries| {
            entries
                .iter()
                .map(|entry| Some((entry["Role"].as_str()?, entry["Prompt"].as_str()?)))
                .collect::<Option<Vec<_>>>()
        });
    let output = tool["data"]["rawOutput"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| bounded(s, 8_000));
    let (title, prompt) = if let Some(launches) = launches {
        if let [(title, prompt)] = launches.as_slice() {
            (title.to_string(), prompt.to_string())
        } else {
            (
                launches
                    .iter()
                    .map(|(role, _)| *role)
                    .collect::<Vec<_>>()
                    .join(", "),
                launches
                    .iter()
                    .map(|(role, prompt)| format!("{role}\n{prompt}"))
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            )
        }
    } else {
        (
            ANTIGRAVITY_SUBAGENT_BATCH_TITLE.into(),
            output
                .clone()
                .or_else(|| tool["detail"].as_str().map(str::to_owned))
                .or_else(|| tool["title"].as_str().map(str::to_owned))
                .unwrap_or_else(|| ANTIGRAVITY_SUBAGENT_BATCH_TITLE.into()),
        )
    };
    let status = match tool["status"].as_str() {
        Some("failed") => "failed",
        Some("pending") => "pending",
        _ => "running",
    };
    Some(
        json!({"nativeTaskId":tool["toolCallId"],"prompt":prompt,"title":title,"model":null,
        "status":status,"childSessionId":null,"result":if status == "failed" { json!(output) } else { Value::Null }}),
    )
}

#[derive(Default)]
pub struct AntigravityReadRepair {
    open: Vec<(String, String, String, Option<bool>)>,
    early: VecDeque<(String, bool)>,
}
impl AntigravityReadRepair {
    pub fn reset(&mut self) {
        self.open.clear();
        self.early.clear();
    }

    pub fn attempted(&mut self, path: &str, served: bool) -> Option<(String, String)> {
        if let Some((id, session, _, outcome)) = self
            .open
            .iter_mut()
            .find(|(_, _, candidate, outcome)| candidate == path && outcome.is_none())
        {
            *outcome = Some(served);
            return served.then(|| (session.clone(), id.clone()));
        }
        self.early.push_back((path.into(), served));
        None
    }

    pub fn repair(&mut self, mut notification: Value) -> Value {
        let session = notification["sessionId"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let update = &mut notification["update"];
        if !matches!(
            update["sessionUpdate"].as_str(),
            Some("tool_call" | "tool_call_update")
        ) {
            return notification;
        }
        let Some(id) = update["toolCallId"].as_str().map(str::to_owned) else {
            return notification;
        };
        let path = matches!(
            update["title"].as_str(),
            Some("Running client_view_file" | "Run client_view_file?")
        )
        .then(|| {
            update["locations"][0]["path"]
                .as_str()
                .or_else(|| update["rawInput"]["absolute_path"].as_str())
        })
        .flatten()
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
        if let Some(path) = path
            && !self.open.iter().any(|(key, _, _, _)| key == &id)
        {
            let early = self
                .early
                .iter()
                .position(|(candidate, _)| candidate == &path)
                .and_then(|index| self.early.remove(index))
                .map(|(_, outcome)| outcome);
            self.open.push((id.clone(), session, path, early));
        }
        let index = self.open.iter().position(|(key, _, _, _)| key == &id);
        let served = index.is_some_and(|index| self.open[index].3 == Some(true));
        match update["status"].as_str() {
            Some("pending" | "in_progress") if served => update["status"] = json!("completed"),
            Some("completed" | "failed") => {
                let failed = update["status"] == "failed";
                if let Some(index) = index {
                    self.open.remove(index);
                }
                if failed && served {
                    update["status"] = json!("completed");
                    if let Some(update) = update.as_object_mut() {
                        update.remove("rawOutput");
                    }
                }
            }
            _ => {}
        }
        notification
    }
}

pub fn antigravity_approval_options(
    request: &Value,
) -> ProviderResult<Vec<t3_contracts::provider_policy::ProviderApprovalOption>> {
    if request["toolCall"]["toolCallId"]
        .as_str()
        .is_some_and(|id| id.starts_with("interaction_"))
    {
        return Ok(vec![]);
    }
    let mut options = Vec::new();
    for (kind, decision, label) in [
        ("allow_once", "accept", "Allow once"),
        ("allow_always", "acceptForSession", "Allow for this thread"),
        ("reject_once", "decline", "Deny"),
    ] {
        let found = request["options"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|option| {
                option["kind"] == kind
                    && option["optionId"]
                        .as_str()
                        .is_some_and(|id| !id.trim().is_empty())
            });
        if let Some(option) = found {
            let mut value = json!({"decision":decision,"label":label});
            if kind == "allow_always" {
                let warning = &option["_meta"]["agy.security.warning"];
                if let Some(warning) = warning["message"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        warning["title"]
                            .as_str()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                    })
                {
                    value["warning"] = json!(bounded(warning, 512));
                }
            }
            options.push(value);
        }
    }
    options.push(json!({"decision":"cancel","label":"Cancel"}));
    serde_json::from_value(json!(options)).map_err(|_| ProviderError::Protocol {
        reason: None,
        detail: "Invalid Antigravity approval options".into(),
        payload: None,
    })
}
