//! `OrchestratorMcpService.listThreads` and `readThread` (TypeScript
//! `apps/server/src/mcp/OrchestratorMcpService.ts`) over the projections. Both are `reads`: like
//! TypeScript, they are not scoped to the caller's project or link.
//!
//! Not ported: acknowledging a delegated child task's result when its parent reads it
//! (`readTask(..., "thread-read-acknowledge")`), which needs the task-status service.
use super::{decoded, encoded};
use crate::{services::OrchestratorService, *};
use async_trait::async_trait;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use t3_contracts::{base_schemas::ThreadId, orchestrator_mcp::*};
use t3_orchestration::{
    api::*,
    ports::thread_project::{
        thread_management_service::{ThreadManagementService, latest_active_run, latest_run},
        thread_settlement_service::is_snoozed,
    },
    stores::StoreDatabase,
};

const DEFAULT_THREAD_LIST_LIMIT: usize = 10;
const DEFAULT_THREAD_READ_LIMIT: u64 = 10;
const DEFAULT_THREAD_RUN_LIMIT: usize = 10;
const DEFAULT_THREAD_ITEM_MAX_CHARS: usize = 500;
const THREAD_PULL_REQUEST_PREVIEW_LIMIT: usize = 5;
const THREAD_READ_RESULT_MAX_BYTES: usize = 30_000;
/// `DEFAULT_HOSTED_APP_URL`: outside MCP clients get thread links they can open.
const HOSTED_APP_URL: &str = "https://app.t3.codes";

pub struct ThreadReadAdapter {
    pub threads: ThreadManagementService,
    pub database: StoreDatabase,
}

fn failure(code: &str, message: impl Into<String>) -> McpError {
    McpError::failure(code, message)
}

