//! Port of apps/web/src/components/chat/turnDetailAnnouncement.ts: what a screen reader is told
//! about the latest turn beyond its status (the agent's finished messages and its current step),
//! for ThreadActivityAnnouncer's "messages" and "everything" levels.
//!
//! The walk reads typed entries only (no serialization) and stops at the turn's start, so its
//! work is bounded by the latest turn, not the thread. Copy is returned as structure (a step's
//! verb and subject, a message's text and whether it was cut) so the UI translates it.

use std::collections::BTreeMap;
use std::rc::Rc;

use t3_contracts::orchestration_v2::{
    OrchestrationV2TurnItem, OrchestrationV2TurnItemStatus, OrchestrationV2UserMessageInputIntent,
    RunAttemptStatus,
};
use t3_contracts::orchestration_v2_id_table::IdOrIndex;
use t3_contracts::settings::ScreenReaderAnnouncements;

use super::activity_announcement::ThreadActivityStatus;
use crate::thread_views::messages_timeline::live_work_entry_label;
use crate::view_logic::session::{ChatMessage, TimelineContent, TimelineEntry, WorkLogEntry};

const ACTIVITY_MAX_LENGTH: usize = 120;
// Parsing is synchronous, and nobody listens to a longer message through a live region.
const MESSAGE_MAX_LENGTH: usize = 2_000;

/// One of the agent's messages in the latest turn.
#[derive(Clone, Debug, PartialEq)]
pub struct TurnMessage {
    pub id: String,
    /// The message itself (its text is Markdown), shared: a streamed token copies nothing.
    pub message: Rc<ChatMessage>,
    /// The agent stopped writing it and went on working. A turn's closing reply never is.
    pub finished: bool,
    /// It arrived after the entry the caller last saw, unlike history loaded above it.
    pub fresh: bool,
}

/// The step the agent is on, as the UI words it ("Editing {subject}", ...).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepLabel {
    Thinking,
    Editing(String),
    SearchingFor(String),
    SearchingTheWebFor(String),
    /// A label spoken as it is (a command, a tool title).
    Plain(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TurnDetail {
    pub activity: Option<StepLabel>,
    /// Oldest first.
    pub messages: Vec<TurnMessage>,
    /// The thread's newest entry.
    pub tail_id: Option<String>,
}

/// Each message seen in the turn and whether it was finished then, the step last spoken, and the
/// newest entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TurnDetailMemory {
    pub activity: Option<StepLabel>,
    pub messages: BTreeMap<String, bool>,
    pub tail_id: Option<String>,
}

/// A finished message to announce: its plain text, and whether it was cut ("More in the thread.").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnouncedMessage {
    pub text: String,
    pub truncated: bool,
}

/// `cutText`: the first `max` characters of `text`, or none when it has no more than that.
fn cut_text(text: &str, max: usize) -> Option<&str> {
    let (end, _) = text.char_indices().nth(max)?;
    Some(&text[..end])
}

/// An id as its wire text (an unresolved table index as its number).
fn id_text<T: serde::Serialize>(id: &IdOrIndex<T>) -> String {
    match serde_json::to_value(id) {
        Ok(serde_json::Value::String(text)) => text,
        Ok(other) => other.to_string(),
        Err(_) => String::new(),
    }
}

/// `timelineEntryRunId`, typed.
fn entry_run_id(entry: &TimelineEntry) -> Option<String> {
    let run = match &entry.content {
        TimelineContent::Message { message, .. } => {
            return (message.role == "assistant")
                .then(|| message.run_id.as_ref().map(|run| id_text(run)))
                .flatten();
        }
        TimelineContent::ProposedPlan { proposed_plan } => {
            return proposed_plan["runId"].as_str().map(str::to_owned);
        }
        TimelineContent::HtmlRender { run_id, .. }
        | TimelineContent::SentMessage { run_id, .. }
        | TimelineContent::McpApp { run_id, .. } => run_id.as_ref(),
        TimelineContent::Work { entry } => entry.run_id.as_ref(),
        TimelineContent::Event { .. } => None,
    };
    run.map(|run| id_text(run))
}

