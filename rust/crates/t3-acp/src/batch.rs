use std::{collections::HashMap, time::Duration};

use serde_json::Value;
use tokio::time::Instant;

use crate::SessionUpdate;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSnapshot {
    pub session_id: String,
    pub kind: String,
    pub message_id: Option<String>,
    pub segment: u64,
    pub text: String,
    pub complete: bool,
}

struct Segment {
    snapshot: TextSnapshot,
    dirty: bool,
    due: Option<Instant>,
}

/// Coalesces text into full snapshots, with an immediate first emission and a
/// final emission at segment boundaries. Tool updates remain on the raw event stream.
#[derive(Default)]
pub struct TextBatcher {
    segments: HashMap<String, Segment>,
    closed: HashMap<(String, String, String), TextSnapshot>,
    next_segment: u64,
}

impl TextBatcher {
    pub const INTERVAL: Duration = Duration::from_millis(50);

    pub fn push(&mut self, update: &SessionUpdate, now: Instant) -> Vec<TextSnapshot> {
        let kind = update.kind();
        let text = match kind {
            "agent_message_chunk" | "agent_thought_chunk" | "user_message_chunk" => update
                .update
                .pointer("/content/text")
                .and_then(Value::as_str),
            _ => None,
        };
        let Some(text) = text else {
            return if matches!(
                kind,
                "tool_call"
                    | "tool_call_update"
                    | "agent_message"
                    | "agent_thought"
                    | "user_message"
            ) || (kind == "state_update" && update.update["state"] == "idle")
            {
                self.finish(&update.session_id)
            } else {
                Vec::new()
            };
        };
        if text.is_empty() {
            return Vec::new();
        }
        let message_id = update.update["messageId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let mut result = Vec::new();
        if self
            .segments
            .get(&update.session_id)
            .is_some_and(|segment| {
                segment.snapshot.kind != kind
                    || (segment.snapshot.message_id.is_some()
                        && message_id.is_some()
                        && segment.snapshot.message_id != message_id)
            })
        {
            result.extend(self.finish(&update.session_id));
        }
        if let Some(segment) = self.segments.get_mut(&update.session_id) {
            // Agents may omit the first messageId; later IDs retain the original segment identity.
            if segment.snapshot.message_id.is_none() {
                segment.snapshot.message_id = message_id;
            }
            segment.snapshot.text.push_str(text);
            if !segment.dirty {
                segment.due = Some(now + Self::INTERVAL);
            }
            segment.dirty = true;
        } else {
            let previous = message_id
                .as_ref()
                .and_then(|id| {
                    self.closed
                        .get(&(update.session_id.clone(), kind.into(), id.clone()))
                })
                .cloned();
            let snapshot = if let Some(mut snapshot) = previous {
                // Agents can append to a named message after intervening tool updates.
                snapshot.text.push_str(text);
                snapshot.complete = false;
                snapshot
            } else {
                let snapshot = TextSnapshot {
                    session_id: update.session_id.clone(),
                    kind: kind.into(),
                    message_id,
                    segment: self.next_segment,
                    text: text.into(),
                    complete: false,
                };
                self.next_segment += 1;
                snapshot
            };
            result.push(snapshot.clone());
            self.segments.insert(
                update.session_id.clone(),
                Segment {
                    snapshot,
                    dirty: false,
                    due: None,
                },
            );
        }
        result
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.segments.values().filter_map(|s| s.due).min()
    }

    pub fn flush_due(&mut self, now: Instant) -> Vec<TextSnapshot> {
        let mut result = Vec::new();
        for segment in self.segments.values_mut() {
            if segment.dirty && segment.due.is_some_and(|due| due <= now) {
                segment.dirty = false;
                segment.due = None;
                result.push(segment.snapshot.clone());
            }
        }
        result.sort_by_key(|s| s.segment);
        result
    }

    pub fn finish(&mut self, session_id: &str) -> Vec<TextSnapshot> {
        // Agents can complete before a scheduled text flush; emit final text before terminal events.
        self.segments
            .remove(session_id)
            .map(|mut s| {
                s.snapshot.complete = true;
                if let Some(id) = &s.snapshot.message_id {
                    self.closed.insert(
                        (session_id.into(), s.snapshot.kind.clone(), id.clone()),
                        s.snapshot.clone(),
                    );
                }
                vec![s.snapshot]
            })
            .unwrap_or_default()
    }

    /// Finish a turn and release named-message history retained across tool calls.
    pub fn end_turn(&mut self, session_id: &str) -> Vec<TextSnapshot> {
        let result = self.finish(session_id);
        self.closed
            .retain(|(session, _, _), _| session != session_id);
        result
    }

    pub fn finish_all(&mut self) -> Vec<TextSnapshot> {
        self.closed.clear();
        let mut result: Vec<_> = self
            .segments
            .drain()
            .map(|(_, mut s)| {
                s.snapshot.complete = true;
                s.snapshot
            })
            .collect();
        result.sort_by_key(|s| s.segment);
        result
    }
}
