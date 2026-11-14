//! Port of chat/timelineMinimapItems.ts; structural changes and streamed row updates are separate.

use crate::thread_views::messages_timeline::{MessagesTimelineRow, MessagesTimelineRowContent};
use crate::thread_views::user_message::is_attachment_only_bootstrap_prompt;
use crate::view_logic::session::{ChatMessage, thread_links::replace_thread_links};
use std::rc::Rc;

#[derive(Clone, Debug)]
pub struct TimelineMinimapItem {
    pub id: String,
    pub row_index: usize,
    user_text: Option<Rc<str>>,
    assistant: Option<(usize, Rc<ChatMessage>)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimelineMinimapPreview {
    pub user_text: Option<Rc<str>>,
    pub assistant_text: Option<Rc<str>>,
}

#[derive(Default)]
pub struct TimelineMinimapItems {
    items: Vec<TimelineMinimapItem>,
    pub row_visits: usize,
    pub preview_bytes: usize,
}

impl TimelineMinimapItems {
    /// Initial load, prepend, remove or reorder only. Never call for a text delta.
    pub fn replace(&mut self, rows: &[Rc<MessagesTimelineRow>]) {
        self.items.clear();
        for (row_index, row) in rows.iter().enumerate() {
            self.row_visits += 1;
            let MessagesTimelineRowContent::Message { message, .. } = &row.content else {
                continue;
            };
            if message.role == "user" {
                self.items.push(TimelineMinimapItem {
                    id: row.id.clone(),
                    row_index,
                    user_text: user_preview(&message.text),
                    assistant: None,
                });
            } else if message.role == "assistant"
                && let Some(last) = self.items.last_mut()
            {
                last.assistant = Some((row_index, message.clone()));
            }
        }
    }

    /// O(log turns), pointer copies only; final-assistant selection agrees with the web scan.
    pub fn update_row(&mut self, row_index: usize, row: &MessagesTimelineRow) {
        self.row_visits += 1;
        let Some(at) = self
            .items
            .partition_point(|item| item.row_index <= row_index)
            .checked_sub(1)
        else {
            return;
        };
        let item = &mut self.items[at];
        let MessagesTimelineRowContent::Message { message, .. } = &row.content else {
            return;
        };
        if message.role == "user" && row_index == item.row_index && row.id == item.id {
            item.user_text = user_preview(&message.text);
        } else if message.role == "assistant"
            && item
                .assistant
                .as_ref()
                .is_none_or(|(last, _)| row_index >= *last)
        {
            item.assistant = Some((row_index, message.clone()));
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn item(&self, index: usize) -> Option<&TimelineMinimapItem> {
        self.items.get(index)
    }

    pub fn current_index(&self, start: usize, end: usize) -> Option<usize> {
        let next = self.items.partition_point(|item| item.row_index < start);
        if self
            .items
            .get(next)
            .is_some_and(|item| item.row_index <= end)
        {
            Some(next)
        } else {
            next.checked_sub(1)
        }
    }

    pub fn visible_turns(&self, start: usize, end: usize) -> std::ops::Range<usize> {
        let first = self.items.partition_point(|item| item.row_index < start);
        let last = self.items.partition_point(|item| item.row_index <= end);
        first..last
    }

    pub fn preview(&mut self, index: usize) -> Option<TimelineMinimapPreview> {
        self.preview_with_thread_label(index, "a thread")
    }

    pub fn preview_with_thread_label(
        &mut self,
        index: usize,
        thread_label: &str,
    ) -> Option<TimelineMinimapPreview> {
        let item = self.items.get(index)?;
        let user_text = item.user_text.clone();
        let assistant_text = item.assistant.as_ref().and_then(|(_, message)| {
            let mut units = 0;
            let end = message
                .text
                .char_indices()
                .find_map(|(at, c)| {
                    units += c.len_utf16();
                    (units > 1_500).then_some(at)
                })
                .unwrap_or(message.text.len());
            let source = &message.text[..end];
            self.preview_bytes += source.len();
            let source = replace_thread_links(source, |_, label| {
                Some(label.filter(|label| crate::view_logic::session::markdown_link_presentation::parse_thread_link_href(label).is_none()).unwrap_or(thread_label).to_owned())
            });
            // Reuse the shared rendered Markdown policy rather than maintain another parser.
            compact(&t3_shared::find_text::markdown_segments(&source, false, None, &[]).join(" "))
                .map(Rc::from)
        });
        Some(TimelineMinimapPreview {
            user_text,
            assistant_text,
        })
    }
}

fn compact(text: &str) -> Option<String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

fn user_preview(text: &str) -> Option<Rc<str>> {
    if is_attachment_only_bootstrap_prompt(text) {
        None
    } else {
        compact(text).map(Rc::from)
    }
}