/// `threadManagementFailure`.
fn management_failure(error: t3_orchestration::error::Error) -> McpError {
    match error {
        t3_orchestration::error::Error::ThreadNotFound(_) => {
            failure("thread_not_found", error.to_string())
        }
        error => failure("orchestration_error", error.to_string()),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// `threadLinkFor`: T3 renders `t3-thread://` links as chips; an outside client gets the hosted
/// link, which opens the thread anywhere.
pub fn thread_link(scope: &InvocationScope, thread: &str, title: Option<&str>) -> String {
    if scope.client.as_ref().is_some_and(|client| !client.linked) {
        let segment = |id: &str| {
            percent_encoding::utf8_percent_encode(id, URI_COMPONENT)
                .to_string()
                .chars()
                .flat_map(|c| match c {
                    '.' | '!' | '~' | '*' | '\'' | '(' | ')' => {
                        format!("%{:X}", c as u32).chars().collect::<Vec<_>>()
                    }
                    c => vec![c],
                })
                .collect::<String>()
        };
        return format!(
            "{HOSTED_APP_URL}/threads/{}/{}",
            segment(&scope.environment_id.0),
            segment(thread)
        );
    }
    let href = format!("t3-thread://v2/{}/{thread}", scope.environment_id.0);
    let label = title.map(|title| {
        title
            .replace(['[', ']', '\\', '\r', '\n'], " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    });
    let destination = if href.contains(['(', ')']) {
        format!("<{href}>")
    } else {
        href.clone()
    };
    match label.filter(|label| !label.is_empty()) {
        Some(label) => format!("[{label}]({destination})"),
        None => format!("<{href}>"),
    }
}

/// `encodeURIComponent`'s unescaped set.
const URI_COMPONENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

impl ThreadReadAdapter {
    fn require_capability(context: &CallContext) -> Result<()> {
        if context.scope.capabilities.contains(&Capability::Orchestration) {
            Ok(())
        } else {
            Err(failure(
                "capability_denied",
                "This MCP credential does not grant orchestration capabilities.",
            ))
        }
    }

    /// `loadCaller(scope, [])`: the calling thread's row, or nothing for a client caller.
    async fn caller(&self, context: &CallContext) -> Result<Option<Value>> {
        Self::require_capability(context)?;
        let Some(caller) = &context.scope.thread else {
            return Ok(None);
        };
        let records = self
            .threads
            .records(caller.thread_id.clone(), Vec::new(), RecordFilter::default())
            .await
            .map_err(|error| {
                failure(
                    "orchestration_error",
                    format!("Unable to read thread {}: {error}", caller.thread_id.0),
                )
            })?;
        Ok(Some(encoded(records.thread)?))
    }

    /// `threadNotFound`: agents often paste the bare uuid of an `mcp:<uuid>` thread.
    async fn thread_not_found(&self, thread: &str, message: String) -> McpError {
        let prefixed = ThreadId(format!("mcp:{thread}"));
        let live = self
            .threads
            .reader()
            .shell(prefixed.clone())
            .await
            .ok()
            .flatten()
            .is_some_and(|shell| shell.deleted_at.is_none());
        failure(
            "thread_not_found",
            if live {
                format!("{message} Did you mean `{}`?", prefixed.0)
            } else {
                message
            },
        )
    }

    async fn require_project(&self, project: &str) -> Result<()> {
        let id = t3_contracts::base_schemas::ProjectId(project.into());
        let found = self
            .database
            .read(move |db| t3_orchestration::stores::projects::get(db, &id, false))
            .await
            .map_err(|error| {
                failure(
                    "orchestration_error",
                    format!("Unable to read project {project}: {error}"),
                )
            })?;
        if found.is_none() {
            return Err(failure(
                "invalid_request",
                format!("Project {project} was not found."),
            ));
        }
        Ok(())
    }
}

fn visible_pull_requests(links: &Value) -> Vec<&Value> {
    links
        .as_array()
        .into_iter()
        .flatten()
        .filter(|link| link["source"] != "stack-dismissed")
        .collect()
}

fn pull_request_updated_at_ms(link: &Value) -> i64 {
    link["snapshot"]["updatedAt"]
        .as_str()
        .or_else(|| link["linkedAt"].as_str())
        .and_then(iso_ms)
        .unwrap_or(0)
}

fn iso_ms(value: &str) -> Option<i64> {
    let value = value.trim();
    let (date, time) = value.split_once('T')?;
    let mut parts = date.split('-');
    let (y, m, d): (i64, i64, i64) = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    let time = time.trim_end_matches('Z');
    let mut clock = time.split(':');
    let (h, min): (i64, i64) = (clock.next()?.parse().ok()?, clock.next()?.parse().ok()?);
    let seconds: f64 = clock.next().unwrap_or("0").parse().ok()?;
    let (y, m) = if m <= 2 { (y - 1, m + 12) } else { (y, m) };
    let days = 365 * y + y / 4 - y / 100 + y / 400 + (153 * (m - 3) + 2) / 5 + d - 719_469;
    Some(((days * 24 + h) * 60 + min) * 60_000 + (seconds * 1000.0).round() as i64)
}

/// `threadPullRequests`. The host is the projection's identity host, which the projection writer
/// normalizes through the forge registry when one is bound.
fn thread_pull_requests(links: &Value, include_links: bool, out: &mut Map<String, Value>) {
    let visible = visible_pull_requests(links);
    if !include_links {
        out.insert("pullRequestCount".into(), visible.len().into());
        return;
    }
    let mut open: Vec<&Value> = visible
        .iter()
        .copied()
        .filter(|link| {
            link["snapshot"]["state"] != "merged" && link["snapshot"]["state"] != "closed"
        })
        .collect();
    let open_count = open.len();
    open.sort_by(|left, right| {
        (!right["watch"].is_null() && right.get("watch").is_some())
            .cmp(&(!left["watch"].is_null() && left.get("watch").is_some()))
            .then(pull_request_updated_at_ms(right).cmp(&pull_request_updated_at_ms(left)))
    });
    let previews = open
        .into_iter()
        .take(THREAD_PULL_REQUEST_PREVIEW_LIMIT)
        .map(|link| {
            let watching = link.get("watch").is_some_and(|w| !w.is_null());
            json!({
                "host": link["host"].as_str().unwrap_or_default().trim().to_lowercase(),
                "repository": link["repository"],
                "number": link["number"],
                "url": link["url"],
                "source": link["source"],
                "state": link["snapshot"]["state"].clone(),
                "watching": watching,
                "watchStartedAt": if watching { link["watch"]["startedAt"].clone() } else { Value::Null },
            })
        })
        .collect::<Vec<_>>();
    out.insert("pullRequests".into(), previews.into());
    out.insert("pullRequestCount".into(), visible.len().into());
    out.insert("openPullRequestCount".into(), open_count.into());
}

fn settlement(thread: &Value, out: &mut Map<String, Value>) {
    let settled = thread["settledOverride"] == "settled";
    out.insert("settled".into(), settled.into());
    out.insert(
        "settledAt".into(),
        if settled {
            thread["settledAt"].clone()
        } else {
            Value::Null
        },
    );
}

fn snooze(
    shell: &t3_contracts::orchestration_v2::OrchestrationV2ThreadShell,
    value: &Value,
    now: i64,
    out: &mut Map<String, Value>,
) {
    let snoozed = is_snoozed(shell, now);
    out.insert("snoozed".into(), snoozed.into());
    out.insert(
        "snoozedUntil".into(),
        if snoozed {
            value["snoozedUntil"].clone()
        } else {
            Value::Null
        },
    );
}

/// `threadBackgroundWork`: provider-internal links stay out of agent results.
fn background_work(shell: &Value, out: &mut Map<String, Value>) {
    out.insert(
        "blockedOnThreadWait".into(),
        (shell["blockedOnThreadWait"] == true).into(),
    );
    let tasks = shell["pendingBackgroundTasks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|task| {
            let mut entry = Map::new();
            entry.insert("taskId".into(), task["taskId"].clone());
            entry.insert("kind".into(), task["kind"].clone());
            for key in ["description", "awaited"] {
                if let Some(value) = task.get(key) {
                    entry.insert(key.into(), value.clone());
                }
            }
            if task["kind"] == "subagent"
                && let Some(child) = task.get("childThreadId")
            {
                entry.insert("childThreadId".into(), child.clone());
            }
            Value::Object(entry)
        })
        .collect::<Vec<_>>();
    out.insert("pendingBackgroundTasks".into(), tasks.into());
}

/// `backgroundWorkHoldsCompletion`.
fn holds_completion(tasks: &Value) -> bool {
    tasks.as_array().into_iter().flatten().any(|task| {
        task["awaited"] == true || matches!(task["kind"].as_str(), Some("subagent" | "background_task"))
    })
}

/// `listedThreadStatus`: awaited background work keeps a thread live after its turn ends.
fn listed_status(shell: &Value) -> Value {
    if !shell["activityRunStatus"].is_null() && shell.get("activityRunStatus").is_some() {
        return shell["activityRunStatus"].clone();
    }
    if holds_completion(&shell["pendingBackgroundTasks"]) {
        "waiting".into()
    } else {
        shell["status"].clone()
    }
}

fn nullable(value: &Value, key: &str) -> Value {
    value.get(key).cloned().unwrap_or(Value::Null)
}

fn list_item(
    scope: &InvocationScope,
    shell: &t3_contracts::orchestration_v2::OrchestrationV2ThreadShell,
    now: i64,
) -> Result<Value> {
    let value = encoded(shell)?;
    let mut out = Map::new();
    out.insert("threadId".into(), value["id"].clone());
    out.insert("projectId".into(), value["projectId"].clone());
    out.insert("title".into(), value["title"].clone());
    out.insert(
        "link".into(),
        thread_link(scope, &shell.id.0, Some(&shell.title)).into(),
    );
    out.insert("createdBy".into(), value["createdBy"].clone());
    out.insert("creationSource".into(), value["creationSource"].clone());
    out.insert("status".into(), listed_status(&value));
    out.insert("latestRunId".into(), nullable(&value, "latestRunId"));
    out.insert(
        "providerInstanceId".into(),
        value["modelSelection"]["instanceId"].clone(),
    );
    out.insert("model".into(), value["modelSelection"]["model"].clone());
    out.insert("runtimeMode".into(), value["runtimeMode"].clone());
    out.insert("interactionMode".into(), value["interactionMode"].clone());
    out.insert(
        "linkedPullRequest".into(),
        nullable(&value, "linkedPullRequest"),
    );
    thread_pull_requests(&value["pullRequests"], true, &mut out);
    settlement(&value, &mut out);
    snooze(shell, &value, now, &mut out);
    background_work(&value, &mut out);
    out.insert(
        "parentThreadId".into(),
        value["lineage"]["parentThreadId"].clone(),
    );
    out.insert(
        "relationshipToParent".into(),
        value["lineage"]["relationshipToParent"].clone(),
    );
    out.insert("itemCount".into(), value["visibleItemCount"].clone());
    out.insert("createdAt".into(), value["createdAt"].clone());
    out.insert("updatedAt".into(), value["updatedAt"].clone());
    Ok(Value::Object(out))
}

fn json_text(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn join(parts: &[Option<String>]) -> String {
    parts.iter().flatten().cloned().collect::<Vec<_>>().join("\n")
}

fn text_of(item: &Value, key: &str) -> Option<String> {
    item.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// `turnItemText`.
fn turn_item_text(item: &Value) -> Option<String> {
    let s = |key: &str| text_of(item, key);
    Some(match item["type"].as_str()? {
        "notification" => join(&[s("summary"), s("detail")]),
        "user_message" | "assistant_message" | "reasoning" => s("text")?,
        "proposed_plan" => s("markdown")?,
        "todo_list" => {
            let mut lines = vec![s("explanation")];
            for step in item["steps"].as_array().into_iter().flatten() {
                lines.push(Some(format!(
                    "[{}] {}",
                    step["status"].as_str().unwrap_or_default(),
                    step["text"].as_str().unwrap_or_default()
                )));
            }
            join(&lines)
        }
        "user_input_request" => json_text(&item["questions"]),
        "file_change" => join(&[
            s("fileName"),
            if item.get("additions").is_none() && item.get("deletions").is_none() {
                None
            } else {
                Some(format!(
                    "+{} -{}",
                    item["additions"].as_i64().unwrap_or(0),
                    item["deletions"].as_i64().unwrap_or(0)
                ))
            },
            s("diffStr").or_else(|| s("newStr")),
        ]),
        "command_execution" => join(&[
            Some(format!("$ {}", item["input"].as_str().unwrap_or_default())),
            s("output"),
        ]),
        "file_search" => {
            json_text(&json!({"pattern": item.get("pattern"), "results": item.get("results")}))
        }
        "web_search" => {
            json_text(&json!({"patterns": item.get("patterns"), "results": item.get("results")}))
        }
        "approval_request" => join(&[s("prompt").or_else(|| s("requestKind")), s("reason")]),
        "checkpoint" => json_text(&item["files"]),
        "run_interrupt_request" | "run_interrupt_result" | "system_notice" => s("message")?,
        "error" => text_of(&item["failure"], "message")?,
        "compaction" => s("summary")?,
        "handoff" => s("summary").unwrap_or_else(|| {
            format!(
                "{} handoff to {}",
                item["strategy"].as_str().unwrap_or_default(),
                item["toProviderInstanceId"].as_str().unwrap_or_default()
            )
        }),
        "fork" => format!(
            "Forked to thread {}.",
            item["targetThreadId"].as_str().unwrap_or_default()
        ),
        "thread_created" => format!(
            "Created thread {} with {} ({}).",
            item["targetThreadId"].as_str().unwrap_or_default(),
            item["targetProviderInstanceId"].as_str().unwrap_or_default(),
            item["targetModel"].as_str().unwrap_or_default()
        ),
        "secret_request" => format!(
            "Asked the user for {}: {}.",
            item["label"].as_str().unwrap_or_default(),
            item["secretStatus"].as_str().unwrap_or_default()
        ),
        "link_request" => {
            if item["linkStatus"] == "linked" {
                format!(
                    "Linked {} ({}).",
                    s("linkedLabel")
                        .or_else(|| s("url"))
                        .unwrap_or_else(|| "an environment".into()),
                    item["linkedAccess"].as_str().unwrap_or_default()
                )
            } else {
                format!(
                    "Asked the user to link {}: {}.",
                    s("url").unwrap_or_else(|| "an environment".into()),
                    item["linkStatus"].as_str().unwrap_or_default()
                )
            }
        }
        "subagent" => s("result").or_else(|| s("progress")).or_else(|| s("prompt"))?,
        "dynamic_tool" => json_text(&json!({
            "toolName": item.get("toolName"),
            "input": item.get("input"),
            "output": item.get("output"),
        })),
        _ => return None,
    })
}

/// `textPageEnd`, in UTF-16 code units: a cut inside a surrogate pair moves.
fn text_page_end(text: Option<&[u16]>, offset: usize, max_chars: usize) -> usize {
    let end = offset + max_chars;
    let Some(text) = text else { return end };
    if end >= text.len() {
        return end;
    }
    let last = text[end - 1];
    if !(0xd800..=0xdbff).contains(&last) {
        return end;
    }
    if end - 1 > offset { end - 1 } else { end + 1 }
}

fn utf16_slice(text: &[u16], start: usize, end: usize) -> String {
    let start = start.min(text.len());
    let end = end.min(text.len()).max(start);
    String::from_utf16_lossy(&text[start..end])
}

struct ItemInput<'a> {
    row: &'a Value,
    max_chars: usize,
    text_offset: Option<usize>,
    messages: &'a BTreeMap<String, Vec<Value>>,
}

/// `timelineItem`.
fn timeline_item(input: &ItemInput<'_>, max_chars: usize, title: Option<Value>) -> Value {
    let item = &input.row["item"];
    let text = turn_item_text(item).map(|t| t.encode_utf16().collect::<Vec<u16>>());
    let offset = input.text_offset.unwrap_or(0);
    let end = text_page_end(text.as_deref(), offset, max_chars);
    let truncated = text.as_ref().is_some_and(|t| t.len() > end);
    let kind = item["type"].as_str().unwrap_or_default();
    let message_id = if matches!(kind, "user_message" | "assistant_message") {
        item["messageId"].clone()
    } else {
        Value::Null
    };
    let message = message_id.as_str().and_then(|id| {
        input
            .messages
            .get(input.row["sourceThreadId"].as_str().unwrap_or_default())?
            .iter()
            .find(|m| m["id"] == id)
    });
    let mut out = Map::new();
    out.insert("position".into(), input.row["position"].clone());
    out.insert("visibility".into(), input.row["visibility"].clone());
    out.insert("sourceThreadId".into(), input.row["sourceThreadId"].clone());
    out.insert("itemId".into(), input.row["sourceItemId"].clone());
    if kind == "subagent" {
        out.insert("subagentId".into(), item["subagentId"].clone());
    }
    out.insert("runId".into(), nullable(item, "runId"));
    out.insert("messageId".into(), message_id);
    out.insert(
        "createdBy".into(),
        message.map_or(Value::Null, |m| nullable(m, "createdBy")),
    );
    out.insert(
        "creationSource".into(),
        message.map_or(Value::Null, |m| nullable(m, "creationSource")),
    );
    if let Some(sender) = message.and_then(|m| m.get("senderThreadId")) {
        out.insert("senderThreadId".into(), sender.clone());
    }
    out.insert("type".into(), item["type"].clone());
    out.insert("status".into(), item["status"].clone());
    out.insert(
        "title".into(),
        title.unwrap_or_else(|| nullable(item, "title")),
    );
    out.insert(
        "text".into(),
        text.as_ref()
            .map_or(Value::Null, |t| utf16_slice(t, offset, end).into()),
    );
    out.insert("textTruncated".into(), truncated.into());
    out.insert(
        "nextTextOffset".into(),
        if truncated { end.into() } else { Value::Null },
    );
    out.insert("updatedAt".into(), item["updatedAt"].clone());
    Value::Object(out)
}

fn json_bytes(value: &Value) -> usize {
    serde_json::to_string(value).map_or(0, |s| s.len())
}

/// The longest prefix length in [low, high] that `fits`, or `low - 1`.
fn longest_fitting(mut low: i64, mut high: i64, fits: impl Fn(usize) -> bool) -> i64 {
    let mut best = low - 1;
    while low <= high {
        let mid = (low + high).div_euclid(2);
        if fits(mid.max(0) as usize) {
            best = mid;
            low = mid + 1;
        } else {
            high = mid - 1;
        }
    }
    best
}

/// `timelineItemWithin`: the text cut, and an oversized title too, until the JSON fits.
fn timeline_item_within(input: &ItemInput<'_>, max_bytes: usize) -> Value {
    let longest_text = |title: Value| {
        let with_text = |chars: usize| timeline_item(input, chars, Some(title.clone()));
        let chars = longest_fitting(2, input.max_chars as i64 - 1, |n| {
            json_bytes(&with_text(n)) <= max_bytes
        });
        with_text(chars.max(1) as usize)
    };
    let item = longest_text(nullable(&input.row["item"], "title"));
    let Some(title) = item["title"].as_str().map(str::to_owned) else {
        return item;
    };
    if json_bytes(&item) <= max_bytes {
        return item;
    }
    let units = title.encode_utf16().collect::<Vec<u16>>();
    let cut = |chars: usize| {
        if chars == 0 {
            String::new()
        } else {
            utf16_slice(&units, 0, text_page_end(Some(&units), 0, chars))
        }
    };
    let title_chars = longest_fitting(1, units.len() as i64 - 1, |n| {
        let mut candidate = item.clone();
        candidate["title"] = cut(n).into();
        json_bytes(&candidate) as f64 <= max_bytes as f64 / 2.0
    });
    longest_text(cut(title_chars.max(0) as usize).into())
}

/// `timelineItemsWithinBudget`: never empty for a non-empty page.
fn items_within_budget(
    envelope: &Value,
    inputs: &[ItemInput<'_>],
    newest_first: bool,
) -> (Vec<Value>, usize) {
    let mut empty = envelope.clone();
    empty["items"] = json!([]);
    let budget = (THREAD_READ_RESULT_MAX_BYTES as i64 - json_bytes(&empty) as i64)
        .max((THREAD_READ_RESULT_MAX_BYTES / 2) as i64) as usize;
    let mut items = Vec::new();
    let mut bytes = 0;
    let ordered: Vec<&ItemInput<'_>> = if newest_first {
        inputs.iter().rev().collect()
    } else {
        inputs.iter().collect()
    };
    for input in ordered {
        let item = timeline_item(input, input.max_chars, None);
        bytes += json_bytes(&item) + usize::from(!items.is_empty());
        if bytes <= budget {
            items.push(item);
            continue;
        }
        if items.is_empty() {
            items.push(timeline_item_within(input, budget));
        }
        break;
    }
    if newest_first {
        items.reverse();
        let start = inputs.len() - items.len();
        (items, start)
    } else {
        (items, 0)
    }
}

fn thread_run(projection: &t3_contracts::orchestration_v2::OrchestrationV2ThreadProjection, run: &Value) -> Value {
    let attempt = &run["activeAttemptId"];
    let executed = projection
        .provider_turns
        .iter()
        .filter_map(|turn| encoded(turn).ok())
        .filter(|turn| !attempt.is_null() && turn["runAttemptId"] == *attempt)
        .max_by_key(|turn| turn["ordinal"].as_u64())
        .map_or(Value::Null, |turn| nullable(&turn, "model"));
    json!({
        "runId": run["id"],
        "ordinal": run["ordinal"],
        "status": run["status"],
        "providerInstanceId": run["modelSelection"]["instanceId"],
        "model": run["modelSelection"]["model"],
        "executedModel": executed,
        "requestedAt": run["requestedAt"],
        "startedAt": nullable(run, "startedAt"),
        "completedAt": nullable(run, "completedAt"),
    })
}

#[async_trait]
impl OrchestratorService for ThreadReadAdapter {
    async fn t3_thread_list(
        &self,
        context: CallContext,
        input: OrchestratorMcpThreadListInput,
    ) -> Result<OrchestratorMcpThreadListResult> {
        let input = encoded(input)?;
        let parent = self.caller(&context).await?;
        let all_projects = input["allProjects"] == true;
        let project_id = if all_projects {
            None
        } else {
            Some(
                input["projectId"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| parent.as_ref().and_then(|p| p["projectId"].as_str().map(str::to_owned)))
                    .ok_or_else(|| failure("target_required", "Pass projectId, or allProjects: true for every project. This MCP client is not running inside a T3 thread."))?,
            )
        };
        if !all_projects && let Some(project) = input["projectId"].as_str() {
            self.require_project(project).await?;
        }
        let list_failure =
            |error: t3_orchestration::error::Error| failure("orchestration_error", format!("Unable to list threads: {error}"));
        let entries = self
            .threads
            .list_project_threads(
                project_id
                    .clone()
                    .map(t3_contracts::base_schemas::ProjectId),
                input["includeSubagents"] != false,
            )
            .await
            .map_err(list_failure)?;
        let now = now_ms();
        let statuses: Option<Vec<Value>> = input["statuses"]
            .as_array()
            .filter(|statuses| !statuses.is_empty())
            .cloned();
        let title_contains = input["titleContains"].as_str().map(str::to_lowercase);
        let snoozed_filter = input.get("snoozed").and_then(Value::as_bool);
        let settled_filter = input.get("settled").and_then(Value::as_bool);
        let candidates: Vec<ThreadId> = entries
            .into_iter()
            .filter(|thread| {
                settled_filter.is_none_or(|settled| {
                    (encoded(&thread.settled_override).ok() == Some(json!("settled"))) == settled
                })
            })
            .filter(|thread| {
                title_contains
                    .as_ref()
                    .is_none_or(|needle| thread.title.to_lowercase().contains(needle))
            })
            .filter(|thread| {
                snoozed_filter != Some(true)
                    || thread
                        .snoozed_until
                        .as_ref()
                        .and_then(|until| iso_ms(&until.0))
                        .is_some_and(|until| until > now)
            })
            .map(|thread| thread.id)
            .collect();
        let shells = |ids: Vec<ThreadId>| async move {
            let mut shells = Vec::new();
            for id in ids {
                if let Some(shell) = self
                    .threads
                    .reader()
                    .shell(id)
                    .await
                    .map_err(list_failure)?
                    .filter(|shell| shell.deleted_at.is_none() && shell.archived_at.is_none())
                {
                    shells.push(shell);
                }
            }
            Ok::<_, McpError>(shells)
        };
        let cursor = input["cursor"].as_u64().unwrap_or(0) as usize;
        let limit = input["limit"]
            .as_u64()
            .map_or(DEFAULT_THREAD_LIST_LIMIT, |l| l as usize);
        let (page, total) = if statuses.is_none() && snoozed_filter.is_none() {
            let total = candidates.len();
            let page = candidates.into_iter().skip(cursor).take(limit).collect();
            (shells(page).await?, total)
        } else {
            let mut filtered = Vec::new();
            for shell in shells(candidates).await? {
                let value = encoded(&shell)?;
                if statuses
                    .as_ref()
                    .is_some_and(|statuses| !statuses.contains(&listed_status(&value)))
                {
                    continue;
                }
                if snoozed_filter.is_some_and(|snoozed| is_snoozed(&shell, now) != snoozed) {
                    continue;
                }
                filtered.push(shell);
            }
            let total = filtered.len();
            (filtered.into_iter().skip(cursor).take(limit).collect(), total)
        };
        let threads = page
            .iter()
            .map(|shell| list_item(&context.scope, shell, now))
            .collect::<Result<Vec<_>>>()?;
        decoded(json!({
            "projectId": project_id,
            "currentThreadId": parent.as_ref().map_or(Value::Null, |p| p["id"].clone()),
            "threads": threads,
            "nextCursor": if cursor + limit < total { json!(cursor + limit) } else { Value::Null },
            "total": total,
        }))
    }

    async fn t3_thread_read(
        &self,
        context: CallContext,
        input: OrchestratorMcpThreadReadInput,
    ) -> Result<OrchestratorMcpThreadReadResult> {
        let input = encoded(input)?;
        let thread_id = ThreadId(input["threadId"].as_str().unwrap_or_default().into());
        let parent = self.caller(&context).await?;
        let shell = self
            .threads
            .reader()
            .shell(thread_id.clone())
            .await
            .map_err(management_failure)?;
        let Some(shell) = shell.filter(|shell| shell.deleted_at.is_none()) else {
            return Err(self
                .thread_not_found(
                    &thread_id.0,
                    format!("Thread {} is no longer available.", thread_id.0),
                )
                .await);
        };
        let mut fields = vec![
            RecordKind::Runs,
            RecordKind::ProviderTurns,
            RecordKind::RuntimeRequests,
        ];
        if parent.is_some() {
            fields.push(RecordKind::ContextTransfers);
        }
        let target = self
            .threads
            .project_records(&shell.project_id, thread_id.clone(), fields, RecordFilter::default())
            .await
            .map_err(management_failure)?;
        let view = match input["view"].as_str() {
            Some("activity") => TimelineView::Activity,
            _ => TimelineView::Messages,
        };
        let limit = input["limit"].as_u64().unwrap_or(DEFAULT_THREAD_READ_LIMIT);
        let max_chars = input["maxCharsPerItem"]
            .as_u64()
            .map_or(DEFAULT_THREAD_ITEM_MAX_CHARS, |n| n as usize);
        let after = input["afterPosition"].as_i64();
        let before = input["beforePosition"].as_u64();
        let item_id = input["itemId"].as_str().map(|id| t3_contracts::base_schemas::TurnItemId(id.into()));
        // Client callers (the CLI, older linked environments) page forward from a cursorless read.
        let newest_first = before.is_some() || (after.is_none() && context.scope.thread.is_some());
        let timeline = self
            .threads
            .reader()
            .timeline(TimelineQuery {
                thread_id: thread_id.clone(),
                after_position: after.map(|p| p.max(0) as u64),
                before_position: if newest_first {
                    Some(before.unwrap_or(u64::MAX))
                } else {
                    None
                },
                limit,
                item_id: item_id.clone(),
                attachment_id: None,
                view: Some(view),
            })
            .await
            .map_err(management_failure)?;
        let rows = timeline
            .items
            .iter()
            .map(encoded)
            .collect::<Result<Vec<_>>>()?;
        let mut ids_by_thread: BTreeMap<String, Vec<t3_contracts::base_schemas::MessageId>> =
            BTreeMap::new();
        for row in &rows {
            if matches!(
                row["item"]["type"].as_str(),
                Some("user_message" | "assistant_message")
            ) && let (Some(source), Some(message)) = (
                row["sourceThreadId"].as_str(),
                row["item"]["messageId"].as_str(),
            ) {
                ids_by_thread
                    .entry(source.into())
                    .or_default()
                    .push(t3_contracts::base_schemas::MessageId(message.into()));
            }
        }
        let mut messages = BTreeMap::new();
        for (source, ids) in ids_by_thread {
            let records = self
                .threads
                .query_records(ProjectionRecordQuery {
                    thread_id: ThreadId(source.clone()),
                    fields: vec![RecordKind::Messages],
                    filter: RecordFilter::default(),
                    messages: MessageFilter {
                        ids: Some(ids),
                        ..Default::default()
                    },
                    run_statuses: None,
                    include_latest_run: false,
                })
                .await
                .map_err(management_failure)?;
            messages.insert(
                source,
                records
                    .messages
                    .iter()
                    .map(encoded)
                    .collect::<Result<Vec<_>>>()?,
            );
        }
        let first_page = after.is_none() && before.is_none() && item_id.is_none();
        let thread = self.detail(&context.scope, &target, timeline.total_items, &shell, first_page)?;
        let mut runs = target.runs.iter().map(encoded).collect::<Result<Vec<_>>>()?;
        runs.sort_by(|l, r| r["ordinal"].as_u64().cmp(&l["ordinal"].as_u64()));
        let run_limit = input["runLimit"]
            .as_u64()
            .map_or(DEFAULT_THREAD_RUN_LIMIT, |n| n as usize);
        let recent_runs = runs
            .iter()
            .take(run_limit)
            .map(|run| thread_run(&target, run))
            .collect::<Vec<_>>();
        let inputs = rows
            .iter()
            .map(|row| ItemInput {
                row,
                max_chars,
                text_offset: item_id
                    .as_ref()
                    .map(|_| input["textOffset"].as_u64().unwrap_or(0) as usize),
                messages: &messages,
            })
            .collect::<Vec<_>>();
        let envelope = json!({"thread": thread, "recentRuns": recent_runs, "nextPosition": null, "hasMore": false, "earlierPosition": null});
        let (items, start) = items_within_budget(&envelope, &inputs, newest_first);
        decoded(json!({
            "thread": thread,
            "recentRuns": recent_runs,
            "nextPosition": items.last().map_or(Value::Null, |item| item["position"].clone()),
            "hasMore": timeline.has_more || (!newest_first && items.len() < rows.len()),
            "earlierPosition": if timeline.has_earlier || start > 0 { items.first().map_or(Value::Null, |item| item["position"].clone()) } else { Value::Null },
            "items": items,
        }))
    }
}

impl ThreadReadAdapter {
    /// `threadDetail`.
    fn detail(
        &self,
        scope: &InvocationScope,
        projection: &t3_contracts::orchestration_v2::OrchestrationV2ThreadProjection,
        item_count: u64,
        shell: &t3_contracts::orchestration_v2::OrchestrationV2ThreadShell,
        first_page: bool,
    ) -> Result<Value> {
        let thread = encoded(&projection.thread)?;
        let shell_value = encoded(shell)?;
        let latest = latest_run(&projection.runs).map(encoded).transpose()?;
        let active = latest_active_run(&projection.runs).map(encoded).transpose()?;
        let mut out = Map::new();
        out.insert("threadId".into(), thread["id"].clone());
        out.insert("projectId".into(), thread["projectId"].clone());
        out.insert("title".into(), thread["title"].clone());
        out.insert(
            "link".into(),
            thread_link(
                scope,
                thread["id"].as_str().unwrap_or_default(),
                thread["title"].as_str(),
            )
            .into(),
        );
        out.insert("createdBy".into(), thread["createdBy"].clone());
        out.insert("creationSource".into(), thread["creationSource"].clone());
        out.insert(
            "status".into(),
            active
                .as_ref()
                .or(latest.as_ref())
                .map_or("idle".into(), |run| run["status"].clone()),
        );
        out.insert(
            "latestRunId".into(),
            latest.as_ref().map_or(Value::Null, |run| run["id"].clone()),
        );
        out.insert(
            "activeRunId".into(),
            active.as_ref().map_or(Value::Null, |run| run["id"].clone()),
        );
        out.insert(
            "providerInstanceId".into(),
            thread["modelSelection"]["instanceId"].clone(),
        );
        out.insert("model".into(), thread["modelSelection"]["model"].clone());
        out.insert("runtimeMode".into(), thread["runtimeMode"].clone());
        out.insert("interactionMode".into(), thread["interactionMode"].clone());
        out.insert(
            "linkedPullRequest".into(),
            nullable(&thread, "linkedPullRequest"),
        );
        thread_pull_requests(&thread["pullRequests"], first_page, &mut out);
        let regeneration = nullable(&thread, "titleRegeneration");
        out.insert(
            "titleRegeneration".into(),
            if regeneration.is_null() {
                Value::Null
            } else {
                json!({"requestId": regeneration["requestId"], "startedAt": regeneration["startedAt"]})
            },
        );
        out.insert(
            "lastTitleRegeneration".into(),
            nullable(&thread, "lastTitleRegeneration"),
        );
        out.insert("branch".into(), nullable(&thread, "branch"));
        out.insert("worktreePath".into(), nullable(&thread, "worktreePath"));
        out.insert(
            "parentThreadId".into(),
            thread["lineage"]["parentThreadId"].clone(),
        );
        out.insert(
            "relationshipToParent".into(),
            thread["lineage"]["relationshipToParent"].clone(),
        );
        out.insert("runCount".into(), projection.runs.len().into());
        out.insert("itemCount".into(), item_count.into());
        out.insert(
            "pendingRequestCount".into(),
            projection
                .runtime_requests
                .iter()
                .filter_map(|r| encoded(r).ok())
                .filter(|r| r["status"] == "pending")
                .count()
                .into(),
        );
        out.insert("archived".into(), (!thread["archivedAt"].is_null()).into());
        settlement(&thread, &mut out);
        snooze(shell, &shell_value, now_ms(), &mut out);
        background_work(&shell_value, &mut out);
        out.insert("createdAt".into(), thread["createdAt"].clone());
        out.insert("updatedAt".into(), thread["updatedAt"].clone());
        Ok(Value::Object(out))
    }
}