/// `timelineEntryStartsResponse`: a user's prompt (not a steer) or a notification.
fn starts_response(entry: &TimelineEntry) -> bool {
    match &entry.content {
        TimelineContent::Message { message, .. } => {
            message.role == "user"
                && !matches!(
                    message.input_intent,
                    Some(
                        OrchestrationV2UserMessageInputIntent::Steer
                            | OrchestrationV2UserMessageInputIntent::PromotedQueuedToSteer
                    )
                )
        }
        TimelineContent::Work { entry } => entry.item_type.as_deref() == Some("notification"),
        _ => false,
    }
}

// On screen these labels are a path or a query beside an icon.
fn with_verb(work: &WorkLogEntry, label: String) -> StepLabel {
    match work.item_type.as_deref() {
        Some("file_change") => return StepLabel::Editing(label),
        Some("file_search" | "web_search") => {}
        _ => return StepLabel::Plain(label),
    }
    let lower = label.to_lowercase();
    let already = ["search ", "searched ", "searching "]
        .iter()
        .any(|verb| lower.starts_with(verb))
        || ["search", "searched", "searching"].iter().any(|verb| {
            lower
                .strip_prefix(verb)
                .and_then(|rest| rest.chars().next())
                .is_some_and(char::is_whitespace)
        });
    if already {
        StepLabel::Plain(label)
    } else if work.item_type.as_deref() == Some("web_search") {
        StepLabel::SearchingTheWebFor(label)
    } else {
        StepLabel::SearchingFor(label)
    }
}

fn step_label(work: &WorkLogEntry, workspace_root: Option<&str>) -> Option<StepLabel> {
    // Reasoning text is never read out.
    if work.item_type.as_deref() == Some("reasoning") {
        return Some(StepLabel::Thinking);
    }
    if matches!(
        work.tool_lifecycle_status.as_deref(),
        Some("failed" | "declined" | "stopped")
    ) {
        return None;
    }
    let label = live_work_entry_label(work, workspace_root, true)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if label.is_empty() {
        return None;
    }
    // The cut applies to the spoken words, verb included, as the web's `cutText(spoken)`.
    let step = with_verb(work, label);
    let verb = match &step {
        StepLabel::Editing(_) => "Editing ".len(),
        StepLabel::SearchingFor(_) => "Searching for ".len(),
        StepLabel::SearchingTheWebFor(_) => "Searching the web for ".len(),
        _ => 0,
    };
    let cut = |subject: String| match cut_text(&subject, ACTIVITY_MAX_LENGTH.saturating_sub(verb)) {
        Some(cut) => format!("{cut}…"),
        None => subject,
    };
    Some(match step {
        StepLabel::Editing(subject) => StepLabel::Editing(cut(subject)),
        StepLabel::SearchingFor(subject) => StepLabel::SearchingFor(cut(subject)),
        StepLabel::SearchingTheWebFor(subject) => StepLabel::SearchingTheWebFor(cut(subject)),
        StepLabel::Plain(subject) => StepLabel::Plain(cut(subject)),
        StepLabel::Thinking => StepLabel::Thinking,
    })
}

// A steer starts a new attempt in the same run. The reply it cut off also stops streaming, so
// only one that completed before the steer finished.
fn completed_before_superseded(entry: &TimelineEntry) -> bool {
    let TimelineContent::Message {
        projected_item: Some(projected),
        ..
    } = &entry.content
    else {
        return false;
    };
    let OrchestrationV2TurnItem::AssistantMessage(item) = &projected.item else {
        return false;
    };
    let superseded_at = entry
        .attempt
        .as_ref()
        .and_then(|attempt| attempt.completed_at.as_ref())
        .and_then(|at| crate::time::epoch_ms(&at.0));
    let completed_at = item
        .completed_at
        .as_ref()
        .and_then(|at| crate::time::epoch_ms(&at.0));
    item.status == OrchestrationV2TurnItemStatus::Completed
        && matches!((completed_at, superseded_at), (Some(done), Some(cut)) if done < cut)
}

