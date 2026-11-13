use super::thread_history_paging::id_text;
use crate::provider::{ProviderEvent, ProviderEventKind};
use t3_contracts::orchestration_v2::*;

#[derive(Debug, Clone, Default)]
struct AssistantTextScan {
    text: String,
    line_start: usize,
    open_fence: Option<(u8, usize, usize)>,
    boundary: Option<usize>,
    title_awaiting_content: bool,
}
fn list_start(line: &str, partial: bool) -> bool {
    let line = line.trim_start_matches([' ', '\t']);
    let bytes = line.as_bytes();
    let marker_end = if bytes.first().is_some_and(|b| b"-*+".contains(b)) {
        1
    } else {
        let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
        if digits == 0 || digits > 9 || !bytes.get(digits).is_some_and(|b| b".)".contains(b)) {
            return false;
        }
        digits + 1
    };
    if !bytes.get(marker_end).is_some_and(|b| b" \t".contains(b)) {
        return false;
    }
    !partial
        || !line[marker_end + 1..]
            .trim_start_matches([' ', '\t', '\r'])
            .is_empty()
}
fn heading(line: &str) -> bool {
    let count = line.bytes().take_while(|b| *b == b'#').count();
    (1..=6).contains(&count)
        && line
            .as_bytes()
            .get(count)
            .is_none_or(|b| b" \t".contains(b))
}
fn section_title(line: &str) -> bool {
    let spaces = line.bytes().take_while(|b| *b == b' ').count();
    if spaces > 3 {
        return false;
    }
    let line = &line[spaces..];
    if heading(line) {
        return true;
    }
    let line = line.strip_suffix(':').unwrap_or(line);
    line.strip_prefix("**")
        .and_then(|s| s.strip_suffix("**"))
        .is_some_and(|s| !s.is_empty() && !s.contains("**"))
}
fn advance_scan(scan: &mut AssistantTextScan, text: &str) {
    let resumed = text.starts_with(&scan.text);
    let mut search_from = if resumed { scan.text.len() } else { 0 };
    if !resumed {
        *scan = AssistantTextScan::default();
    }
    while let Some(offset) = text[search_from..].find('\n') {
        let newline = search_from + offset;
        let line = text[scan.line_start..newline].trim_end_matches([' ', '\t', '\r']);
        if scan.open_fence.is_none()
            && scan.line_start > 0
            && !scan.title_awaiting_content
            && list_start(line, false)
        {
            scan.boundary = Some(scan.line_start);
        }
        let indent = line.bytes().take_while(|b| *b == b' ').count();
        let marker = line.as_bytes().get(indent).copied();
        let length = marker.map_or(0, |marker| {
            line.as_bytes()[indent..]
                .iter()
                .take_while(|b| **b == marker)
                .count()
        });
        if marker.is_some_and(|m| b"`~".contains(&m)) && length >= 3 {
            if scan.open_fence.is_none() {
                scan.open_fence = marker.map(|m| (m, length, indent));
                scan.title_awaiting_content = false;
            } else if let Some((old_marker, old_length, old_indent)) = scan.open_fence
                && marker == Some(old_marker)
                && length >= old_length
                && indent <= old_indent + 3
                && line.len() == indent + length
            {
                scan.open_fence = None;
                scan.boundary = Some(newline + 1);
            }
        } else if scan.open_fence.is_none()
            && line.bytes().all(|b| b" \t".contains(&b))
            && scan.line_start > 0
        {
            if !scan.title_awaiting_content {
                scan.boundary = Some(newline + 1);
            }
        } else if scan.open_fence.is_none() {
            if scan.line_start > 0 && !scan.title_awaiting_content && heading(line) {
                scan.boundary = Some(scan.line_start);
            }
            scan.title_awaiting_content = section_title(line);
        }
        scan.line_start = newline + 1;
        search_from = scan.line_start;
    }
    scan.text.clear();
    scan.text.push_str(text);
}
fn ready_boundary(scan: &AssistantTextScan) -> Option<usize> {
    if scan.open_fence.is_none()
        && scan.line_start > 0
        && !scan.title_awaiting_content
        && list_start(&scan.text[scan.line_start..], true)
    {
        Some(scan.line_start)
    } else {
        scan.boundary
    }
}
pub fn split_buffered_assistant_text(text: &str) -> (&str, &str) {
    let mut scan = AssistantTextScan::default();
    advance_scan(&mut scan, text);
    text.split_at(ready_boundary(&scan).unwrap_or(0))
}

