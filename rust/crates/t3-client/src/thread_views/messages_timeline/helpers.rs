use super::*;
use crate::workspace::project_ops::paths::format_workspace_relative_path;
use crate::{view_logic::JsTrim, work_log::*};

pub(super) fn tool_action(entry: &WorkLogEntry) -> &'static str {
    if entry.item_type.as_deref() == Some("thread_created") {
        return "thread-create";
    }
    if let Some(p) = super::presentation::tool_presentation(entry, None) {
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
        || entry.presentation.contains_key("viewedImagePath")
    {
        return "read";
    }
    if matches!(
        entry.item_type.as_deref(),
        Some("approval_request" | "user_input_request")
    ) {
        return if session::work_log_entry_is_tool_like(entry) {
            "other"
        } else {
            "update"
        };
    }
    let item = entry
        .structured_payload
        .as_ref()
        .map(value)
        .unwrap_or(Value::Null);
    let data = entry.tool_data.as_ref().unwrap_or(&Value::Null);
    let name = if item["type"] == "dynamic_tool" {
        item["toolName"].as_str()
    } else {
        data["toolName"].as_str().or(entry.tool_title.as_deref())
    };
    let name = name
        .or_else(|| data["item"]["tool"].as_str())
        .unwrap_or_default()
        .js_trim();
    let token = if name.contains(['.', '/']) || name.contains("__") {
        String::new()
    } else {
        name.chars()
            .filter(|c| !crate::view_logic::is_js_whitespace(*c) && !matches!(c, '_' | '-'))
            .flat_map(char::to_lowercase)
            .collect()
    };
    let kind = s(&data["kind"]).js_trim().to_lowercase();
    let classified = match entry.item_type.as_deref() {
        Some("command_execution") => "command",
        Some("file_change") => "edit",
        Some("web_search") => "search",
        _ if kind == "execute" => "command",
        _ if matches!(kind.as_str(), "edit" | "move" | "delete" | "write") => "edit",
        _ if kind == "search"
            || matches!(token.as_str(), "find" | "grep" | "glob" | "rg" | "ls") =>
        {
            "search"
        }
        _ if kind == "read" => "read",
        _ if matches!(token.as_str(), "terminal" | "bash" | "shell") => "command",
        _ if matches!(token.as_str(), "read" | "readfile") => "read",
        _ => "other",
    };
    let local = entry.item_type.as_deref() == Some("file_search")
        || (entry.item_type.as_deref() == Some("web_search")
            && entry
                .tool_title
                .as_deref()
                .unwrap_or(&entry.label)
                .to_lowercase()
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .any(|s| s == "grep"));
    match classified {
        "read" => "read",
        "edit" => "edit",
        "command" => "command",
        "search" => {
            if entry.item_type.as_deref() == Some("web_search") && !local {
                "search"
            } else {
                "code-search"
            }
        }
        _ if entry.command.as_deref().is_some_and(|c| !c.is_empty()) => "command",
        _ if local => "code-search",
        _ if entry.changed_files.as_ref().is_some_and(|p| !p.is_empty()) => "edit",
        _ if session::work_log_entry_is_tool_like(entry) => "other",
        _ => "update",
    }
}

pub(super) fn summary_entries(entries: &[&WorkLogEntry]) -> Vec<WorkLogEntry> {
    entries
        .iter()
        .map(|entry| {
            let mut next = (*entry).clone();
            let action = tool_action(entry);
            if action == tool_group_action(entry) {
                return next;
            }
            // The older shared classifier falls back to labels; this selector uses the web's tool metadata.
            let name = match action {
                "read" => "Read",
                "command" => "shell",
                "code-search" => "grep",
                _ => "timeline-unclassified",
            };
            let mut data = next
                .tool_data
                .clone()
                .filter(Value::is_object)
                .unwrap_or_else(|| json!({}));
            data["toolName"] = json!(name);
            if action == "edit" {
                data["kind"] = json!("edit");
            }
            next.tool_data = Some(data);
            if let Some(payload) = next.structured_payload.as_ref().filter(|p| {
                matches!(
                    p.as_ref(),
                    t3_contracts::orchestration_v2::OrchestrationV2TurnItem::DynamicTool(_)
                )
            }) {
                let mut payload = value(payload);
                payload["toolName"] = json!(name);
                next.structured_payload = Some(Rc::new(
                    serde_json::from_value(payload).expect("canonical tool name"),
                ));
            }
            next
        })
        .collect()
}