/// `turnDetail`: the latest turn's step and messages, read from the thread's entries before any
/// are hidden.
pub fn turn_detail(
    entries: &[Rc<TimelineEntry>],
    workspace_root: Option<&str>,
    known_tail_id: Option<&str>,
) -> TurnDetail {
    let mut messages: Vec<TurnMessage> = Vec::new();
    // Runs and attempts the agent kept working in after the entry being looked at.
    let mut continued_runs: Vec<Option<String>> = Vec::new();
    let mut continued_attempts: Vec<String> = Vec::new();
    let mark = |entry: &TimelineEntry,
                run: Option<String>,
                runs: &mut Vec<Option<String>>,
                attempts: &mut Vec<String>| {
        runs.push(run);
        if let Some(attempt) = &entry.attempt {
            attempts.push(id_text(&attempt.id));
        }
    };
    let mut activity: Option<Option<StepLabel>> = None;
    let mut fresh = true;
    let mut index = entries.len();
    while index > 0 {
        let entry = entries[index - 1].as_ref();
        if starts_response(entry) {
            break;
        }
        index -= 1;
        if Some(entry.id.as_str()) == known_tail_id {
            fresh = false;
        }
        match &entry.content {
            TimelineContent::Event { .. } => continue,
            TimelineContent::Work { entry: work } => {
                if work.source_activity_kind.as_deref() == Some("context-compaction") {
                    if activity.is_none() {
                        activity = Some(
                            (work.tool_lifecycle_status.as_deref() == Some("inProgress"))
                                .then(|| StepLabel::Plain(work.label.clone())),
                        );
                    }
                    continue;
                }
                // Notices, todo lists and checkpoints sit beside the turn's steps without being one.
                if work.tone != "tool" && work.item_type.as_deref() != Some("reasoning") {
                    continue;
                }
                mark(
                    entry,
                    work.run_id.as_ref().map(|run| id_text(run)),
                    &mut continued_runs,
                    &mut continued_attempts,
                );
                // A request is spoken as the thread's status; once answered, its step goes on.
                if matches!(
                    work.item_type.as_deref(),
                    Some("approval_request" | "user_input_request")
                ) {
                    continue;
                }
                if activity.is_none() {
                    activity = Some(step_label(work, workspace_root));
                }
                continue;
            }
            TimelineContent::Message { message, .. } if message.role != "assistant" => continue,
            _ => {}
        }
        // A reply, plan or rendered page is newer than any step before it.
        if activity.is_none() {
            activity = Some(None);
        }
        let run = entry_run_id(entry);
        if let TimelineContent::Message { message, .. } = &entry.content {
            let attempt_continues = entry.attempt.as_ref().is_none_or(|attempt| {
                attempt.status != RunAttemptStatus::Superseded
                    || continued_attempts.contains(&id_text(&attempt.id))
                    || completed_before_superseded(entry)
            });
            messages.push(TurnMessage {
                id: id_text(&message.id),
                message: message.clone(),
                finished: !message.streaming && continued_runs.contains(&run) && attempt_continues,
                fresh,
            });
        }
        mark(entry, run, &mut continued_runs, &mut continued_attempts);
    }
    // Nothing is known to be new when the entry last seen is gone, or there was none.
    let mut tail_known = !fresh;
    if let Some(known) = known_tail_id {
        while !tail_known && index > 0 {
            index -= 1;
            tail_known = entries[index].id == known;
        }
    }
    if !tail_known {
        for message in &mut messages {
            message.fresh = false;
        }
    }
    messages.reverse();
    TurnDetail {
        activity: activity.flatten(),
        messages,
        tail_id: entries.last().map(|entry| entry.id.clone()),
    }
}