struct StreamTrack {
    key: String,
    text: String,
    at: Option<i64>,
    held: Option<ProviderEvent>,
    due_at: i64,
    scan: AssistantTextScan,
}
pub struct AssistantStreamingFilter {
    turn_mode: bool,
    idle_flush_ms: i64,
    tracks: Vec<StreamTrack>,
    next_due_at: Option<i64>,
}
fn streamed_message(event: &ProviderEvent) -> Option<(String, &str, bool)> {
    match &event.kind {
        ProviderEventKind::Message(message)
            if message.role == ConversationMessageRole::Assistant =>
        {
            Some((
                format!("message.updated:{}", id_text(&message.id)),
                &message.text,
                message.streaming,
            ))
        }
        ProviderEventKind::TurnItem { item, .. } => match item.as_ref() {
            OrchestrationV2TurnItem::AssistantMessage(item) => Some((
                format!("turn_item.updated:{}", id_text(&item.id)),
                &item.text,
                item.streaming,
            )),
            OrchestrationV2TurnItem::Reasoning(item) => Some((
                format!("turn_item.updated:{}", id_text(&item.id)),
                &item.text,
                item.streaming,
            )),
            _ => None,
        },
        _ => None,
    }
}
fn with_text(event: &mut ProviderEvent, text: String) {
    match &mut event.kind {
        ProviderEventKind::Message(message) => message.text = text,
        ProviderEventKind::TurnItem { item, .. } => match item.as_mut() {
            OrchestrationV2TurnItem::AssistantMessage(item) => item.text = text,
            OrchestrationV2TurnItem::Reasoning(item) => item.text = text,
            _ => {}
        },
        _ => {}
    }
}
impl AssistantStreamingFilter {
    pub fn new(turn_mode: bool, idle_flush_ms: i64) -> Self {
        Self {
            turn_mode,
            idle_flush_ms,
            tracks: Vec::new(),
            next_due_at: None,
        }
    }
    pub fn next_idle_flush_at(&self) -> Option<i64> {
        self.next_due_at
    }
    pub fn filter(&mut self, mut event: ProviderEvent, now: i64) -> Option<ProviderEvent> {
        if matches!(&event.kind,ProviderEventKind::Node(node) if node.kind == ExecutionNodeKind::AssistantMessage && node.status == ExecutionNodeStatus::Running)
        {
            return None;
        }
        let Some((key, text, streaming)) = streamed_message(&event) else {
            return Some(event);
        };
        if !streaming {
            self.tracks.retain(|t| t.key != key);
            return Some(event);
        }
        if self.turn_mode {
            return None;
        }
        let index = self
            .tracks
            .iter()
            .position(|t| t.key == key)
            .unwrap_or_else(|| {
                self.tracks.push(StreamTrack {
                    key,
                    text: String::new(),
                    at: None,
                    held: None,
                    due_at: 0,
                    scan: AssistantTextScan::default(),
                });
                self.tracks.len() - 1
            });
        let track = &mut self.tracks[index];
        let throttled = track.at.is_some_and(|at| now - at < 400);
        if !throttled {
            advance_scan(&mut track.scan, text);
        }
        let boundary = ready_boundary(&track.scan).unwrap_or(0);
        let ready = if throttled { "" } else { &text[..boundary] };
        let hold = throttled || track.text.starts_with(ready);
        let ready = (!hold).then(|| ready.to_owned());
        let held = if hold {
            text != track.text
        } else {
            boundary != text.len()
        };
        track.due_at = now + self.idle_flush_ms;
        if held {
            track.held = Some(event.clone());
            if self.next_due_at.is_none_or(|due| track.due_at < due) {
                self.next_due_at = Some(track.due_at);
            }
        } else {
            track.held = None;
        }
        if let Some(ready) = ready {
            track.text.clone_from(&ready);
            track.at = Some(now);
            with_text(&mut event, ready);
            Some(event)
        } else {
            None
        }
    }
    pub fn take_idle_flushes(&mut self, now: i64) -> Vec<ProviderEvent> {
        self.next_due_at = None;
        let mut due = Vec::new();
        for track in &mut self.tracks {
            if track.held.is_none() {
                continue;
            }
            if track.due_at > now {
                if self.next_due_at.is_none_or(|old| track.due_at < old) {
                    self.next_due_at = Some(track.due_at);
                }
                continue;
            }
            if let Some(event) = track.held.take() {
                if let Some((_, text, _)) = streamed_message(&event) {
                    track.text = text.into();
                }
                track.at = Some(now);
                track.due_at = now;
                due.push(event);
            }
        }
        due
    }
}