fn compact(text: &str) -> String {
    text.split(crate::view_logic::is_js_whitespace)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
fn raw_paths(entry: &WorkLogEntry) -> Vec<String> {
    if let Some(paths) = entry.changed_files.as_ref().filter(|p| !p.is_empty()) {
        return paths.clone();
    }
    fn collect(v: &Value, paths: &mut Vec<String>, depth: usize) {
        if depth > 4 || paths.len() >= 8 {
            return;
        }
        if let Some(values) = v.as_array() {
            for v in values {
                collect(v, paths, depth + 1);
            }
            return;
        }
        if !v.is_object() {
            return;
        }
        for key in [
            "path",
            "filePath",
            "file_path",
            "relativePath",
            "filename",
            "fileName",
            "newPath",
            "oldPath",
        ] {
            if let Some(path) = v[key].as_str().map(str::js_trim).filter(|s| !s.is_empty())
                && !paths.iter().any(|p| p == path)
            {
                paths.push(path.into());
                if paths.len() >= 8 {
                    return;
                }
            }
        }
        for key in [
            "locations",
            "item",
            "input",
            "result",
            "rawInput",
            "data",
            "changes",
        ] {
            collect(&v[key], paths, depth + 1);
        }
    }
    let mut paths = vec![];
    if let Some(item) = &entry.structured_payload {
        let v = value(item);
        if v["type"] == "dynamic_tool" {
            collect(&json!({"input":v["input"]}), &mut paths, 0);
        }
    }
    if paths.is_empty() {
        collect(
            entry.tool_data.as_ref().unwrap_or(&Value::Null),
            &mut paths,
            0,
        );
    }
    paths
}
fn dynamic_title(entry: &WorkLogEntry) -> Option<String> {
    let item = entry.structured_payload.as_ref().map(value)?;
    if item["type"] != "dynamic_tool" {
        return None;
    }
    let name = s(&item["toolName"]);
    let nonblank = |v: &Value| {
        v.as_str()
            .map(str::js_trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    if matches!(name, "js" | "cua_repl.js" | "node_repl.js") {
        let retained = nonblank(&item["title"])
            .filter(|s| !s.eq_ignore_ascii_case("js") && !s.eq_ignore_ascii_case(name));
        return nonblank(&item["input"]["title"]).or(retained).or_else(|| {
            let code = s(&item["input"]["code"]);
            let computer = name == "cua_repl.js"
                || code.contains("\"@oai/sky\"")
                || code.contains("'@oai/sky'")
                || code.match_indices("sky").any(|(i, _)| {
                    (i == 0
                        || !code[..i]
                            .chars()
                            .next_back()
                            .is_some_and(|c| c.is_alphanumeric() || c == '_'))
                        && code[i + 3..].js_trim_start().starts_with('.')
                });
            Some(
                if computer {
                    "Computer use"
                } else {
                    "Run JavaScript"
                }
                .into(),
            )
        });
    }
    if name == "Skill" {
        return nonblank(&item["input"]["skill"]).map(|n| format!("Skill: {n}"));
    }
    None
}
fn search_label(data: &Value) -> Option<String> {
    let data = [&data["rawInput"], &data["input"], &data["item"]["input"]]
        .into_iter()
        .find(|v| v.as_object().is_some_and(|o| !o.is_empty()))
        .unwrap_or(data);
    let first = |keys: &[&str]| {
        keys.iter().find_map(|k| {
            data[*k]
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
    let target = target.and_then(|s| s.rsplit(['/', '\\']).find(|s| !s.is_empty() && *s != "."));
    match (query, glob, target) {
        (Some(q), _, Some(t)) => Some(format!("Searched {q} in {t}")),
        (_, Some(g), Some(t)) => Some(format!("Searched files {g} in {t}")),
        (_, Some(g), _) => Some(format!("Searched files {g}")),
        (Some(q), _, _) => Some(format!("Searched {q}")),
        (_, _, Some(t)) => Some(format!("Searched in {t}")),
        _ => None,
    }
}
pub fn work_entry_display_label(entry: &WorkLogEntry, workspace_root: Option<&str>) -> String {
    if entry.item_type.as_deref() == Some("system_notice") {
        return entry.label.clone();
    }
    if entry.item_type.as_deref() == Some("reasoning") || entry.tone == "thinking" {
        let thought = compact(entry.detail.as_deref().unwrap_or_default());
        return if thought.is_empty() {
            entry.label.clone()
        } else {
            thought
        };
    }
    if let Some(p) = super::presentation::tool_presentation(entry, None) {
        return p.display_name;
    }
    if let Some(command) = entry.command.as_deref().filter(|s| !s.js_trim().is_empty()) {
        return command_display_text(command);
    }
    let action = tool_action(entry);
    if matches!(action, "code-search" | "search")
        && let Some(label) = if entry.item_type.as_deref() == Some("file_search") {
            Some(entry.label.clone())
        } else {
            search_label(entry.tool_data.as_ref().unwrap_or(&Value::Null))
        }
    {
        return label;
    }
    let paths = if action == "read" {
        raw_paths(entry)
    } else {
        vec![]
    };
    if let Some(first) = paths.first().filter(|s| !s.is_empty()) {
        let path = format_workspace_relative_path(first, workspace_root);
        return if paths.len() == 1 {
            format!("Read {path}")
        } else {
            format!("Read {path} +{} more", paths.len() - 1)
        };
    }
    let retry = entry.projected_item.as_ref().is_some_and(|p| {
        let v = value(&p.item);
        v["type"] == "error" && v.get("retry").is_some()
    });
    if let Some(title) = dynamic_title(entry) {
        return title;
    }
    let detail = entry.detail.as_deref().unwrap_or_default().js_trim();
    if !detail.is_empty()
        && !retry
        && action != "read"
        && !(matches!(action, "code-search" | "search") && detail.contains(['\r', '\n']))
    {
        return detail.into();
    }
    if let Some(files) = entry.changed_files.as_ref()
        && let Some(first) = files.first().filter(|s| !s.is_empty())
    {
        let path = format_workspace_relative_path(first, workspace_root);
        return if files.len() == 1 {
            path
        } else {
            format!("{path} +{} more", files.len() - 1)
        };
    }
    if action == "read" && !entry.presentation.get("viewedImagePath").is_some_and(truth) {
        return "Read file".into();
    }
    let heading = normalize_compact_tool_label(
        entry
            .tool_title
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&entry.label),
    );
    heading.chars().next().map_or_else(String::new, |c| {
        format!("{}{}", c.to_uppercase(), &heading[c.len_utf8()..])
    })
}
pub(super) fn single_tool_label(entry: &WorkLogEntry) -> String {
    if entry.item_type.as_deref() == Some("reasoning") {
        let thought = compact(entry.detail.as_deref().unwrap_or_default());
        return if thought.is_empty() {
            "Thought".into()
        } else {
            thought
        };
    }
    if let Some(p) = super::presentation::tool_presentation(entry, Some("completed")) {
        return p.display_name;
    }
    if let Some(title) = dynamic_title(entry) {
        return title;
    }
    if entry.item_type.as_deref() == Some("web_search") {
        return entry
            .tool_title
            .clone()
            .unwrap_or_else(|| "Web search".into());
    }
    work_entry_display_label(entry, None)
}
pub fn live_work_entry_label(
    entry: &WorkLogEntry,
    workspace_root: Option<&str>,
    active: bool,
) -> String {
    let status = live_activity_tool_status(entry.tool_lifecycle_status.as_deref(), active);
    if entry.item_type.as_deref() == Some("reasoning") {
        let thought = compact(entry.detail.as_deref().unwrap_or_default());
        return if !thought.is_empty() {
            thought
        } else if status == "inProgress" {
            "Thinking".into()
        } else {
            "Thought".into()
        };
    }
    let mut displayed = entry.clone();
    displayed.tool_lifecycle_status = Some(status.into());
    if let Some(p) = super::presentation::tool_presentation(&displayed, None) {
        return p.display_name;
    }
    if let Some(command) = entry
        .command
        .as_deref()
        .map(str::js_trim)
        .filter(|s| !s.is_empty())
    {
        let verb = match status {
            "inProgress" => "Running",
            "failed" => "Failed",
            "declined" => "Declined",
            "stopped" => "Stopped",
            _ => "Ran",
        };
        return format!(
            "{verb} {}",
            command_program_name(command)
                .as_deref()
                .unwrap_or("command")
        );
    }
    work_entry_display_label(entry, workspace_root)
}
pub fn work_entry_is_visible_in_group(
    entry: &WorkLogEntry,
    expanded_tool_group_entry: bool,
) -> bool {
    if entry.item_type.as_deref() == Some("reasoning") {
        return entry
            .detail
            .as_deref()
            .is_some_and(|s| !s.js_trim().is_empty());
    }
    (expanded_tool_group_entry
        && (entry.tool_lifecycle_status.as_deref() == Some("inProgress")
            || entry.source_activity_kind.as_deref() == Some("task.progress")))
        || entry.tool_lifecycle_status.as_deref() == Some("stopped")
        || !session::work_entry_indicates_tool_neutral_status(entry)
}
pub fn work_entry_read_output(
    entry: &WorkLogEntry,
    workspace_root: Option<&str>,
) -> Option<String> {
    let mut paths = vec![];
    for path in raw_paths(entry) {
        let path = path.js_trim().replace('\\', "/");
        let absolute = path.starts_with('/') || windows_absolute(&path);
        let path = if let Some(root) = workspace_root.filter(|_| !absolute) {
            format!(
                "{}/{}",
                root.replace('\\', "/").trim_end_matches('/'),
                path.strip_prefix("./")
                    .unwrap_or(&path)
                    .trim_start_matches('/')
            )
        } else {
            path
        };
        if !path.is_empty() && !paths.contains(&path) {
            paths.push(path);
        }
    }
    (!paths.is_empty()).then(|| paths.join("\n"))
}
fn windows_absolute(path: &str) -> bool {
    let b = path.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'/' | b'\\')
}
pub fn thread_read_target_id(entry: &WorkLogEntry) -> Option<String> {
    let item = entry.structured_payload.as_ref().map(value)?;
    if item["type"] != "dynamic_tool"
        || super::presentation::tool_id(s(&item["toolName"])) != Some("t3_thread_read")
    {
        return None;
    }
    item["input"]["threadId"]
        .as_str()
        .map(str::js_trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
pub fn thread_read_target_title(
    title: &str,
    archived_at: Option<&str>,
    deleted_at: Option<&str>,
) -> Option<String> {
    (archived_at.is_none() && deleted_at.is_none())
        .then(|| title.js_trim())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
pub fn thread_read_label_prefix(label: &str) -> Option<String> {
    label
        .strip_suffix(" a T3 thread")
        .map(|s| format!("{s} thread"))
}
pub fn timeline_entry_run_id(entry: &TimelineEntry) -> Option<String> {
    entry_run_id(&Entry::new(value(entry)))
}
pub fn timeline_entry_starts_response(entry: &TimelineEntry) -> bool {
    starts_response(&Entry::new(value(entry)))
}
pub fn live_answer_preview(entries: &[Rc<TimelineEntry>]) -> Option<String> {
    live_preview(
        &entries
            .iter()
            .map(|e| Entry::new(value(e)))
            .collect::<Vec<_>>(),
    )
}
pub(super) fn live_preview(entries: &[Entry]) -> Option<String> {
    for e in entries
        .iter()
        .skip(boundary(entries).map_or(0, |i| i + 1))
        .rev()
    {
        let text = if e.kind() == "message" && e.message()["role"] == "assistant" {
            s(&e.message()["text"])
        } else if e.kind() == "work" && e.w()["itemType"] == "reasoning" {
            s(&e.w()["detail"])
        } else {
            ""
        };
        let line = live_thought_line(text);
        if !line.is_empty() {
            return Some(line);
        }
    }
    None
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantMessageCopyState {
    pub text: Option<String>,
    pub visible: bool,
}
pub fn resolve_assistant_message_copy_state(
    text: Option<&str>,
    show_copy_button: bool,
    streaming: bool,
) -> AssistantMessageCopyState {
    let text = text.filter(|s| !s.js_trim().is_empty());
    let visible = show_copy_button && text.is_some() && !streaming;
    AssistantMessageCopyState {
        text: text.map(|s| {
            if visible {
                super::viewport::render_directives_for_copy(s)
            } else {
                s.into()
            }
        }),
        visible,
    }
}

pub(super) fn pending(e: &Entry, input: &MessagesTimelineRowsInput) -> bool {
    e.kind() == "work"
        && e.item()["type"] == "approval_request"
        && input
            .pending_approval_request_ids
            .contains(s(&e.item()["requestId"]))
}
pub(super) fn shown_entries(input: &MessagesTimelineRowsInput, entries: &[Entry]) -> Vec<Entry> {
    let terminal = folds::terminals(entries);
    let retained = folds::retained_replies(entries);
    let answering = input.is_working
        && !input
            .latest_run
            .as_ref()
            .is_some_and(|r| r.turn_ended == Some(true));
    let answering_id = answering.then(|| unsettled(input)).flatten();
    let answering_from = if answering {
        boundary(entries).map_or(0, |i| i + 1)
    } else {
        entries.len()
    };
    let mut steer_answers = BTreeSet::new();
    let mut awaiting = false;
    let mut candidate = None;
    for e in entries {
        if e.kind() == "work" && e.w()["itemType"] == "reasoning" {
            continue;
        }
        let reply = e.kind() == "message" && e.message()["role"] == "assistant";
        if reply && let Some(id) = candidate.take() {
            steer_answers.insert(id);
        }
        candidate = None;
        if steer(e) {
            awaiting = true;
        } else if e.kind() == "message" && e.message()["role"] == "user" {
            awaiting = false;
        } else if reply && awaiting {
            candidate = Some(s(&e.message()["id"]).to_owned());
            awaiting = false;
        }
    }
    let mut shown: Vec<_> = entries
        .iter()
        .enumerate()
        .filter(|(index, e)| {
            if input.tool_call_visibility == "zen" {
                if e.kind() == "message" {
                    let id = s(&e.message()["id"]);
                    return e.message()["role"] != "assistant"
                        || input.revealed_message_id.as_deref() == Some(id)
                        || steer_answers.contains(id)
                        || retained.contains(e.id())
                        || (terminal.contains(id)
                            && *index < answering_from
                            && (answering_id.is_none()
                                || optional_id(&e.message()["runId"]) != answering_id));
                }
                if e.kind() == "event" {
                    return e.item()["type"] != "subagent";
                }
                if e.kind() != "work" {
                    return true;
                }
                return pending(e, input)
                    || e.w().get("questionAnswer").is_some()
                    || e.compaction()
                    || starts_response(e)
                    || e.w()["tone"] == "error"
                    || e.w()["sourceActivityKind"] == "runtime.error"
                    || (e.w()["itemType"] == "error"
                        && e.w()["toolLifecycleStatus"] != "completed");
            }
            input.tool_call_visibility.is_empty()
                || input.tool_call_visibility == "showAll"
                || e.work.as_ref().is_none_or(|w| {
                    !work_entry_hidden_by_tool_call_visibility(w, &input.tool_call_visibility)
                })
        })
        .map(|(_, e)| e.clone())
        .collect();
    let len = shown.len();
    for (i, e) in shown.iter_mut().enumerate() {
        if i + 1 < len
            && e.kind() == "work"
            && e.w()["itemType"] == "reasoning"
            && e.w()["toolLifecycleStatus"] == "inProgress"
        {
            e.value["entry"]["toolLifecycleStatus"] = json!("completed");
            if let Some(w) = &mut e.work {
                w.tool_lifecycle_status = Some("completed".into());
            }
        }
    }
    let mut children: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for e in &shown {
        if e.kind() == "event"
            && e.item()["type"] == "subagent"
            && e.item()["origin"] == "app_owned"
            && let Some(run) = optional_id(&e.item()["runId"])
        {
            children
                .entry(run)
                .or_default()
                .insert(s(&e.item()["subagentId"]).into());
        }
    }
    shown.retain(|e| {
        let Some(w) = &e.work else {
            return true;
        };
        if session::work_entry_display_indicates_tool_failure(w) {
            return true;
        }
        let item = if e.item().is_null() {
            &e.w()["structuredPayload"]
        } else {
            e.item()
        };
        if item["type"] != "dynamic_tool"
            || !matches!(s(&item["status"]), "running" | "completed")
            || super::presentation::tool_id(s(&item["toolName"])) != Some("delegate_task")
        {
            return true;
        }
        let Some(task_id) = delegation_task_id(&item["output"]) else {
            return true;
        };
        !children
            .get(s(&item["runId"]))
            .is_some_and(|ids| ids.contains(&task_id))
    });
    shown
}

fn delegation_task_id(output: &Value) -> Option<String> {
    fn read(
        v: &Value,
        depth: usize,
        bytes: &mut usize,
        nodes: &mut usize,
        exceeded: &mut bool,
    ) -> (Option<Value>, bool) {
        if depth > 4 || *nodes == 0 {
            *exceeded = true;
            return (None, false);
        }
        *nodes -= 1;
        match v {
            Value::String(text) => {
                if text.len() > *bytes {
                    *exceeded = true;
                    return (None, false);
                }
                *bytes -= text.len();
                serde_json::from_str(text).ok().map_or((None, false), |v| {
                    read(&v, depth + 1, bytes, nodes, exceeded)
                })
            }
            Value::Array(blocks) => {
                if blocks.len() > 32 {
                    *exceeded = true;
                    return (None, false);
                }
                let mut data = None;
                let mut failed = false;
                for block in blocks {
                    let text = &block["text"];
                    let text = if text.is_object() {
                        text.get("text").unwrap_or(text)
                    } else {
                        text
                    };
                    let result = read(text, depth + 1, bytes, nodes, exceeded);
                    if data.is_none() {
                        data = result.0;
                    }
                    failed |= result.1;
                    if *exceeded {
                        break;
                    }
                }
                (data, failed)
            }
            Value::Object(fields) => {
                let failed = v["isError"] == true
                    || v["is_error"] == true
                    || v["_tag"] == "OrchestratorMcpFailure"
                    || fields.get("error").is_some_and(|v| !v.is_null());
                if let Some(content) = fields
                    .get("structuredContent")
                    .filter(|v| !v.is_null())
                    .or_else(|| fields.get("content"))
                {
                    let result = read(content, depth + 1, bytes, nodes, exceeded);
                    (result.0, failed || result.1)
                } else {
                    (Some(v.clone()), failed)
                }
            }
            _ => (None, false),
        }
    }
    let mut exceeded = false;
    let (data, failed) = read(output, 0, &mut 16384, &mut 128, &mut exceeded);
    if exceeded || failed {
        return None;
    }
    data?["taskId"]
        .as_str()
        .filter(|s| s.encode_utf16().count() <= 256 && !s.js_trim().is_empty())
        .map(str::to_owned)
}

pub(super) fn row_unchanged(a: &MessagesTimelineRow, b: &MessagesTimelineRow) -> bool {
    if a.id != b.id || std::mem::discriminant(&a.content) != std::mem::discriminant(&b.content) {
        return false;
    }
    let mut a = value(a);
    let mut b = value(b);
    a.as_object_mut().map(|m| m.remove("continuesWorkLog"));
    b.as_object_mut().map(|m| m.remove("continuesWorkLog"));
    match s(&a["kind"]) {
        "turn-fold" | "attempt-fold" => {
            if a.get("deferredWorkItemIds").is_none() {
                a["deferredWorkItemIds"] = json!([]);
            }
            if b.get("deferredWorkItemIds").is_none() {
                b["deferredWorkItemIds"] = json!([]);
            }
        }
        "worktree-setup" => return a["snapshot"] == b["snapshot"],
        "work" => {
            a.as_object_mut().map(|m| m.remove("createdAt"));
            b.as_object_mut().map(|m| m.remove("createdAt"));
        }
        "event" => {
            a.as_object_mut().map(|m| m.remove("createdAt"));
            b.as_object_mut().map(|m| m.remove("createdAt"));
        }
        "context-compaction" => {
            a.as_object_mut().map(|m| m.remove("tokenCounts"));
            b.as_object_mut().map(|m| m.remove("tokenCounts"));
        }
        _ => {}
    }
    a == b
}
pub(super) fn patch_streaming(
    input: &MessagesTimelineRowsInput,
    previous: &MessagesTimelineRowsProjection,
) -> Option<Vec<Rc<MessagesTimelineRow>>> {
    if !same_rows_context(input, &previous.input)
        || input.timeline_entries.len() != previous.input.timeline_entries.len()
    {
        return None;
    }
    let mut messages = BTreeMap::new();
    let mut thoughts = BTreeMap::new();
    let mut plans = BTreeMap::new();
    for (e, p) in input
        .timeline_entries
        .iter()
        .zip(&previous.input.timeline_entries)
    {
        if Rc::ptr_eq(e, p) || e == p {
            continue;
        }
        if e.id != p.id || e.created_at != p.created_at || e.attempt != p.attempt {
            return None;
        }
        match (&e.content, &p.content) {
            (
                session::TimelineContent::Message {
                    message,
                    projected_item,
                },
                session::TimelineContent::Message {
                    message: before,
                    projected_item: before_item,
                },
            ) => {
                if !session::is_streaming_message_text_update(before, message) {
                    return None;
                }
                if projected_item != before_item
                    && !before_item
                        .as_ref()
                        .zip(projected_item.as_ref())
                        .is_some_and(|(p, n)| session::is_streaming_turn_item_text_update(p, n))
                {
                    return None;
                }
                messages.insert(
                    s(&value(&message.id)).to_owned(),
                    (message.clone(), projected_item.clone()),
                );
            }
            (
                session::TimelineContent::Work { entry },
                session::TimelineContent::Work { entry: before },
            ) => {
                if before.item_type.as_deref() != Some("reasoning")
                    || !before
                        .projected_item
                        .as_ref()
                        .zip(entry.projected_item.as_ref())
                        .is_some_and(|(p, n)| session::is_streaming_turn_item_text_update(p, n))
                    || before
                        .detail
                        .as_deref()
                        .is_some_and(|s| !s.js_trim().is_empty())
                        != entry
                            .detail
                            .as_deref()
                            .is_some_and(|s| !s.js_trim().is_empty())
                {
                    return None;
                }
                let mut a = value(before);
                let mut b = value(entry);
                for key in ["detail", "structuredPayload", "projectedItem"] {
                    a.as_object_mut()?.remove(key);
                    b.as_object_mut()?.remove(key);
                }
                if a != b {
                    return None;
                }
                thoughts.insert(e.id.clone(), entry.as_ref().clone());
            }
            (
                session::TimelineContent::ProposedPlan { proposed_plan },
                session::TimelineContent::ProposedPlan {
                    proposed_plan: before,
                },
            ) => {
                let mut a = before.clone();
                let mut b = proposed_plan.clone();
                for key in ["planMarkdown", "updatedAt"] {
                    a.as_object_mut()?.remove(key);
                    b.as_object_mut()?.remove(key);
                }
                if a != b {
                    return None;
                }
                plans.insert(e.id.clone(), proposed_plan.clone());
            }
            _ => return None,
        }
    }
    let mut patched_live = BTreeSet::new();
    let mut patched_work = BTreeMap::new();
    let mut patch = |entry: &Rc<WorkLogEntry>| -> Rc<WorkLogEntry> {
        let Some(next) = thoughts.get(&entry.id) else {
            return entry.clone();
        };
        patched_work
            .entry((entry.id.clone(), entry.tool_lifecycle_status.clone()))
            .or_insert_with(|| {
                let mut next = next.clone();
                if entry.tool_lifecycle_status.is_some() {
                    next.tool_lifecycle_status
                        .clone_from(&entry.tool_lifecycle_status);
                }
                Rc::new(next)
            })
            .clone()
    };
    let mut result = Vec::with_capacity(previous.rows.len());
    for row in previous.rows.iter() {
        let mut updated = None;
        match &row.content {
            MessagesTimelineRowContent::Message { message, .. }
            | MessagesTimelineRowContent::AssistantMeta { message, .. } => {
                if let Some((message, projected)) = messages.get(s(&value(&message.id))) {
                    let mut next = row.as_ref().clone();
                    match &mut next.content {
                        MessagesTimelineRowContent::Message {
                            message: target,
                            projected_item,
                            ..
                        }
                        | MessagesTimelineRowContent::AssistantMeta {
                            message: target,
                            projected_item,
                            ..
                        } => {
                            *target = message.clone();
                            if projected.is_some() {
                                *projected_item = projected.clone();
                            }
                        }
                        _ => unreachable!(),
                    }
                    updated = Some(next);
                }
            }
            MessagesTimelineRowContent::ProposedPlan { .. } => {
                if let Some(plan) = plans.get(&row.id) {
                    let mut next = row.as_ref().clone();
                    next.content = MessagesTimelineRowContent::ProposedPlan {
                        proposed_plan: plan.clone(),
                    };
                    updated = Some(next);
                }
            }
            MessagesTimelineRowContent::Work {
                grouped_entries, ..
            }
            | MessagesTimelineRowContent::WorkLive {
                grouped_entries, ..
            } if grouped_entries.iter().any(|e| thoughts.contains_key(&e.id)) => {
                let mut next = row.as_ref().clone();
                match &mut next.content {
                    MessagesTimelineRowContent::Work {
                        grouped_entries, ..
                    } => {
                        for entry in grouped_entries {
                            *entry = patch(entry);
                        }
                    }
                    MessagesTimelineRowContent::WorkLive {
                        entry,
                        grouped_entries,
                        thought,
                        ..
                    } => {
                        for entry in grouped_entries {
                            if thoughts.contains_key(&entry.id) {
                                patched_live.insert(entry.id.clone());
                            }
                            *entry = patch(entry);
                        }
                        *entry = patch(entry);
                        if let Some(thought) = thought {
                            *thought = patch(thought);
                        }
                    }
                    _ => unreachable!(),
                }
                updated = Some(next);
            }
            _ => {}
        }
        result.push(updated.map_or_else(|| row.clone(), Rc::new));
    }
    thoughts
        .keys()
        .all(|id| patched_live.contains(id))
        .then_some(result)
}