/// `markdownPlainText`: one line of the text the Markdown renders, without markup or link
/// targets (the shared find text's rendering).
fn markdown_plain_text(markdown: &str) -> String {
    t3_shared::find_text::markdown_segments(markdown, false, None, &[])
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `nextTurnDetailAnnouncement`: which of the turn's messages and steps a screen reader hears
/// next. The first snapshot of a thread is a silent baseline, like its status.
pub fn next_turn_detail_announcement(
    previous: Option<&TurnDetailMemory>,
    detail: &TurnDetail,
    level: &ScreenReaderAnnouncements,
    status: ThreadActivityStatus,
) -> (TurnDetailMemory, Vec<AnnouncedMessage>, Option<StepLabel>) {
    let memory = TurnDetailMemory {
        // A step is spoken again after a pause for an approval or a question.
        activity: (status == ThreadActivityStatus::Working)
            .then(|| detail.activity.clone())
            .flatten(),
        messages: detail
            .messages
            .iter()
            .map(|message| (message.id.clone(), message.finished))
            .collect(),
        tail_id: detail.tail_id.clone(),
    };
    // A turn that already ended is history, however late its entries arrive.
    let Some(previous) = previous.filter(|_| {
        *level != ScreenReaderAnnouncements::Status && status != ThreadActivityStatus::Idle
    }) else {
        return (memory, Vec::new(), None);
    };
    let messages = detail
        .messages
        .iter()
        .filter(|message| {
            message.finished
                && previous.messages.get(&message.id) != Some(&true)
                && (message.fresh || previous.messages.contains_key(&message.id))
        })
        .filter_map(|message| {
            let text = message.message.text.as_str();
            let (source, truncated) = match cut_text(text, MESSAGE_MAX_LENGTH) {
                Some(cut) => (cut, true),
                None => (text, false),
            };
            let text = markdown_plain_text(source);
            (!text.is_empty()).then_some(AnnouncedMessage { text, truncated })
        })
        .collect();
    let activity = (*level == ScreenReaderAnnouncements::Everything
        && memory.activity != previous.activity)
        .then(|| memory.activity.clone())
        .flatten();
    (memory, messages, activity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const AT: &str = "2026-01-01T00:00:00.000Z";

    fn entry(value: Value) -> Rc<TimelineEntry> {
        Rc::new(serde_json::from_value(value).expect("timeline entry"))
    }

    fn message(id: &str, role: &str, extra: Value) -> Rc<TimelineEntry> {
        let mut message = json!({"id":id,"role":role,"text":id,
            "runId":if role == "assistant" {json!("run-1")} else {Value::Null},
            "streaming":false,"createdAt":AT,"updatedAt":AT});
        message
            .as_object_mut()
            .expect("message")
            .extend(extra.as_object().expect("extra").clone());
        entry(json!({"id":id,"kind":"message","createdAt":AT,"message":message}))
    }

    fn work(id: &str, extra: Value) -> Rc<TimelineEntry> {
        let mut work = json!({"id":id,"createdAt":AT,"runId":"run-1","label":id,"tone":"tool"});
        work.as_object_mut()
            .expect("work")
            .extend(extra.as_object().expect("extra").clone());
        entry(json!({"id":id,"kind":"work","createdAt":AT,"entry":work}))
    }

    fn command(id: &str) -> Rc<TimelineEntry> {
        work(
            id,
            json!({"itemType":"command_execution","command":"git status","toolLifecycleStatus":"inProgress"}),
        )
    }

    #[test]
    fn a_message_the_agent_worked_past_is_finished_and_the_latest_step_is_the_activity() {
        let entries = [
            message("prompt", "user", json!({})),
            message("early", "assistant", json!({})),
            command("step"),
        ];
        let detail = turn_detail(&entries, None, None);
        assert_eq!(detail.messages.len(), 1);
        assert!(detail.messages[0].finished, "work followed it in its run");
        assert!(matches!(detail.activity, Some(StepLabel::Plain(_))));
        assert_eq!(detail.tail_id.as_deref(), Some("step"));
        let closing = [
            message("prompt", "user", json!({})),
            message("last", "assistant", json!({})),
        ];
        let detail = turn_detail(&closing, None, None);
        assert!(
            !detail.messages[0].finished,
            "a turn's closing reply never is"
        );
        assert_eq!(detail.activity, None);
    }

    #[test]
    fn a_steer_continues_the_turn_and_a_new_prompt_ends_the_walk() {
        let entries = [
            message("old", "assistant", json!({})),
            message("prompt", "user", json!({})),
            message("steer", "user", json!({"inputIntent":"steer"})),
            message("early", "assistant", json!({})),
            command("step"),
        ];
        let detail = turn_detail(&entries, None, None);
        let ids: Vec<_> = detail.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            ["early"],
            "messages before the prompt are another turn"
        );
    }

    #[test]
    fn notices_are_not_steps_and_reasoning_is_spoken_as_thinking() {
        let entries = [
            message("prompt", "user", json!({})),
            work(
                "think",
                json!({"itemType":"reasoning","detail":"secret thoughts"}),
            ),
            work(
                "notice",
                json!({"itemType":"system_notice","tone":"info","label":"Heads up"}),
            ),
        ];
        assert_eq!(
            turn_detail(&entries, None, None).activity,
            Some(StepLabel::Thinking)
        );
    }

    #[test]
    fn edits_and_searches_get_their_verbs_and_long_steps_are_cut() {
        let edit = work(
            "e",
            json!({"itemType":"file_change","toolTitle":"src/a.ts","toolLifecycleStatus":"inProgress"}),
        );
        let entries = [message("prompt", "user", json!({})), edit];
        assert!(matches!(
            turn_detail(&entries, None, None).activity,
            Some(StepLabel::Editing(_))
        ));
        let long = "x".repeat(300);
        let entries = [
            message("prompt", "user", json!({})),
            work(
                "long",
                json!({"itemType":"command_execution","command":long,"toolLifecycleStatus":"inProgress"}),
            ),
        ];
        match turn_detail(&entries, None, None).activity {
            Some(StepLabel::Plain(text)) => {
                assert!(text.ends_with('…'));
                assert_eq!(text.chars().count(), ACTIVITY_MAX_LENGTH + 1);
            }
            other => panic!("a cut command, got {other:?}"),
        }
    }

    #[test]
    fn the_first_snapshot_is_silent_and_finished_messages_are_spoken_once() {
        let level = ScreenReaderAnnouncements::Messages;
        let working = ThreadActivityStatus::Working;
        let start = [
            message("prompt", "user", json!({})),
            message(
                "early",
                "assistant",
                json!({"text":"**Done** with [the plan](https://x.dev)"}),
            ),
        ];
        let first = turn_detail(&start, None, None);
        let (memory, spoken, _) = next_turn_detail_announcement(None, &first, &level, working);
        assert!(spoken.is_empty(), "baseline");
        let mut next = start.to_vec();
        next.push(command("step"));
        let detail = turn_detail(&next, None, memory.tail_id.as_deref());
        let (memory, spoken, activity) =
            next_turn_detail_announcement(Some(&memory), &detail, &level, working);
        assert_eq!(
            spoken,
            vec![AnnouncedMessage {
                text: "Done with the plan".into(),
                truncated: false
            }]
        );
        assert_eq!(activity, None, "steps only at the everything level");
        let (_, again, _) = next_turn_detail_announcement(Some(&memory), &detail, &level, working);
        assert!(again.is_empty(), "never twice");
        let (_, at_status, _) = next_turn_detail_announcement(
            Some(&memory),
            &detail,
            &ScreenReaderAnnouncements::Status,
            working,
        );
        assert!(at_status.is_empty());
    }

    #[test]
    fn steps_are_spoken_at_the_everything_level_when_they_change() {
        let level = ScreenReaderAnnouncements::Everything;
        let working = ThreadActivityStatus::Working;
        let one = [message("prompt", "user", json!({})), command("a")];
        let (memory, _, _) =
            next_turn_detail_announcement(None, &turn_detail(&one, None, None), &level, working);
        let mut two = one.to_vec();
        two.push(work(
            "b",
            json!({"itemType":"file_change","toolTitle":"src/b.ts","toolLifecycleStatus":"inProgress"}),
        ));
        let (_, _, activity) = next_turn_detail_announcement(
            Some(&memory),
            &turn_detail(&two, None, Some("a")),
            &level,
            working,
        );
        assert!(matches!(activity, Some(StepLabel::Editing(_))));
    }
}
