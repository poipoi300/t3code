use super::*;
use crate::{
    view_logic::JsTrim,
    work_log::{self, T3ToolSummaryCall, ToolGroupSummary, WorkEntryToolPresentation},
};
include!("tool_definitions.rs");
type Definition = (&'static str, [&'static str; 4], &'static str, &'static str);
fn definition(name: &str) -> Option<&'static Definition> {
    TOOL_DEFINITIONS.iter().find(|d| d.0 == name)
}
pub(super) fn tool_id(name: &str) -> Option<&'static str> {
    let name = work_log::normalize_compact_tool_label(name);
    if let Some(d) = definition(name) {
        return Some(d.0);
    }
    if let Some(id) = work_log::presentation::tool_id(name)
        && let Some(d) = definition(&id)
    {
        return Some(d.0);
    }
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("mcp__")
        && let Some((server, tool)) = name[5..].split_once("__")
        && [
            "t3-code",
            "t3_code",
            "t3code",
            "t3-code-direct",
            "t3_code_direct",
        ]
        .contains(&server.to_ascii_lowercase().as_str())
    {
        return definition(tool).map(|d| d.0);
    }
    for prefix in [
        "t3-code-direct",
        "t3_code_direct",
        "t3-code",
        "t3_code",
        "t3code",
        "t3 code",
        "mcp-t3-code",
        "mcp_t3_code",
        "mcp__t3_code",
    ] {
        if lower.starts_with(prefix) {
            let rest = &name[prefix.len()..];
            let tail = rest.js_trim_start();
            let candidate = tail.strip_prefix('·').map(str::js_trim_start).or_else(|| {
                rest.starts_with(['.', ':', '/', '_', '-', ' '])
                    .then(|| rest.trim_start_matches(['.', ':', '/', '_', '-', ' ']))
            });
            if let Some(d) = candidate.and_then(definition) {
                return Some(d.0);
            }
        }
    }
    if lower.starts_with("t3-code-") {
        return TOOL_DEFINITIONS
            .iter()
            .filter(|d| name.ends_with(&format!("_{}", d.0)))
            .max_by_key(|d| d.0.len())
            .map(|d| d.0);
    }
    None
}
fn name(entry: &WorkLogEntry) -> String {
    let item = entry
        .structured_payload
        .as_ref()
        .map(value)
        .unwrap_or(Value::Null);
    if item["type"] == "dynamic_tool"
        && let Some(name) = item["toolName"].as_str().filter(|s| !s.is_empty())
    {
        return name.into();
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
fn call(entry: &WorkLogEntry) -> T3ToolSummaryCall {
    let item = entry
        .structured_payload
        .as_ref()
        .map(value)
        .unwrap_or(Value::Null);
    let data = entry.tool_data.as_ref().unwrap_or(&Value::Null);
    let first = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| data.get(*k).filter(|v| !v.is_null()))
            .cloned()
            .unwrap_or(Value::Null)
    };
    T3ToolSummaryCall {
        input: if item["type"] == "dynamic_tool" {
            item["input"].clone()
        } else {
            first(&["arguments", "input", "rawInput"])
        },
        output: if item["type"] == "dynamic_tool" {
            item["output"].clone()
        } else {
            first(&["output", "result", "rawOutput", "content"])
        },
        outcome: if entry.tone == "error"
            || matches!(
                entry.tool_lifecycle_status.as_deref(),
                Some("failed" | "declined")
            ) {
            "failed"
        } else if entry.tool_lifecycle_status.as_deref() == Some("completed") {
            "completed"
        } else {
            "unfinished"
        }
        .into(),
    }
}
fn resolved(entry: &WorkLogEntry) -> Option<&'static Definition> {
    let name = name(entry);
    let id = tool_id(&name)?;
    let input = call(entry).input;
    let action = input["action"].as_str();
    let target = action.map(|a| match id {
        "preview_act" => format!("preview_{a}"),
        "t3_queue" => format!("t3_queue_{a}"),
        "t3_pending_request" => format!("t3_pending_request_{a}"),
        "t3_thread_transfer" => match a {
            "fork" => "t3_thread_fork",
            "merge_back" => "t3_thread_merge_back",
            _ => "t3_thread_transfers",
        }
        .into(),
        _ => String::new(),
    });
    target
        .as_deref()
        .and_then(definition)
        .or_else(|| definition(id))
}
fn summary_action(entry: &WorkLogEntry) -> Option<&'static str> {
    let d = resolved(entry)?;
    let input = call(entry).input;
    Some(match d.0 {
        "t3_thread_configure" if input.get("modelSelection").is_none() => "thread-configuration",
        "schedule_task" if input.get("scheduledTaskId").is_some() => "schedule-update",
        _ => d.3,
    })
}
pub(super) fn tool_presentation(
    entry: &WorkLogEntry,
    fallback: Option<&str>,
) -> Option<WorkEntryToolPresentation> {
    if let Some(p) = work_log::resolve_work_entry_tool_presentation(entry, fallback) {
        return Some(p);
    }
    let d = resolved(entry)?;
    let mut canonical = entry.clone();
    let mut data = canonical
        .tool_data
        .clone()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    data["toolName"] = json!(d.0);
    if let Some(d) = data.as_object_mut() {
        d.remove("server");
        d.remove("tool");
    }
    canonical.tool_data = Some(data);
    if let Some(item) = canonical.structured_payload.as_ref().filter(|p| {
        matches!(
            p.as_ref(),
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::DynamicTool(_)
        )
    }) {
        let mut item = value(item);
        item["toolName"] = json!(d.0);
        canonical.structured_payload = Some(Rc::new(
            serde_json::from_value(item).expect("canonical T3 tool name"),
        ));
    }
    if let Some(p) = work_log::resolve_work_entry_tool_presentation(&canonical, fallback) {
        return Some(p);
    }
    let [action, running, completed, detail] = d.1;
    let output = call(entry).output;
    let status = if work_log::t3_tool_result_indicates_failure(&output) {
        Some("failed")
    } else {
        entry.tool_lifecycle_status.as_deref().or(fallback)
    };
    let verb = match status {
        Some("completed") => completed.into(),
        Some("failed") => format!("Failed to {}", action.to_lowercase()),
        Some("declined") => format!("Declined to {}", action.to_lowercase()),
        Some("stopped") => format!("Stopped {}", running.to_lowercase()),
        _ => running.into(),
    };
    Some(WorkEntryToolPresentation {
        display_name: format!("{verb} {detail}"),
        icon: d.2.into(),
        action: None,
    })
}
fn priority(action: &str) -> usize {
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
            | "theme-set"
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
fn join(labels: &[String]) -> String {
    match labels {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        _ => format!(
            "{}, and {}",
            labels[..labels.len() - 1].join(", "),
            labels.last().map_or("", String::as_str)
        ),
    }
}
pub(super) fn summarize(entries: &[WorkLogEntry]) -> ToolGroupSummary {
    let tools: Vec<_> = entries
        .iter()
        .filter(|e| e.item_type.as_deref() != Some("reasoning"))
        .collect();
    if tools.is_empty() {
        return work_log::summarize_tool_group(entries);
    }
    let mut groups: Vec<(&str, Option<&str>, Vec<&WorkLogEntry>)> = vec![];
    let mut group_indexes = BTreeMap::new();
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
        let action = helpers::tool_action(entry);
        let key = t3.unwrap_or(action);
        if let Some(index) = group_indexes.get(key).copied() {
            let group: &mut (&str, Option<&str>, Vec<&WorkLogEntry>) = &mut groups[index];
            group.2.push(entry);
        } else {
            group_indexes.insert(key, groups.len());
            groups.push((action, t3, vec![entry]));
        }
    }
    let mut summaries: Vec<_> = groups
        .iter()
        .enumerate()
        .map(|(index, (action, t3, entries))| {
            let summary = if let Some(action) = t3 {
                let calls: Vec<_> = entries.iter().map(|e| call(e)).collect();
                if matches!(*action, "theme-read" | "theme-set") {
                    let completed = calls
                        .iter()
                        .filter(|c| {
                            c.outcome == "completed"
                                && !work_log::t3_tool_result_indicates_failure(&c.output)
                        })
                        .count();
                    let count = if completed > 0 {
                        completed
                    } else {
                        calls.len()
                    };
                    let verb = match (*action, completed > 0) {
                        ("theme-read", true) => "Checked",
                        ("theme-read", false) => "Tried to check",
                        (_, true) => "Set",
                        (_, false) => "Tried to set",
                    };
                    work_log::T3ToolSummary {
                        label: format!(
                            "{verb} {} {count} time{}",
                            if *action == "theme-read" {
                                "themes"
                            } else {
                                "the theme"
                            },
                            if count == 1 { "" } else { "s" }
                        ),
                        failed_count: calls
                            .iter()
                            .filter(|c| {
                                c.outcome == "failed"
                                    || work_log::t3_tool_result_indicates_failure(&c.output)
                            })
                            .count(),
                    }
                } else {
                    work_log::summarize_t3_tool_calls(action, &calls)
                }
            } else {
                let prepared = helpers::summary_entries(entries);
                let label = work_log::summarize_tool_group(&prepared).summary;
                work_log::T3ToolSummary {
                    label,
                    failed_count: entries
                        .iter()
                        .filter(|e| session::work_entry_display_indicates_tool_failure(e))
                        .count(),
                }
            };
            (
                index,
                priority(t3.unwrap_or(action)),
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
        let mut names = vec![];
        for source in &sources {
            if let Some(name) = source["name"].as_str()
                && !names.iter().any(|n| n == name)
            {
                names.push(name.to_owned());
            }
        }
        let suffix = if sources.iter().all(|s| s["kind"] == "integration") {
            if names.len() == 1 {
                " integration"
            } else {
                " integrations"
            }
        } else {
            ""
        };
        labels.insert(0, format!("Used {}{suffix}", join(&names)));
    }
    if remaining > 0 {
        labels.push(format!(
            "Performed {remaining} other action{}",
            if remaining == 1 { "" } else { "s" }
        ));
    }
    for label in labels.iter_mut().skip(1) {
        if let Some(c) = label.chars().next() {
            label.replace_range(..c.len_utf8(), &c.to_lowercase().to_string());
        }
    }
    ToolGroupSummary {
        summary: join(&labels),
        has_failure,
    }
}
