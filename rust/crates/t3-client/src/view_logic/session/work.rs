use super::*;
use crate::{state::items, view_logic::JsTrim};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkLogEntry {
    pub id: String,
    pub created_at: String,
    pub run_id: Option<IdOrIndex<RunId>>,
    pub label: String,
    pub tone: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_files: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_lifecycle_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_activity_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_payload: Option<Rc<OrchestrationV2TurnItem>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projected_item: Option<Rc<OrchestrationV2ProjectedTurnItem>>,
    #[serde(flatten)]
    pub presentation: BTreeMap<String, Value>,
}
pub fn work_log_entry_is_tool_like(entry: &WorkLogEntry) -> bool {
    matches!(entry.tone.as_str(), "tool" | "thinking" | "error")
        || entry.command.is_some()
        || entry.request_kind.is_some()
}
pub fn work_entry_signals_severe_failure(entry: &WorkLogEntry) -> bool {
    entry.item_type.as_deref() == Some("error")
}
pub fn work_entry_indicates_tool_success(entry: &WorkLogEntry) -> bool {
    work_log_entry_is_tool_like(entry)
        && !work_entry_indicates_tool_failure(entry)
        && (entry.tone != "thinking" || entry.item_type.as_deref() == Some("reasoning"))
        && !matches!(
            entry.tool_lifecycle_status.as_deref(),
            Some("failed" | "declined" | "inProgress" | "stopped" | "idle")
        )
}
pub fn work_entry_indicates_tool_neutral_status(entry: &WorkLogEntry) -> bool {
    work_log_entry_is_tool_like(entry)
        && !work_entry_indicates_tool_failure(entry)
        && !work_entry_indicates_tool_success(entry)
}
pub fn work_entry_indicates_tool_failure(entry: &WorkLogEntry) -> bool {
    indicates_failure(entry, true)
}
pub fn work_entry_display_indicates_tool_failure(entry: &WorkLogEntry) -> bool {
    indicates_failure(entry, false)
}
fn indicates_failure(entry: &WorkLogEntry, include_command: bool) -> bool {
    crate::work_log::entry_indicates_failure(entry, include_command)
}
pub fn tool_output_indicates_failure(text: &str) -> bool {
    let text = text.to_lowercase();
    if [
        "file not found",
        "no files found",
        "enoent",
        "no such file",
        "commandnotfoundexception",
        "command not found",
        "is not recognized as the name of a cmdlet",
        "a parameter cannot be found that matches parameter name",
    ]
    .iter()
    .any(|p| text.contains(p))
    {
        return true;
    }
    if (text.contains("cannot find path") && text.contains("because it does not exist"))
        || (text.contains("is not recognized") && text.contains("the term '"))
    {
        return true;
    }
    text.match_indices("exit code").any(|(i, _)| {
        let tail = &text[i + 9..];
        if (text[..i].ends_with("exit with ") || text[..i].ends_with("exited with "))
            && tail.starts_with(crate::view_logic::is_js_whitespace)
            && tail
                .js_trim_start()
                .starts_with(|c: char| matches!(c, '1'..='9'))
        {
            return true;
        }
        if !tail.starts_with(|c: char| crate::view_logic::is_js_whitespace(c) || c == ':') {
            return false;
        }
        let tail = tail.js_trim_start();
        let tail = tail.strip_prefix(':').unwrap_or(tail).js_trim_start();
        let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
        tail.starts_with(|c: char| matches!(c, '1'..='9'))
            && tail[digits..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
    })
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderErrorPresentation {
    pub label: String,
    pub detail: String,
}
pub fn provider_error_presentation(item: &TurnItemError) -> ProviderErrorPresentation {
    let failure = wire_value(&item.failure);
    let limit = failure["class"] == "usage_limit";
    let base = if limit {
        "Usage limit reached"
    } else {
        "Provider error"
    };
    let message = failure["message"].as_str().unwrap_or_default();
    let Some(retry) = item.retry.as_ref().and_then(Option::as_ref) else {
        return ProviderErrorPresentation {
            label: if limit {
                base.into()
            } else {
                item.title
                    .as_deref()
                    .map(str::js_trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(base)
                    .into()
            },
            detail: message.into(),
        };
    };
    let progress = retry.max_attempts.map_or_else(
        || retry.attempt.to_string(),
        |max| format!("{}/{max}", retry.attempt),
    );
    let label = match item.status {
        OrchestrationV2TurnItemStatus::Running => format!("Retrying provider ({progress})"),
        OrchestrationV2TurnItemStatus::Completed => {
            format!("Provider recovered ({progress} retries)")
        }
        OrchestrationV2TurnItemStatus::Failed => format!("{base} after {progress} retries"),
        _ => format!("Provider retry stopped ({progress})"),
    };
    let mut detail = message.to_string();
    if item.status == OrchestrationV2TurnItemStatus::Running
        && let Some(ms) = retry.retry_delay_ms.filter(|ms| *ms > 0)
    {
        let delay = if ms < 1000 {
            format!("{ms}ms")
        } else {
            format!(
                "{}s",
                format!("{:.1}", ms as f64 / 1000.).trim_end_matches(".0")
            )
        };
        detail.push_str(&format!(" Retrying in {delay}."));
    }
    ProviderErrorPresentation { label, detail }
}
pub(crate) fn wire_value(value: &impl Serialize) -> Value {
    // Contract serializers contain only JSON values, finite numbers, and string keys.
    serde_json::to_value(value).expect("contract serializes to JSON")
}
pub(crate) fn id_key<T: AsRef<str>>(id: &IdOrIndex<T>) -> String {
    match id {
        IdOrIndex::Id(id) => id.as_ref().into(),
        IdOrIndex::Index(index) => format!("#{index}"),
    }
}
pub(crate) fn item_value(item: &OrchestrationV2TurnItem) -> Value {
    wire_value(item)
}
pub(crate) fn projected_created_at(row: &OrchestrationV2ProjectedTurnItem) -> String {
    macro_rules! time {($($variant:ident),*) => {match &row.item {$(OrchestrationV2TurnItem::$variant(v) => v.started_at.as_ref().unwrap_or(&v.updated_at).0.clone(),)*}};}
    time!(
        Notification,
        UserMessage,
        AssistantMessage,
        Reasoning,
        ProposedPlan,
        TodoList,
        UserInputRequest,
        FileChange,
        CommandExecution,
        FileSearch,
        WebSearch,
        ApprovalRequest,
        Checkpoint,
        RunInterruptRequest,
        RunInterruptResult,
        SystemNotice,
        Error,
        Compaction,
        Handoff,
        Fork,
        ThreadCreated,
        SecretRequest,
        LinkRequest,
        Subagent,
        DynamicTool
    )
}
pub(crate) fn projected_work_entry(row: Rc<OrchestrationV2ProjectedTurnItem>) -> WorkLogEntry {
    let item = &row.item;
    let value = item_value(item);
    let kind = value["type"].as_str().unwrap_or_default();
    let title = value["title"]
        .as_str()
        .map(str::js_trim)
        .filter(|s| !s.is_empty());
    let status = match items::status(item) {
        OrchestrationV2TurnItemStatus::Pending
        | OrchestrationV2TurnItemStatus::Running
        | OrchestrationV2TurnItemStatus::Waiting => "inProgress",
        OrchestrationV2TurnItemStatus::Completed => "completed",
        OrchestrationV2TurnItemStatus::Idle => "idle",
        OrchestrationV2TurnItemStatus::Failed => "failed",
        OrchestrationV2TurnItemStatus::Cancelled | OrchestrationV2TurnItemStatus::Interrupted => {
            "stopped"
        }
    };
    let tone = match kind {
        "reasoning" => "thinking",
        "command_execution" | "file_change" | "file_search" | "web_search" | "dynamic_tool"
        | "subagent" | "thread_created" | "user_input_request" | "approval_request" => "tool",
        _ => "info",
    };
    let mut entry = WorkLogEntry {
        id: id_key(items::id(item)),
        created_at: projected_created_at(&row),
        run_id: items::run_id(item).clone(),
        tone: tone.into(),
        item_type: Some(kind.into()),
        tool_lifecycle_status: Some(status.into()),
        structured_payload: Some(Rc::new(item.clone())),
        projected_item: Some(row.clone()),
        ..Default::default()
    };
    entry.presentation = crate::work_log::extract_tool_activity_presentation(&value);
    match item {
        OrchestrationV2TurnItem::Notification(v) => {
            entry.label = v.summary.0.clone();
            entry.tool_lifecycle_status = None;
        }
        OrchestrationV2TurnItem::ThreadCreated(_) => entry.label = "Created thread".into(),
        OrchestrationV2TurnItem::Compaction(_) => {
            entry.label = context_compaction_label(&value);
            entry.source_activity_kind = Some("context-compaction".into());
            entry.detail = value["summary"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string);
        }
        OrchestrationV2TurnItem::Reasoning(v) => {
            entry.label = title.unwrap_or("Thinking").into();
            entry.detail = (!v.text.is_empty()).then(|| v.text.clone());
        }
        OrchestrationV2TurnItem::CommandExecution(v) => {
            entry.label = title.unwrap_or("Ran command").into();
            entry.command = Some(v.input.clone());
            entry.raw_command = Some(v.input.clone());
            entry.tool_title = Some(title.unwrap_or("Command").into());
            entry.tool_data = Some(value.clone());
        }
        OrchestrationV2TurnItem::FileChange(_) => {
            let changes = value["changes"].as_array();
            entry.label = title.map(str::to_string).unwrap_or_else(|| {
                if changes.is_some_and(|c| c.len() > 1) {
                    format!("Changed {} files", changes.map_or(0, Vec::len))
                } else {
                    format!("Changed {}", value["fileName"].as_str().unwrap_or_default())
                }
            });
            entry.changed_files = Some(changes.map_or_else(
                || vec![value["fileName"].as_str().unwrap_or_default().into()],
                |c| {
                    c.iter()
                        .filter_map(|c| c["path"].as_str().map(str::to_string))
                        .collect()
                },
            ));
            entry.tool_title = Some(title.unwrap_or("File change").into());
            entry.tool_data = Some(value.clone());
        }
        OrchestrationV2TurnItem::FileSearch(_) => {
            entry.label = title
                .map(str::to_string)
                .or_else(|| search_label(&value))
                .unwrap_or_else(|| "Searched files".into());
            entry.detail = value["pattern"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            entry.tool_title = Some(title.unwrap_or("File search").into());
            entry.tool_data = Some(value.clone());
        }
        OrchestrationV2TurnItem::WebSearch(_) => {
            entry.label = title.unwrap_or("Searched the web").into();
            entry.detail = value["patterns"]
                .as_array()
                .filter(|p| !p.is_empty())
                .map(|p| {
                    p.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                });
            entry.tool_title = Some(title.unwrap_or("Web search").into());
            entry.tool_data = Some(value.clone());
        }
        OrchestrationV2TurnItem::SystemNotice(_) => {
            entry.label = value["message"].as_str().unwrap_or_default().into();
            entry.source_activity_kind = Some("runtime.warning".into());
        }
        OrchestrationV2TurnItem::Error(v) => {
            let p = provider_error_presentation(v);
            entry.label = p.label;
            entry.detail = Some(p.detail);
            entry.source_activity_kind =
                if value["failure"]["class"] == "usage_limit" && status != "completed" {
                    Some("runtime.warning".into())
                } else if v.retry.is_none() {
                    Some("runtime.error".into())
                } else {
                    None
                };
            entry.tool_data = Some(value.clone());
        }
        OrchestrationV2TurnItem::DynamicTool(_) => {
            let name = value["toolName"].as_str();
            let token = name
                .filter(|s| !s.contains(['.', '/']) && !s.contains("__"))
                .unwrap_or_default()
                .replace(['_', ' ', '-'], "")
                .to_lowercase();
            entry.label = title
                .map(str::to_string)
                .unwrap_or_else(|| match token.as_str() {
                    "read" | "readfile" => {
                        format!("Read {}", first_path(&value["input"]).unwrap_or("file"))
                    }
                    "find" | "grep" | "glob" | "rg" | "ls" => {
                        search_label(&value).unwrap_or_else(|| name.unwrap_or("Tool call").into())
                    }
                    _ => name.unwrap_or("Tool call").into(),
                });
            entry.tool_title = Some(title.or(name).unwrap_or("Tool").into());
            let mut data = serde_json::Map::new();
            for key in ["input", "output"] {
                if let Some(v) = value.get(key) {
                    data.insert(key.into(), v.clone());
                }
            }
            entry.tool_data = Some(Value::Object(data));
        }
        OrchestrationV2TurnItem::TodoList(_) => {
            let steps = value["steps"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default();
            let done = steps.iter().filter(|s| s["status"] == "completed").count();
            let current = steps
                .iter()
                .find(|s| s["status"] == "running")
                .or_else(|| steps.iter().find(|s| s["status"] == "pending"));
            entry.label = format!(
                "Tasks {done}/{}{}",
                steps.len(),
                current.map_or_else(String::new, |s| format!(
                    " · {}",
                    s["text"].as_str().unwrap_or_default()
                ))
            );
        }
        OrchestrationV2TurnItem::ApprovalRequest(_) => {
            entry.label = title.unwrap_or("Approval requested").into();
            entry.detail = value["prompt"]
                .as_str()
                .or_else(|| value["requestKind"].as_str())
                .map(str::to_string);
            entry.tool_data = Some(value.clone());
        }
        OrchestrationV2TurnItem::UserInputRequest(_) => {
            entry.label = title
                .unwrap_or(
                    if value.get("questionAnswer").is_some_and(|v| !v.is_null()) {
                        "Answered questions"
                    } else {
                        "Input requested"
                    },
                )
                .into();
            if let Some(a) = value.get("questionAnswer") {
                entry
                    .presentation
                    .insert("questionAnswer".into(), a.clone());
            }
            entry.tool_data = Some(value.clone());
        }
        _ => {
            entry.label = title
                .map(str::to_string)
                .unwrap_or_else(|| kind.replace('_', " "));
            entry.tool_data = Some(value.clone());
        }
    }
    entry
}
fn first_path(value: &Value) -> Option<&str> {
    [
        "path",
        "filePath",
        "file_path",
        "fileName",
        "filename",
        "target",
    ]
    .iter()
    .find_map(|k| value[*k].as_str().filter(|s| !s.js_trim().is_empty()))
}
fn search_label(value: &Value) -> Option<String> {
    let input = value
        .get("input")
        .filter(|v| v.is_object())
        .unwrap_or(value);
    let first = |keys: &[&str]| {
        keys.iter().find_map(|k| {
            input[*k]
                .as_str()
                .map(str::js_trim)
                .filter(|s| !s.is_empty())
        })
    };
    let query = first(&["pattern", "query", "searchTerm", "regex", "grep", "needle"]);
    let glob = first(&[
        "glob",
        "globPattern",
        "glob_pattern",
        "include",
        "filePattern",
        "file_pattern",
    ]);
    let target = first(&[
        "path",
        "target_directory",
        "targetDirectory",
        "directory",
        "cwd",
        "root",
    ]);
    match (query, glob, target) {
        (Some(q), _, Some(t)) => Some(format!("Searched {q} in {t}")),
        (_, Some(g), Some(t)) => Some(format!("Searched files {g} in {t}")),
        (_, Some(g), _) => Some(format!("Searched files {g}")),
        (Some(q), _, _) => Some(format!("Searched {q}")),
        (_, _, Some(t)) => Some(format!("Searched in {t}")),
        _ => None,
    }
}
pub fn context_compaction_label(item: &Value) -> String {
    match item["status"].as_str() {
        Some("failed") => "Context compaction failed".into(),
        Some("cancelled" | "interrupted") => "Context compaction stopped".into(),
        Some("pending" | "running" | "waiting") => "Compacting context".into(),
        _ => match (
            item["beforeTokenCount"].as_f64(),
            item["afterTokenCount"].as_f64(),
        ) {
            (Some(a), Some(b)) => format!(
                "Context compacted {} → {} tokens",
                format_tokens(a),
                format_tokens(b)
            ),
            _ => "Context compacted".into(),
        },
    }
}
fn format_tokens(n: f64) -> String {
    if n >= 1_000_000. {
        format!("{:.1}m", n / 1_000_000.)
    } else if n >= 1_000. {
        format!("{:.1}k", n / 1_000.)
    } else {
        format!("{n}")
    }
}
