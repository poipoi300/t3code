use super::{context_handoff_delivery::HandoffBudget, provider_event_ingestor::expanded_id};
use crate::{
    api::*,
    error::{Error, Result},
};
use serde_json::json;
use std::{collections::HashSet, sync::Arc};

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}
fn js_whitespace(ch: char) -> bool {
    let mut bytes = [0; 4];
    t3_contracts::wire::trim_js(ch.encode_utf8(&mut bytes)).is_empty()
}
pub(crate) fn collapse_js_whitespace(text: &str) -> String {
    text.split(js_whitespace)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
fn prefix(text: &str, limit: usize) -> &str {
    let mut used = 0;
    for (index, ch) in text.char_indices() {
        used += ch.len_utf16();
        if used > limit {
            return &text[..index];
        }
    }
    text
}
pub fn compact_text(text: &str, limit: usize) -> String {
    let text = collapse_js_whitespace(text);
    if utf16_len(&text) <= limit {
        text
    } else {
        format!("{}...", prefix(&text, limit.saturating_sub(3)))
    }
}
pub fn summarize_delta_item(item: &OrchestrationV2TurnItem) -> Result<Option<String>> {
    let value = t3_contracts::stored::to_value(item)?;
    let text = |field| value[field].as_str().unwrap_or_default();
    let (label, body) = match text("type") {
        "user_message" => ("User", text("text").to_owned()),
        "assistant_message" => ("Assistant", text("text").to_owned()),
        "command_execution" => ("Command", text("input").to_owned()),
        "file_change" => (
            "File change",
            value["changes"]
                .as_array()
                .map(|changes| {
                    changes
                        .iter()
                        .map(|change| change["path"].as_str().unwrap_or_default())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_else(|| text("fileName").to_owned()),
        ),
        "checkpoint" => {
            return Ok(Some(format!(
                "- Checkpoint: {} files",
                value["files"].as_array().map_or(0, Vec::len)
            )));
        }
        "handoff" => (
            "Handoff",
            value["summary"]
                .as_str()
                .unwrap_or(text("strategy"))
                .to_owned(),
        ),
        _ => return Ok(None),
    };
    Ok(Some(format!("- {label}: {}", compact_text(&body, 240))))
}
pub fn make_side_chat_context(parent: &ThreadId, digest: &[String]) -> String {
    make_side_chat_context_with(parent, digest, |line| Ok(Some(line.clone())))
        .expect("string digest summarization is infallible")
}
pub fn make_side_chat_context_with<T>(
    parent: &ThreadId,
    digest: &[T],
    mut summarize: impl FnMut(&T) -> Result<Option<String>>,
) -> Result<String> {
    let framing = "You are in a side chat beside the main thread. The main thread may still be running in the same worktree. Answer the user's questions; do not continue or redo the main thread's task.";
    let header = "Main thread activity since this side chat's starting point:";
    let truncated = "- Earlier items truncated.";
    let footer = format!(
        "Use t3_thread_read on thread {} to see the main thread's live state.",
        parent.0
    );
    let mut remaining = 8_000usize.saturating_sub(utf16_len(
        &[framing, "", header, truncated, "", &footer].join("\n"),
    ));
    let mut kept = vec![];
    let mut omitted = false;
    for item in digest.iter().rev() {
        let Some(line) = summarize(item)? else {
            continue;
        };
        let cost = utf16_len(&line) + 1;
        if cost > remaining {
            omitted = true;
            break;
        }
        kept.push(line);
        remaining -= cost;
    }
    kept.reverse();
    let mut lines = vec![framing.to_owned()];
    if !kept.is_empty() {
        lines.extend([String::new(), header.to_owned()]);
        if omitted {
            lines.push(truncated.to_owned());
        }
        lines.extend(kept);
    }
    lines.extend([String::new(), footer]);
    Ok(lines.join("\n"))
}
pub fn is_side_chat_context_handoff(handoff: &OrchestrationV2ContextHandoff) -> bool {
    handoff.strategy == ContextHandoffStrategy::ManualContext
        && handoff
            .transfer_id
            .as_ref()
            .and_then(Option::as_ref)
            .is_some()
}
pub fn visible_delta_run_ordinals(
    runs: &[OrchestrationV2Run],
    items: &[OrchestrationV2TurnItem],
) -> Result<ContextHandoffCoveredRunOrdinals> {
    let mut ordinals = vec![];
    for item in items {
        let value = t3_contracts::stored::to_value(item)?;
        for run in runs {
            if value["runId"] == t3_contracts::stored::to_value(&run.id)? {
                ordinals.push(run.ordinal);
                break;
            }
        }
    }
    Ok(ContextHandoffCoveredRunOrdinals {
        r#from: ordinals.iter().copied().min().unwrap_or(1),
        to: ordinals.into_iter().max().unwrap_or(1),
    })
}
pub fn visible_conversation_items(
    runs: &[OrchestrationV2Run],
    attempts: &[OrchestrationV2RunAttempt],
    items: Vec<OrchestrationV2TurnItem>,
) -> Result<Vec<OrchestrationV2TurnItem>> {
    let runs = t3_contracts::stored::to_value(&runs)?;
    let attempts = t3_contracts::stored::to_value(&attempts)?;
    let values = items
        .iter()
        .map(t3_contracts::stored::to_value)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut visible = vec![];
    for (item, value) in items.into_iter().zip(&values) {
        let status = runs
            .as_array()
            .expect("a serialized typed vector is an array")
            .iter()
            .rev()
            .find(|run| run["id"] == value["runId"])
            .and_then(|run| run["status"].as_str());
        if status == Some("rolled_back")
            || (status == Some("cancelled")
                && value["type"] == "user_message"
                && value["inputIntent"] == "queued_turn")
        {
            continue;
        }
        if value["type"] == "run_interrupt_result"
            && !value["runId"].is_null()
            && !value["nodeId"].is_null()
            && attempts
                .as_array()
                .expect("a serialized typed vector is an array")
                .iter()
                .any(|attempt| {
                    attempt["status"] == "superseded"
                        && attempt["runId"] == value["runId"]
                        && attempt["rootNodeId"] == value["nodeId"]
                })
            && !values.iter().any(|candidate| {
                candidate["type"] == "run_interrupt_request" && candidate["runId"] == value["runId"]
            })
        {
            continue;
        }
        visible.push(item);
    }
    Ok(visible)
}
async fn own_history(
    reader: &dyn ProjectionReader,
    thread: &OrchestrationV2AppThread,
    through: u64,
) -> Result<Vec<OrchestrationV2TurnItem>> {
    let records = reader
        .records(
            expanded_id(&thread.id)?,
            vec![RecordKind::Runs],
            RecordFilter::default(),
        )
        .await?;
    let mut runs = vec![];
    for records in records.records {
        if let ProjectionRecordSet::Runs(value) = records {
            runs = value;
        }
    }
    let ids = runs
        .iter()
        .filter(|run| run.ordinal <= through)
        .map(|run| expanded_id(&run.id))
        .collect::<Result<Vec<_>>>()?;
    let imported = t3_contracts::stored::to_value(thread)?["historyOrigin"] == "v1_import";
    let mut item_runs = ids.iter().cloned().map(Some).collect::<Vec<_>>();
    if imported {
        item_runs.push(None);
    }
    let records = reader
        .records(
            expanded_id(&thread.id)?,
            vec![RecordKind::Attempts, RecordKind::TurnItems],
            RecordFilter {
                run_ids: Some(ids),
                turn_item_run_ids: Some(item_runs),
                ..Default::default()
            },
        )
        .await?;
    let mut attempts = vec![];
    let mut own = vec![];
    for records in records.records {
        match records {
            ProjectionRecordSet::Attempts(value) => attempts = value,
            ProjectionRecordSet::TurnItems(value) => own = value,
            _ => {}
        }
    }
    visible_conversation_items(&runs, &attempts, own)
}
pub async fn read_inherited_handoff_items(
    reader: &dyn ProjectionReader,
    thread: &OrchestrationV2AppThread,
) -> Result<Vec<OrchestrationV2TurnItem>> {
    let mut current = thread.clone();
    let mut seen = HashSet::from([expanded_id(&thread.id)?]);
    let mut segments = vec![];
    loop {
        let value = t3_contracts::stored::to_value(&current)?;
        if value["forkedFrom"]["type"] != "run" || value["lineage"]["sideChat"] == true {
            break;
        }
        let source: ThreadId = serde_json::from_value(value["forkedFrom"]["threadId"].clone())?;
        if !seen.insert(source.clone()) {
            break;
        }
        let records = match reader
            .records(source, vec![RecordKind::Runs], RecordFilter::default())
            .await
        {
            Ok(value) => value,
            Err(Error::ThreadNotFound(_)) => break,
            Err(error) => return Err(error),
        };
        let ordinal = records.records.iter().find_map(|records| match records {
            ProjectionRecordSet::Runs(runs) => runs
                .iter()
                .find(|run| {
                    t3_contracts::stored::to_value(&run.id).ok().as_ref()
                        == Some(&value["forkedFrom"]["runId"])
                })
                .map(|run| run.ordinal),
            _ => None,
        });
        let Some(ordinal) = ordinal else {
            break;
        };
        segments.push(own_history(reader, &records.thread, ordinal).await?);
        current = records.thread;
    }
    segments.reverse();
    Ok(segments.into_iter().flatten().collect())
}
pub async fn read_fork_history_items(
    reader: &dyn ProjectionReader,
    thread: &OrchestrationV2AppThread,
    through: u64,
) -> Result<Vec<OrchestrationV2TurnItem>> {
    let mut items = read_inherited_handoff_items(reader, thread).await?;
    items.extend(own_history(reader, thread, through).await?);
    Ok(items)
}
fn legacy_summary(items: &[OrchestrationV2TurnItem]) -> Result<String> {
    let header = "Imported conversation history from the previous T3 Code orchestrator. Use it as context; do not repeat it unless the user asks.";
    let mut remaining = 32_000 - utf16_len(header) - 2;
    let mut sections = vec![];
    for item in items.iter().rev() {
        let item = t3_contracts::stored::to_value(item)?;
        let label = match item["type"].as_str() {
            Some("user_message") => "User",
            Some("assistant_message") => "Assistant",
            _ => continue,
        };
        if remaining == 0 {
            break;
        }
        let body = item["text"].as_str().unwrap_or_default();
        let full = format!("{label}:\n{body}");
        let section = if utf16_len(&full) <= remaining {
            full
        } else {
            let prefix = format!("{label}:\n... ");
            let Some(budget) = remaining
                .checked_sub(utf16_len(&prefix))
                .filter(|budget| *budget > 0)
            else {
                continue;
            };
            let skip = utf16_len(body).saturating_sub(budget);
            let mut units = 0;
            let mut start = body.len();
            for (index, ch) in body.char_indices() {
                if units >= skip {
                    start = index;
                    break;
                }
                units += ch.len_utf16();
            }
            if start > 0
                && !body[..start].chars().next_back().is_some_and(js_whitespace)
                && !body[start..].starts_with(js_whitespace)
                && let Some(next) = body[start..].find(js_whitespace)
            {
                start += next;
            }
            let suffix = body[start..].trim_start_matches(js_whitespace);
            if suffix.is_empty() {
                continue;
            }
            format!("{prefix}{suffix}")
        };
        remaining = remaining.saturating_sub(utf16_len(&section) + 2);
        sections.push(section);
    }
    sections.reverse();
    Ok(std::iter::once(header.to_owned())
        .chain(sections)
        .collect::<Vec<_>>()
        .join("\n\n"))
}
pub enum HandoffContent {
    LegacyImport,
    ForkDelta { source_thread_id: ThreadId },
    Provider { strategy: ContextHandoffStrategy },
    SideChat { parent_thread_id: ThreadId },
}
pub struct PrepareHandoff {
    pub thread_id: ThreadId,
    pub target_run_id: RunId,
    pub transfer_id: Option<ContextTransferId>,
    pub from_provider_thread_ids: Vec<ProviderThreadId>,
    pub to_provider_thread_id: ProviderThreadId,
    pub covered_run_ordinals: ContextHandoffCoveredRunOrdinals,
    pub items: Vec<OrchestrationV2TurnItem>,
    pub runs: Vec<OrchestrationV2Run>,
    pub created_at: IsoDateTime,
    pub content: HandoffContent,
}
pub struct ContextHandoffService {
    pub ids: Arc<dyn IdGenerator>,
    pub budget: Arc<dyn HandoffBudget>,
    pub token_cap: u64,
}
impl ContextHandoffService {
    pub fn prepare(&self, input: PrepareHandoff) -> Result<OrchestrationV2ContextHandoff> {
        let (strategy, summary, history) = match &input.content {
            HandoffContent::SideChat { parent_thread_id } => (
                ContextHandoffStrategy::ManualContext,
                make_side_chat_context_with(parent_thread_id, &input.items, summarize_delta_item)?,
                None,
            ),
            content => {
                let source = match content {
                    HandoffContent::ForkDelta { source_thread_id } => source_thread_id,
                    _ => &input.thread_id,
                };
                let coverage =
                    self.budget
                        .coverage(source, &input.covered_run_ordinals, &input.items);
                let mut messages = vec![];
                for item in &input.items {
                    if let Some(mut message) = self.budget.historical_message(item) {
                        if matches!(content, HandoffContent::Provider { .. })
                            && let Some(run_id) = message.run_id.as_ref()
                        {
                            message.run_status = input
                                .runs
                                .iter()
                                .find(|run| expanded_id(&run.id).ok().as_ref() == Some(run_id))
                                .map(|run| t3_contracts::stored::to_value(&run.status))
                                .transpose()?
                                .and_then(|status| {
                                    status.as_str().map(|status| Some(status.to_owned()))
                                });
                        }
                        messages.push(message);
                    }
                }
                let selected =
                    self.budget
                        .select(messages, coverage.clone(), 0, vec![], self.token_cap)?;
                let history = ContextHandoffHistory {
                    messages: selected.messages.clone(),
                    coverage,
                    omitted_items: selected.omitted_items,
                    omitted_item_ids: Some(Some(selected.omitted_item_ids)),
                };
                match content {
                    HandoffContent::LegacyImport => (
                        ContextHandoffStrategy::ManualContext,
                        legacy_summary(&input.items)?,
                        Some(history),
                    ),
                    HandoffContent::ForkDelta { source_thread_id } => {
                        let mut lines = vec![
                            "Merge-back context from forked conversation.".into(),
                            format!("Source thread: {}", source_thread_id.0),
                            format!("Target thread: {}", input.thread_id.0),
                            format!(
                                "Covered fork runs: {}-{}",
                                input.covered_run_ordinals.r#from, input.covered_run_ordinals.to
                            ),
                            "".into(),
                            "Fork delta:".into(),
                        ];
                        let digest = input
                            .items
                            .iter()
                            .map(summarize_delta_item)
                            .collect::<Result<Vec<_>>>()?
                            .into_iter()
                            .flatten()
                            .collect::<Vec<_>>();
                        if digest.is_empty() {
                            lines.push("- No user-visible delta items.".into());
                        } else {
                            lines.extend(digest);
                        }
                        (
                            ContextHandoffStrategy::ForkDeltaSummary,
                            lines.join("\n"),
                            Some(history),
                        )
                    }
                    HandoffContent::Provider { strategy } => (
                        strategy.clone(),
                        self.budget.render(&selected.messages, &selected.context),
                        Some(history),
                    ),
                    HandoffContent::SideChat { .. } => unreachable!(),
                }
            }
        };
        let value = json!({"id":self.ids.next(IdKind::ContextHandoff),"transferId":input.transfer_id,"threadId":input.thread_id,"targetRunId":input.target_run_id,"fromProviderThreadIds":input.from_provider_thread_ids,"toProviderThreadId":input.to_provider_thread_id,"coveredRunOrdinals":input.covered_run_ordinals,"strategy":strategy,"status":"ready","summaryMessageId":null,"summaryText":summary,"createdByProviderInstanceId":null,"createdAt":input.created_at,"updatedAt":input.created_at});
        let mut handoff: OrchestrationV2ContextHandoff = serde_json::from_value(value)?;
        handoff.history = history.map(Some);
        Ok(handoff)
    }
}
