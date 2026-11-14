use crate::view_logic::{JsTrim, session::WorkLogEntry};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
struct ToolDefinition {
    labels: [&'static str; 4],
    icon: &'static str,
    action: &'static str,
}
include!("work_log_tool_tables.rs");
pub fn normalize_compact_tool_label(value: &str) -> &str {
    let trimmed = value.js_trim_end();
    let lower = trimmed.to_ascii_lowercase();
    for suffix in [" completed", " complete"] {
        if lower.ends_with(suffix) {
            return trimmed[..trimmed.len() - suffix.len()].js_trim();
        }
    }
    value.js_trim()
}
pub(crate) fn tool_id(value: &str) -> Option<String> {
    let label = normalize_compact_tool_label(value);
    let lower = label.to_ascii_lowercase();
    let candidate = if lower.starts_with("mcp__") {
        let (server, tool) = label[5..].split_once("__")?;
        if ![
            "t3-code",
            "t3_code",
            "t3code",
            "t3-code-direct",
            "t3_code_direct",
        ]
        .contains(&server.to_ascii_lowercase().as_str())
        {
            return None;
        }
        tool
    } else {
        let prefixes = [
            "t3-code-direct",
            "t3_code_direct",
            "t3-code",
            "t3_code",
            "t3code",
        ];
        let mut found = None;
        for prefix in prefixes {
            if lower.starts_with(prefix) {
                let rest = &label[prefix.len()..];
                let tail = rest.js_trim_start();
                if let Some(tail) = tail.strip_prefix('·') {
                    found = Some(tail.js_trim_start());
                    break;
                }
                if rest.starts_with(['.', ':', '/', '_', '-', ' ']) {
                    found = Some(rest.trim_start_matches(['.', ':', '/', '_', '-', ' ']));
                    break;
                }
            }
        }
        found.unwrap_or(label)
    };
    tool_definition(candidate).map(|_| candidate.to_string())
}
fn resolve_definition(name: &str, input: &Value) -> Option<ToolDefinition> {
    let name = tool_id(name)?;
    let action = input["action"].as_str();
    let action_name = action.map(|action| match name.as_str() {
        "preview_act" => format!("preview_{action}"),
        "t3_queue" => format!("t3_queue_{action}"),
        "t3_pending_request" => format!("t3_pending_request_{action}"),
        "t3_thread_transfer" => match action {
            "fork" => "t3_thread_fork",
            "merge_back" => "t3_thread_merge_back",
            _ => "t3_thread_transfers",
        }
        .into(),
        _ => String::new(),
    });
    action_name
        .as_deref()
        .and_then(tool_definition)
        .or_else(|| tool_definition(&name))
}
fn field<'a>(entry: &'a WorkLogEntry, key: &str) -> Option<&'a str> {
    entry.presentation.get(key).and_then(Value::as_str)
}
fn tool_name(entry: &WorkLogEntry) -> String {
    if let Some(t3_contracts::orchestration_v2::OrchestrationV2TurnItem::DynamicTool(item)) =
        entry.structured_payload.as_deref()
        && let Some(name) = &item.tool_name
    {
        return name.0.clone();
    }
    let data = entry.tool_data.as_ref().unwrap_or(&Value::Null);
    if let (Some(server), Some(tool)) = (data["server"].as_str(), data["tool"].as_str()) {
        return format!("{server}.{tool}");
    }
    if let Some(name) = data["toolName"].as_str() {
        return name.into();
    }
    entry
        .tool_title
        .as_deref()
        .filter(|s| tool_id(s).is_some())
        .unwrap_or(&entry.label)
        .into()
}
fn tool_input(entry: &WorkLogEntry) -> &Value {
    if let Some(t3_contracts::orchestration_v2::OrchestrationV2TurnItem::DynamicTool(item)) =
        entry.structured_payload.as_deref()
    {
        return &item.input;
    }
    entry
        .tool_data
        .as_ref()
        .and_then(|d| {
            ["arguments", "input", "rawInput"]
                .iter()
                .find_map(|k| d.get(k).filter(|v| !v.is_null()))
        })
        .unwrap_or(&Value::Null)
}
fn tool_output(entry: &WorkLogEntry) -> &Value {
    if let Some(t3_contracts::orchestration_v2::OrchestrationV2TurnItem::DynamicTool(item)) =
        entry.structured_payload.as_deref()
    {
        return item
            .output
            .as_ref()
            .and_then(Option::as_ref)
            .unwrap_or(&Value::Null);
    }
    entry
        .tool_data
        .as_ref()
        .and_then(|d| {
            ["output", "result", "rawOutput", "content"]
                .iter()
                .find_map(|k| d.get(k).filter(|v| !v.is_null()))
        })
        .unwrap_or(&Value::Null)
}
pub fn t3_tool_result_indicates_failure(value: &Value) -> bool {
    result_failed(value, 0)
}
fn result_failed(value: &Value, depth: usize) -> bool {
    if depth > 4 {
        return false;
    }
    if let Some(text) = value.as_str() {
        return serde_json::from_str::<Value>(text)
            .is_ok_and(|value| result_failed(&value, depth + 1));
    }
    if let Some(blocks) = value.as_array() {
        return blocks.iter().any(|block| {
            let text = block
                .get("text")
                .or_else(|| block.get("content").and_then(|c| c.get("text")))
                .unwrap_or(&Value::Null);
            result_failed(text.get("text").unwrap_or(text), depth + 1)
        });
    }
    value["isError"] == true
        || value["is_error"] == true
        || value["_tag"]
            .as_str()
            .is_some_and(|s| s.ends_with("Error") || s.ends_with("Failure"))
        || value.get("error").is_some_and(|v| !v.is_null())
        || value
            .get("structuredContent")
            .filter(|v| !v.is_null())
            .or_else(|| value.get("content"))
            .is_some_and(|content| result_failed(content, depth + 1))
}
fn read_result(value: &Value, depth: usize) -> (Option<Value>, bool) {
    if depth > 4 {
        return (None, false);
    }
    if let Some(text) = value.as_str() {
        return serde_json::from_str::<Value>(text)
            .map_or((None, false), |v| read_result(&v, depth + 1));
    }
    if let Some(blocks) = value.as_array() {
        let mut data = None;
        let mut failed = false;
        for block in blocks {
            let text = block
                .get("text")
                .or_else(|| block.get("content").and_then(|c| c.get("text")))
                .unwrap_or(&Value::Null);
            let (nested, error) = read_result(text.get("text").unwrap_or(text), depth + 1);
            if data.is_none() {
                data = nested;
            }
            failed |= error;
        }
        return (data, failed);
    }
    if !value.is_object() {
        return (None, false);
    }
    let failed = value["isError"] == true
        || value["is_error"] == true
        || value["_tag"]
            .as_str()
            .is_some_and(|s| s.ends_with("Error") || s.ends_with("Failure"))
        || value.get("error").is_some_and(|v| !v.is_null());
    if let Some(content) = value
        .get("structuredContent")
        .filter(|v| !v.is_null())
        .or_else(|| value.get("content"))
    {
        let (data, error) = read_result(content, depth + 1);
        return (data, failed || error);
    }
    (Some(value.clone()), failed)
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkEntryToolPresentation {
    pub display_name: String,
    pub icon: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}
pub fn resolve_work_entry_tool_presentation(
    entry: &WorkLogEntry,
    fallback: Option<&str>,
) -> Option<WorkEntryToolPresentation> {
    let definition = resolve_definition(&tool_name(entry), tool_input(entry))?;
    let status = if t3_tool_result_indicates_failure(tool_output(entry)) {
        Some("failed")
    } else {
        entry.tool_lifecycle_status.as_deref().or(fallback)
    };
    let [action, running, completed, detail] = definition.labels;
    let verb = match status {
        Some("completed") => completed.into(),
        Some("failed") => format!("Failed to {}", action.to_lowercase()),
        Some("declined") => format!("Declined to {}", action.to_lowercase()),
        Some("stopped") => format!("Stopped {}", running.to_lowercase()),
        _ => running.into(),
    };
    let pr = matches!(
        definition.action,
        "link-pr" | "unlink-pr" | "list-prs" | "watch-pr" | "unwatch-pr"
    );
    let input = tool_input(entry);
    let url = input["url"].as_str().and_then(|s| url::Url::parse(s).ok());
    let url_number = url.as_ref().and_then(|u| {
        u.path_segments()
            .and_then(|mut p| p.next_back())
            .and_then(|s| s.parse::<u64>().ok())
    });
    let number = url_number
        .or_else(|| input["number"].as_u64())
        .filter(|n| *n > 0 && *n <= 9_007_199_254_740_991);
    let target = if pr && definition.action != "list-prs" && number.is_some() {
        format!(
            "{} #{}",
            if url
                .as_ref()
                .is_some_and(|u| u.path().contains("/merge_requests/"))
            {
                "MR"
            } else {
                "PR"
            },
            number.unwrap_or_default()
        )
    } else {
        detail.into()
    };
    Some(WorkEntryToolPresentation {
        display_name: format!("{verb} {target}"),
        icon: definition.icon.into(),
        action: pr.then(|| definition.action.into()),
    })
}
pub fn live_activity_tool_status(status: Option<&str>, present: bool) -> &str {
    match status {
        Some(s @ ("failed" | "declined" | "stopped" | "idle")) => s,
        _ if present || status == Some("inProgress") => "inProgress",
        _ => "completed",
    }
}
pub fn presentation_entry_is_tool_like(entry: &WorkLogEntry) -> bool {
    matches!(entry.tone.as_str(), "tool" | "thinking" | "error")
        || entry
            .command
            .as_ref()
            .is_some_and(|s| !s.js_trim().is_empty())
        || entry.request_kind.is_some()
        || matches!(
            entry.item_type.as_deref(),
            Some(
                "command_execution"
                    | "file_change"
                    | "file_search"
                    | "web_search"
                    | "dynamic_tool"
                    | "subagent"
                    | "thread_created"
            )
        )
}
pub fn entry_indicates_failure(entry: &WorkLogEntry, include_command: bool) -> bool {
    if entry.tone == "error"
        || matches!(
            entry.tool_lifecycle_status.as_deref(),
            Some("failed" | "declined")
        )
    {
        return true;
    }
    if !presentation_entry_is_tool_like(entry) || entry.item_type.as_deref() == Some("reasoning") {
        return false;
    }
    if tool_id(&tool_name(entry)).is_some() && t3_tool_result_indicates_failure(tool_output(entry))
    {
        return true;
    }
    if let Some(t3_contracts::orchestration_v2::OrchestrationV2TurnItem::CommandExecution(item)) =
        entry.structured_payload.as_deref()
    {
        if let Some(exit) = item.exit_code {
            return exit != Some(0);
        }
        if item.output_indicates_failure == Some(Some(true)) {
            return true;
        }
        if item
            .output
            .as_ref()
            .and_then(Option::as_deref)
            .is_some_and(|output| {
                let mut units = 0;
                let end = output
                    .char_indices()
                    .find_map(|(index, ch)| {
                        units += ch.len_utf16();
                        (units > 32768).then_some(index)
                    })
                    .unwrap_or(output.len());
                crate::view_logic::session::tool_output_indicates_failure(&output[..end])
            })
        {
            return true;
        }
    }
    let output = if include_command {
        [entry.detail.as_deref(), entry.command.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        entry.detail.clone().unwrap_or_default()
    };
    crate::view_logic::session::tool_output_indicates_failure(&output)
}
pub fn work_entry_hidden_by_tool_call_visibility(entry: &WorkLogEntry, visibility: &str) -> bool {
    if visibility == "showAll"
        || !matches!(
            entry.item_type.as_deref(),
            Some(
                "command_execution" | "file_change" | "file_search" | "web_search" | "dynamic_tool"
            )
        )
    {
        return false;
    }
    visibility != "hideSuccessful"
        || entry.tool_lifecycle_status.as_deref() == Some("completed")
            && !entry_indicates_failure(entry, false)
}
pub fn tool_group_action(entry: &WorkLogEntry) -> &'static str {
    if entry.item_type.as_deref() == Some("thread_created") {
        return "thread-create";
    }
    if let Some(p) = resolve_work_entry_tool_presentation(entry, None) {
        match p.action.as_deref() {
            Some("link-pr") => return "link-pr",
            Some("unlink-pr") => return "unlink-pr",
            Some("list-prs") => return "list-prs",
            Some("watch-pr") => return "watch-pr",
            Some("unwatch-pr") => return "unwatch-pr",
            _ => {}
        }
        if p.icon == "browser" {
            return "browser";
        }
        if p.icon == "device" {
            return "device";
        }
    }
    if entry.request_kind.as_deref() == Some("file-read")
        || field(entry, "viewedImagePath").is_some()
    {
        return "read";
    }
    if matches!(
        entry.item_type.as_deref(),
        Some("approval_request" | "user_input_request")
    ) {
        return if presentation_entry_is_tool_like(entry) {
            "other"
        } else {
            "update"
        };
    }
    let name = tool_name(entry);
    let token = if name.contains(['.', '/']) || name.contains("__") {
        String::new()
    } else {
        name.replace(['_', ' ', '-'], "").to_lowercase()
    };
    if matches!(token.as_str(), "read" | "readfile") {
        return "read";
    }
    if entry.item_type.as_deref() == Some("file_change")
        || entry.tool_data.as_ref().is_some_and(|d| {
            matches!(
                d["kind"].as_str(),
                Some("edit" | "move" | "delete" | "write")
            )
        })
    {
        return "edit";
    }
    if entry.item_type.as_deref() == Some("command_execution")
        || entry.command.as_ref().is_some_and(|c| !c.is_empty())
        || matches!(token.as_str(), "terminal" | "bash" | "shell")
    {
        return "command";
    }
    let local = entry.item_type.as_deref() == Some("file_search")
        || entry.item_type.as_deref() == Some("web_search")
            && entry
                .tool_title
                .as_deref()
                .unwrap_or(&entry.label)
                .to_lowercase()
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .any(|s| s == "grep");
    if local || matches!(token.as_str(), "find" | "grep" | "glob" | "rg" | "ls") {
        return "code-search";
    }
    if entry.item_type.as_deref() == Some("web_search") {
        return "search";
    }
    if entry.changed_files.as_ref().is_some_and(|v| !v.is_empty()) {
        return "edit";
    }
    if presentation_entry_is_tool_like(entry) {
        "other"
    } else {
        "update"
    }
}
fn image_path(path: &str) -> bool {
    let ext = path
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    !path.is_empty()
        && !path.contains(['\r', '\n'])
        && matches!(
            ext.as_str(),
            "png"
                | "jpg"
                | "jpeg"
                | "webp"
                | "gif"
                | "bmp"
                | "ico"
                | "avif"
                | "svg"
                | "heic"
                | "heif"
                | "tif"
                | "tiff"
        )
}
pub fn work_entry_viewed_image_path(entry: &WorkLogEntry) -> Option<&str> {
    field(entry, "viewedImagePath")
        .map(str::js_trim)
        .filter(|s| image_path(s))
        .or_else(|| {
            (tool_group_action(entry) == "read")
                .then_some(entry.detail.as_deref())
                .flatten()
                .map(str::js_trim)
                .filter(|s| image_path(s))
        })
}
pub fn work_entries_viewed_image_paths(entries: &[WorkLogEntry]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    entries
        .iter()
        .filter_map(work_entry_viewed_image_path)
        .filter(|s| seen.insert(*s))
        .map(str::to_string)
        .collect()
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewedImageAsset {
    pub resource: t3_contracts::assets::AssetResource,
    pub alt: String,
    pub src_fragment: String,
}
pub fn resolve_viewed_image_asset(
    source: &str,
    thread_id: &t3_contracts::base_schemas::ThreadId,
    workspace_root: Option<&str>,
) -> Option<ViewedImageAsset> {
    use crate::view_logic::session::links::*;
    let source = source.js_trim();
    let source = source
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(source);
    let lower = source.to_ascii_lowercase();
    if source.is_empty()
        || source.starts_with(['#', '?'])
        || source.starts_with("//")
        || ["http:", "https:", "data:", "blob:"]
            .iter()
            .any(|p| lower.starts_with(p))
    {
        return None;
    }
    let (raw, fragment) = source
        .split_once('#')
        .map_or((source, ""), |(p, _)| (p, &source[p.len()..]));
    let raw = raw.split('?').next().unwrap_or_default();
    let raw = if lower.starts_with("file:") {
        parse_file_url_href(source)?.0
    } else {
        raw.into()
    };
    let path = safe_decode_uri_component(&raw);
    if path.is_empty() || path.starts_with("~/") || path.starts_with("~\\") {
        return None;
    }
    if path.starts_with("\\\\") && workspace_relative_file_path(&path, workspace_root).is_none() {
        return None;
    }
    if !is_windows_absolute(&path)
        && path.split_once(':').is_some_and(|(scheme, _)| {
            scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.'))
        })
    {
        return None;
    }
    let path = if is_absolute(&path) {
        path
    } else {
        resolve_path_link_target(&path, workspace_root.unwrap_or("."))
    };
    let path = if workspace_root.is_none() {
        path.strip_prefix("./").unwrap_or(&path).to_string()
    } else {
        path
    };
    let path = split_file_path_position(&path, "").path;
    if !image_path(&path)
        && !matches!(
            path.rsplit('.')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase()
                .as_str(),
            "mp4" | "webm" | "mov" | "m4v" | "ogv"
        )
    {
        return None;
    }
    let alt = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_string();
    Some(ViewedImageAsset {
        resource: t3_contracts::assets::AssetResource::MediaFile {
            thread_id: thread_id.clone(),
            path: t3_contracts::base_schemas::TrimmedNonEmptyString(path),
            disposition: None,
            host_only: None,
        },
        alt,
        src_fragment: fragment.into(),
    })
}
pub fn extract_tool_activity_presentation(value: &Value) -> BTreeMap<String, Value> {
    let trimmed = |value: &Value, max: usize| {
        value
            .as_str()
            .map(str::js_trim)
            .filter(|s| !s.is_empty() && s.encode_utf16().count() <= max)
            .map(str::to_string)
    };
    let image_url = |value: &Value, page: bool| {
        trimmed(value, 4096)
            .and_then(|s| url::Url::parse(&s).ok())
            .filter(|u| matches!(u.scheme(), "http" | "https") || !page && u.scheme() == "data")
            .map(|u| u.to_string())
    };
    let icon = |value: &Value| -> Option<Value> {
        match value["_tag"].as_str()? {
            "website" => {
                let mut v = json!({"_tag":"website","pageUrl":image_url(&value["pageUrl"],true)?});
                for key in ["faviconUrl", "faviconUrlDark"] {
                    if let Some(url) = image_url(&value[key], false) {
                        v[key] = json!(url);
                    }
                }
                Some(v)
            }
            "themed-logo" => {
                let mut v =
                    json!({"_tag":"themed-logo","logoUrl":image_url(&value["logoUrl"],false)?});
                if let Some(url) = image_url(&value["logoUrlDark"], false) {
                    v["logoUrlDark"] = json!(url);
                }
                Some(v)
            }
            "native-app" => {
                let app = &value["app"];
                let app = match app["_tag"].as_str()? {
                    "app-id" => {
                        let id = trimmed(&app["appId"], 512).filter(|s| {
                            s.bytes().all(|c| {
                                c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-')
                            })
                        })?;
                        json!({"_tag":"app-id","appId":id})
                    }
                    "display-name" => {
                        json!({"_tag":"display-name","displayName":trimmed(&app["displayName"],160)?})
                    }
                    _ => return None,
                };
                Some(json!({"_tag":"native-app","app":app}))
            }
            _ => None,
        }
    };
    let mut result = BTreeMap::new();
    let source = &value["toolSource"];
    if let (Some(key), Some(name), Some(kind)) = (
        trimmed(&source["key"], 512),
        trimmed(&source["name"], 160),
        source["kind"]
            .as_str()
            .filter(|s| matches!(*s, "browser" | "computer" | "integration")),
    ) {
        let mut v = json!({"key":key,"name":name,"kind":kind});
        if let Some(icon) = icon(&source["icon"]) {
            v["icon"] = icon;
        }
        result.insert("toolSource".into(), v);
    }
    let code = value["input"]["code"].as_str().unwrap_or_default();
    let name = value["toolName"].as_str().unwrap_or_default();
    let repl = matches!(
        name,
        "js" | "node_repl.js" | "mcp__node_repl__js" | "cua_repl.js"
    );
    let inferred = (value["type"] == "dynamic_tool"
        && (name == "cua_repl.js"
            || repl
                && (code.contains("sky.")
                    || code.contains("sky .")
                    || code.contains("'@oai/sky'")
                    || code.contains("\"@oai/sky\""))))
    .then_some("computer");
    let surface = value["toolSurface"]
        .as_str()
        .filter(|s| matches!(*s, "computer" | "browser"))
        .or_else(|| {
            result
                .get("toolSource")
                .and_then(|s| s["kind"].as_str())
                .filter(|s| matches!(*s, "browser" | "computer"))
        })
        .or(inferred)
        .map(str::to_string);
    if let Some(surface) = surface {
        if surface == "computer"
            && result
                .get("toolSource")
                .is_none_or(|s| s["kind"] != "computer")
        {
            result.insert(
                "toolSource".into(),
                json!({"key":"computer-use","name":"Computer Use","kind":"computer"}),
            );
        }
        result.insert("toolSurface".into(), json!(surface));
    }
    if let Some(icon) = icon(&value["toolIcon"]) {
        result.insert("toolIcon".into(), icon);
    }
    if let Some(path) = trimmed(&value["viewedImagePath"], 4096).filter(|s| image_path(s)) {
        result.insert("viewedImagePath".into(), json!(path));
    }
    result
}

#[derive(Debug, Clone)]
pub struct T3ToolSummaryCall {
    pub input: Value,
    pub output: Value,
    pub outcome: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct T3ToolSummary {
    pub label: String,
    pub failed_count: usize,
}
fn quantity(count: usize, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}
fn entity_count(ids: impl Iterator<Item = Option<String>>) -> usize {
    let mut seen = BTreeSet::new();
    let mut missing = 0;
    for id in ids {
        if let Some(id) = id {
            seen.insert(id);
        } else {
            missing += 1;
        }
    }
    seen.len() + missing
}
pub fn summarize_t3_tool_calls(action: &str, calls: &[T3ToolSummaryCall]) -> T3ToolSummary {
    let calls: Vec<_> = calls
        .iter()
        .map(|call| {
            let (input, _) = read_result(&call.input, 0);
            let input = input
                .map(|v| {
                    if v["toolName"].is_string() {
                        v["args"].clone()
                    } else {
                        v
                    }
                })
                .unwrap_or(Value::Null);
            let (output, failed) = read_result(&call.output, 0);
            (
                input,
                output.unwrap_or(Value::Null),
                if failed {
                    "failed"
                } else {
                    call.outcome.as_str()
                },
            )
        })
        .collect();
    let completed: Vec<_> = calls.iter().filter(|c| c.2 == "completed").collect();
    let mut failed_count = calls.iter().filter(|c| c.2 == "failed").count();
    let selected = if completed.is_empty() {
        calls.iter().collect::<Vec<_>>()
    } else {
        completed.clone()
    };
    let id = |value: &Value, key: &str| {
        value[key]
            .as_str()
            .filter(|s| !s.js_trim().is_empty())
            .map(str::to_string)
    };
    let ids = |key: &str| {
        selected
            .iter()
            .map(|c| {
                id(&c.1, key)
                    .or_else(|| {
                        if key == "threadId" {
                            id(&c.1["thread"], key)
                        } else if key == "projectId" {
                            id(&c.1, "id")
                        } else {
                            None
                        }
                    })
                    .or_else(|| id(&c.0, key))
            })
            .collect::<Vec<_>>()
    };
    let count = |key: &str| entity_count(ids(key).into_iter());
    let times = quantity(selected.len(), "time");
    let phrase = |past: &str, inf: &str, object: String| {
        format!(
            "{} {object}",
            if completed.is_empty() {
                format!("Tried to {inf}")
            } else {
                past.into()
            }
        )
    };
    let specification = match action {
        "delegate" => Some(("Delegated", "delegate", "taskId", "task", false)),
        "task-cancel" => Some((
            "Requested cancellation of",
            "cancel",
            "taskId",
            "task",
            false,
        )),
        "schedule-create" => Some(("Scheduled", "schedule", "scheduledTaskId", "task", false)),
        "schedule-update" => Some((
            "Updated",
            "update",
            "scheduledTaskId",
            "scheduled task",
            false,
        )),
        "schedule-delete" => Some((
            "Requested deletion of",
            "delete",
            "scheduledTaskId",
            "scheduled task",
            false,
        )),
        "thread-interrupt" => Some((
            "Requested interrupts for",
            "interrupt",
            "threadId",
            "thread",
            false,
        )),
        "workflow-stop" => Some((
            "Requested stops for",
            "stop",
            "subagentId",
            "workflow",
            false,
        )),
        "thread-update" => Some(("Updated", "update", "threadId", "thread", false)),
        "queue-read" => Some(("Read", "read", "queuedRunId", "queued message", false)),
        "queue-edit" => Some(("Edited", "edit", "queuedRunId", "queued message", false)),
        "queue-cancel" => Some((
            "Requested cancellation of",
            "cancel",
            "queuedRunId",
            "queued run",
            false,
        )),
        "queue-reorder" => Some(("Reordered", "reorder", "queuedRunId", "queued run", false)),
        "queue-steer" => Some((
            "Requested steering with",
            "steer with",
            "queuedRunId",
            "queued message",
            false,
        )),
        "question-read" => Some((
            "Read",
            "read",
            "requestId",
            "pending question request",
            false,
        )),
        "question-respond" => Some((
            "Answered",
            "answer",
            "requestId",
            "pending question request",
            false,
        )),
        "worktree-handoff" => Some((
            "Handed off to",
            "hand off to",
            "worktreePath",
            "worktree",
            false,
        )),
        "project-read" => Some(("Read", "read", "projectId", "project", false)),
        "project-create" => Some(("Registered", "register", "projectId", "project", false)),
        "project-update" => Some(("Updated", "update", "projectId", "project", false)),
        "project-delete" => Some(("Deleted", "delete", "projectId", "project", false)),
        "project-clone" => Some(("Cloned", "clone", "cwd", "repository", false)),
        "environment-unlink" => Some((
            "Forgot",
            "forget",
            "environmentId",
            "linked environment",
            false,
        )),
        "attachment-prepare" => Some((
            "Prepared",
            "prepare",
            "attachmentId",
            "attachment upload",
            false,
        )),
        "attachment-discard" => Some((
            "Discarded",
            "discard",
            "attachmentId",
            "pending attachment",
            false,
        )),
        "schedule-run" => Some(("Requested", "request", "", "scheduled task run", false)),
        "thread-fork" => Some(("Requested", "request", "", "thread fork", false)),
        "thread-merge" => Some(("Requested", "request", "", "context merge", false)),
        "secret-request" => Some(("Asked for", "ask for", "", "secret", false)),
        "environment-link" => Some(("Asked to link", "ask to link", "", "environment", false)),
        "watch-pr" => Some(("Watching", "watch", "", "pull request", false)),
        "unwatch-pr" => Some((
            "Stopped watching",
            "stop watching",
            "",
            "pull request",
            false,
        )),
        "read-pr-guide" => Some(("Read", "read", "", "review guide", false)),
        "html-preview" => Some(("Previewed", "preview", "", "HTML page", false)),
        "html-render" => Some(("Rendered", "render", "", "HTML page", false)),
        "thread-list" => Some(("Listed", "list", "", "threads", true)),
        "task-status" => Some(("Checked", "check", "", "task status", true)),
        "background-wait" => Some(("Updated", "update", "", "background waits", true)),
        "schedule-list" => Some(("Listed", "list", "", "scheduled tasks", true)),
        "thread-configuration" => Some(("Checked", "check", "", "thread configuration", true)),
        "thread-configure" => Some(("Set", "set", "", "thread model", true)),
        "thread-search" => Some(("Searched", "search", "", "threads", true)),
        "thread-transfers" => Some(("Checked", "check", "", "thread transfers", true)),
        "thread-organize" => Some(("Organized", "organize", "", "threads", true)),
        "queue-list" => Some(("Listed", "list", "", "queued messages", true)),
        "question-list" => Some(("Listed", "list", "", "pending questions", true)),
        "worktree-list" => Some(("Listed", "list", "", "workspace branches", true)),
        "worktree-status" => Some(("Checked", "check", "", "worktree status", true)),
        "project-list" => Some(("Listed", "list", "", "projects", true)),
        "environment-read" => Some(("Checked", "check", "", "environment preferences", true)),
        "environment-update" => Some(("Updated", "update", "", "environment preferences", true)),
        "environment-links" => Some(("Listed", "list", "", "linked environments", true)),
        "browser" => Some(("Used", "use", "", "browser", true)),
        "device" => Some(("Used", "use", "", "device controls", true)),
        "send-to-user" => Some(("Sent", "send", "", "message", false)),
        "capabilities" => Some(("Checked", "check", "", "orchestration capabilities", true)),
        _ => None,
    };
    let label = if let Some((past, inf, key, noun, timed)) = specification {
        let count = if key.is_empty() {
            selected.len()
        } else {
            count(key)
        };
        let object = if timed {
            format!("{noun} {times}")
        } else if noun == "repository" {
            format!(
                "{count} {}",
                if count == 1 {
                    "repository"
                } else {
                    "repositories"
                }
            )
        } else {
            quantity(count, noun)
        };
        phrase(past, inf, object)
    } else {
        match action {
            "thread-send" => {
                let messages = entity_count(selected.iter().map(|c| id(&c.1, "messageId")));
                let thread_ids = ids("threadId");
                let known = thread_ids.iter().all(Option::is_some);
                let threads = thread_ids.iter().collect::<BTreeSet<_>>().len();
                let object = if known {
                    if messages == threads && messages > 1 {
                        format!("messages to {}", quantity(threads, "thread"))
                    } else {
                        format!(
                            "{} to {}",
                            quantity(messages, "message"),
                            quantity(threads, "thread")
                        )
                    }
                } else {
                    quantity(messages, "message")
                };
                phrase("Sent", "send", object)
            }
            "thread-read" | "thread-wait" => {
                let thread_ids = ids("threadId");
                let object = if thread_ids.iter().all(Option::is_some) {
                    quantity(thread_ids.iter().collect::<BTreeSet<_>>().len(), "thread")
                } else {
                    format!("threads {times}")
                };
                if action == "thread-read" {
                    phrase("Read", "read", object)
                } else {
                    phrase("Waited on", "wait on", object)
                }
            }
            "thread-create" => {
                let mut created = BTreeSet::new();
                let known = !completed.is_empty()
                    && completed.iter().all(|c| {
                        let rows = c.1["threads"]
                            .as_array()
                            .map_or_else(|| vec![&c.1], |a| a.iter().collect());
                        rows.iter().all(|r| {
                            if r["status"] == "rolled_back" {
                                true
                            } else if let Some(id) = id(r, "threadId") {
                                created.insert(id);
                                true
                            } else {
                                false
                            }
                        })
                    });
                if known {
                    format!("Created {}", quantity(created.len(), "thread"))
                } else {
                    format!("Requested thread creation {times}")
                }
            }
            "link-pr" | "unlink-pr" => {
                let key = if action == "link-pr" {
                    "linkedCount"
                } else {
                    "unlinkedCount"
                };
                let counts = selected
                    .iter()
                    .map(|c| {
                        c.1[key].as_u64().map(|n| n as usize).or_else(|| {
                            (c.0.get("url").is_some() || c.0.get("number").is_some()).then_some(1)
                        })
                    })
                    .collect::<Option<Vec<_>>>();
                let object = counts.map_or_else(
                    || "pull requests".into(),
                    |n| quantity(n.iter().sum(), "pull request"),
                );
                if action == "link-pr" {
                    phrase("Linked", "link", object)
                } else {
                    phrase("Unlinked", "unlink", object)
                }
            }
            "list-prs" => phrase(
                "Checked",
                "check",
                format!(
                    "linked pull requests{}",
                    if selected.len() == 1 {
                        String::new()
                    } else {
                        format!(" {times}")
                    }
                ),
            ),
            "attachment-send" => {
                let mut messages = BTreeMap::new();
                for (i, c) in selected.iter().enumerate() {
                    messages.insert(
                        id(&c.1, "messageId").unwrap_or_else(|| format!("call-{i}")),
                        *c,
                    );
                }
                let counts = messages
                    .values()
                    .map(|c| {
                        c.0["attachments"]
                            .as_array()
                            .filter(|a| !a.is_empty())
                            .map(Vec::len)
                    })
                    .collect::<Option<Vec<_>>>();
                let thread_ids = ids("threadId");
                let targets = if thread_ids.iter().all(Option::is_some) {
                    format!(
                        " to {}",
                        quantity(thread_ids.iter().collect::<BTreeSet<_>>().len(), "thread")
                    )
                } else {
                    String::new()
                };
                phrase(
                    "Sent",
                    "send",
                    counts.map_or_else(
                        || format!("attachments{targets} {times}"),
                        |counts| {
                            format!("{}{targets}", quantity(counts.iter().sum(), "attachment"))
                        },
                    ),
                )
            }
            "write-pr-guides" => {
                let outcomes: Vec<_> = selected
                    .iter()
                    .flat_map(|c| {
                        c.1["guides"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|g| g["outcome"].as_str())
                    })
                    .collect();
                let started = outcomes.iter().filter(|o| **o != Some("failed")).count();
                let failed = outcomes.len() - started;
                failed_count += failed;
                if outcomes.is_empty() {
                    phrase(
                        "Started writing",
                        "start writing",
                        format!(
                            "review guides{}",
                            if selected.len() == 1 {
                                String::new()
                            } else {
                                format!(" {times}")
                            }
                        ),
                    )
                } else if started == 0 {
                    format!("Could not start {}", quantity(failed, "review guide"))
                } else {
                    phrase(
                        "Started",
                        "start",
                        format!(
                            "{}{}",
                            quantity(started, "review guide"),
                            if failed > 0 {
                                format!(", {failed} failed")
                            } else {
                                String::new()
                            }
                        ),
                    )
                }
            }
            _ => String::new(),
        }
    };
    T3ToolSummary {
        label,
        failed_count,
    }
}
fn summary_action(entry: &WorkLogEntry) -> Option<&'static str> {
    let name = tool_name(entry);
    let input = tool_input(entry);
    let id = tool_id(&name)?;
    if id == "t3_thread_configure" {
        return Some(if input.get("modelSelection").is_some() {
            "thread-configure"
        } else {
            "thread-configuration"
        });
    }
    if id == "schedule_task" {
        return Some(if input.get("scheduledTaskId").is_some() {
            "schedule-update"
        } else {
            "schedule-create"
        });
    }
    resolve_definition(&name, input).map(|d| d.action)
}
fn action_priority(action: &str) -> usize {
    if matches!(
        action,
        "command"
            | "edit"
            | "delegate"
            | "task-cancel"
            | "thread-create"
            | "thread-send"
            | "thread-interrupt"
            | "workflow-stop"
            | "background-wait"
            | "schedule-create"
            | "schedule-update"
            | "schedule-delete"
            | "schedule-run"
            | "thread-configure"
            | "thread-fork"
            | "thread-merge"
            | "thread-organize"
            | "thread-update"
            | "queue-edit"
            | "queue-cancel"
            | "queue-reorder"
            | "queue-steer"
            | "question-respond"
            | "worktree-handoff"
            | "project-create"
            | "project-update"
            | "project-delete"
            | "project-clone"
            | "environment-update"
            | "attachment-prepare"
            | "attachment-discard"
            | "attachment-send"
    ) {
        0
    } else if matches!(action, "other" | "update") {
        2
    } else {
        1
    }
}
fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        _ => format!(
            "{}, and {}",
            names[..names.len() - 1].join(", "),
            names.last().map_or("", String::as_str)
        ),
    }
}
fn group_action_label(action: &str, count: usize) -> String {
    match action {
        "link-pr" => format!("Linked {}", quantity(count, "pull request")),
        "unlink-pr" => format!("Unlinked {}", quantity(count, "pull request")),
        "watch-pr" => format!("Watching {}", quantity(count, "pull request")),
        "unwatch-pr" => format!("Stopped watching {}", quantity(count, "pull request")),
        "list-prs" => {
            if count == 1 {
                "Checked linked pull requests".into()
            } else {
                format!("Checked linked pull requests {count} times")
            }
        }
        "read" => format!("Read {}", quantity(count, "file")),
        "edit" => format!("Changed {}", quantity(count, "file")),
        "command" => format!("Ran {}", quantity(count, "command")),
        "thread-create" => format!("Created {}", quantity(count, "thread")),
        "device" => format!("Used device controls {}", quantity(count, "time")),
        "browser" => format!("Used browser {}", quantity(count, "time")),
        "search" => format!("Searched the web {}", quantity(count, "time")),
        "code-search" => format!("Searched code {}", quantity(count, "time")),
        "other" => format!("Used {}", quantity(count, "tool")),
        _ => format!("Received {}", quantity(count, "update")),
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct ToolGroupSummary {
    pub summary: String,
    pub has_failure: bool,
}
pub fn summarize_tool_group(entries: &[WorkLogEntry]) -> ToolGroupSummary {
    let tools: Vec<_> = entries
        .iter()
        .filter(|e| e.item_type.as_deref() != Some("reasoning"))
        .collect();
    if !entries.is_empty() && tools.is_empty() {
        return ToolGroupSummary {
            summary: if entries.len() == 1 {
                "Thought".into()
            } else {
                format!("Thought (×{})", entries.len())
            },
            has_failure: false,
        };
    }
    let mut groups: Vec<(&str, Option<&str>, Vec<&WorkLogEntry>)> = vec![];
    let mut sources: Vec<Value> = vec![];
    let mut sourced_count = 0;
    for entry in &tools {
        let t3 = summary_action(entry);
        if let Some(source) = entry
            .presentation
            .get("toolSource")
            .filter(|_| t3.is_none())
        {
            if let Some(existing) = sources.iter_mut().find(|s| s["key"] == source["key"]) {
                *existing = source.clone();
            } else {
                sources.push(source.clone());
            }
            sourced_count += 1;
            continue;
        }
        let action = tool_group_action(entry);
        let key = t3.unwrap_or(action);
        if let Some(group) = groups.iter_mut().find(|g| g.1.unwrap_or(g.0) == key) {
            group.2.push(entry);
        } else {
            groups.push((action, t3, vec![entry]));
        }
    }
    let mut summaries: Vec<_> = groups
        .iter()
        .enumerate()
        .map(|(index, (action, t3, entries))| {
            let summary = if let Some(action) = t3 {
                summarize_t3_tool_calls(
                    action,
                    &entries
                        .iter()
                        .map(|e| T3ToolSummaryCall {
                            input: tool_input(e).clone(),
                            output: tool_output(e).clone(),
                            outcome: if e.tone == "error"
                                || matches!(
                                    e.tool_lifecycle_status.as_deref(),
                                    Some("failed" | "declined")
                                ) {
                                "failed"
                            } else if e.tool_lifecycle_status.as_deref() == Some("completed") {
                                "completed"
                            } else {
                                "unfinished"
                            }
                            .into(),
                        })
                        .collect::<Vec<_>>(),
                )
            } else {
                let count = if *action == "edit" {
                    let files: BTreeSet<_> = entries
                        .iter()
                        .flat_map(|e| e.changed_files.iter().flatten())
                        .collect();
                    files.len()
                        + entries
                            .iter()
                            .filter(|e| e.changed_files.as_ref().is_none_or(Vec::is_empty))
                            .count()
                } else {
                    entries.len()
                };
                T3ToolSummary {
                    label: group_action_label(action, count),
                    failed_count: entries
                        .iter()
                        .filter(|e| entry_indicates_failure(e, false))
                        .count(),
                }
            };
            (
                index,
                action_priority(t3.unwrap_or(action)),
                entries.len(),
                summary,
            )
        })
        .collect();
    let has_failure = summaries.iter().any(|s| s.3.failed_count > 0);
    summaries.sort_by_key(|s| (s.1, s.0));
    summaries.truncate(2);
    summaries.sort_by_key(|s| s.0);
    let remaining = tools.len() - sourced_count - summaries.iter().map(|s| s.2).sum::<usize>();
    let mut labels: Vec<_> = summaries.into_iter().map(|s| s.3.label).collect();
    if !sources.is_empty() {
        let mut seen = BTreeSet::new();
        let names: Vec<_> = sources
            .iter()
            .filter_map(|s| s["name"].as_str())
            .filter(|s| seen.insert(*s))
            .map(str::to_string)
            .collect();
        labels.insert(
            0,
            format!(
                "Used {}{}",
                join_names(&names),
                if sources.iter().all(|s| s["kind"] == "integration") {
                    if names.len() == 1 {
                        " integration"
                    } else {
                        " integrations"
                    }
                } else {
                    ""
                }
            ),
        );
    }
    if remaining > 0 {
        labels.push(format!("Performed {}", quantity(remaining, "other action")));
    }
    for label in labels.iter_mut().skip(1) {
        if let Some(first) = label.chars().next() {
            label.replace_range(..first.len_utf8(), &first.to_lowercase().to_string());
        }
    }
    ToolGroupSummary {
        summary: join_names(&labels),
        has_failure,
    }
}
pub fn tool_group_summary_kind(entries: &[WorkLogEntry]) -> &'static str {
    let tools: Vec<_> = entries
        .iter()
        .filter(|e| e.item_type.as_deref() != Some("reasoning"))
        .collect();
    if !entries.is_empty() && tools.is_empty() {
        return "reasoning";
    }
    if !tools.is_empty()
        && tools.iter().all(|e| {
            resolve_work_entry_tool_presentation(e, None).is_some_and(|p| p.icon == "pull-request")
        })
    {
        return "pull-request";
    }
    let actions: BTreeSet<_> = tools.iter().map(|e| tool_group_action(e)).collect();
    if actions.len() != 1 {
        return "mixed";
    }
    let action = actions.first().copied().unwrap_or("mixed");
    if action != "other" {
        return action;
    }
    let kinds: BTreeSet<_> = tools
        .iter()
        .map(|e| match e.item_type.as_deref() {
            Some("dynamic_tool") => "dynamic-tool",
            Some("subagent") => "agent-tool",
            _ if e.tone == "thinking" => "agent-tool",
            _ if e.tone == "tool" => "tone-tool",
            _ => "other",
        })
        .collect();
    if kinds.len() == 1 {
        kinds.first().copied().unwrap_or("mixed")
    } else {
        "mixed"
    }
}
pub fn live_thought_line(markdown: &str) -> String {
    let trimmed = markdown.js_trim_start();
    let heading = trimmed
        .strip_prefix("**")
        .and_then(|s| s.split_once("**"))
        .filter(|(head, tail)| {
            !head.contains(['*', '\r', '\n'])
                && (tail
                    .trim_start_matches([' ', '\t'])
                    .starts_with(['\r', '\n'])
                    || tail.trim_start_matches([' ', '\t']).is_empty())
        })
        .map(|(head, _)| head);
    let mut text = heading.unwrap_or(markdown).to_string();
    let mut cursor = 0;
    while let Some(open) = text[cursor..].find('[').map(|i| i + cursor) {
        let Some((end, tail)) = text[open + 1..].split_once("](") else {
            break;
        };
        let close_start = open + 1 + end.len() + 2;
        let Some(close) = tail.find(')') else {
            break;
        };
        let label = end.to_string();
        let start = if open > 0 && text.as_bytes()[open - 1] == b'!' {
            open - 1
        } else {
            open
        };
        text.replace_range(start..close_start + close + 1, &label);
        cursor = start + label.len();
    }
    text = text
        .lines()
        .map(|line| {
            let s = line.trim_start_matches([' ', '\t']);
            let count = s.bytes().take_while(|b| *b == b'#').count();
            let marker = if (1..=6).contains(&count) {
                Some(count)
            } else if s.starts_with(['-', '*', '+']) {
                Some(1)
            } else {
                let count = s.bytes().take_while(|b| b.is_ascii_digit()).count();
                (count > 0 && s[count..].starts_with('.')).then_some(count + 1)
            };
            marker
                .filter(|i| s[*i..].starts_with([' ', '\t']))
                .map_or(line, |i| s[i..].trim_start_matches([' ', '\t']))
        })
        .collect::<Vec<_>>()
        .join("\n");
    text = text.replace('`', "").replace("**", "").replace("~~", "");
    let mut plain = String::new();
    let mut iterator = text.chars().peekable();
    while let Some(ch) = iterator.next() {
        if matches!(ch, '*' | '_') {
            let rest = iterator.clone().collect::<String>();
            if let Some(end) = rest
                .find(['*', '_'])
                .filter(|end| !rest[..*end].contains('\n'))
            {
                plain.push_str(&rest[..end]);
                for _ in 0..rest[..end].chars().count() + 1 {
                    iterator.next();
                }
                continue;
            }
        }
        plain.push(ch);
    }
    let text = super::detail::collapse_whitespace(&plain);
    if heading.is_some() {
        return text;
    }
    for (index, ch) in text.char_indices() {
        if matches!(ch, '.' | '?' | '!') {
            let rest = &text[index + 1..];
            let quote = rest
                .chars()
                .next()
                .filter(|c| matches!(c, '"' | '\'' | '”' | '’' | ')'))
                .map_or(0, char::len_utf8);
            if rest[quote..].starts_with(crate::view_logic::is_js_whitespace) {
                return text[..index + 1 + quote].into();
            }
        }
    }
    text
}
