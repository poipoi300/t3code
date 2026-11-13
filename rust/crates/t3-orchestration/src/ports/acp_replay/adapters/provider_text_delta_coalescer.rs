use std::time::Duration;
use tokio::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTextDeltaUpdate {
    pub turn_id: String,
    pub item_id: String,
    pub text: String,
    pub completed: bool,
}

struct BufferedText {
    update: ProviderTextDeltaUpdate,
    dirty: bool,
}

pub struct ProviderTextDeltaCoalescer {
    interval: Duration,
    deadline: Option<Instant>,
    buffered: Vec<BufferedText>,
}

impl ProviderTextDeltaCoalescer {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval: interval.max(Duration::from_millis(1)),
            deadline: None,
            buffered: Vec::new(),
        }
    }
    pub fn append(&mut self, turn_id: &str, item_id: &str, delta: &str) {
        if delta.is_empty() {
            return;
        }
        if let Some(message) = self
            .buffered
            .iter_mut()
            .find(|message| message.update.turn_id == turn_id && message.update.item_id == item_id)
        {
            message.update.text.push_str(delta);
            message.dirty = true;
        } else {
            self.buffered.push(BufferedText {
                update: ProviderTextDeltaUpdate {
                    turn_id: turn_id.into(),
                    item_id: item_id.into(),
                    text: delta.into(),
                    completed: false,
                },
                dirty: true,
            });
        }
        self.deadline
            .get_or_insert_with(|| Instant::now() + self.interval);
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.deadline
    }
    pub fn flush_due(&mut self) -> Vec<ProviderTextDeltaUpdate> {
        if self
            .deadline
            .is_none_or(|deadline| deadline > Instant::now())
        {
            return Vec::new();
        }
        self.deadline = None;
        self.buffered
            .iter_mut()
            .filter_map(|message| {
                if !message.dirty {
                    return None;
                }
                message.dirty = false;
                Some(message.update.clone())
            })
            .collect()
    }
    pub fn complete(
        &mut self,
        turn_id: &str,
        item_id: &str,
        final_text: Option<&str>,
        emit_empty: bool,
    ) -> Option<ProviderTextDeltaUpdate> {
        let existing = self
            .buffered
            .iter()
            .position(|message| {
                message.update.turn_id == turn_id && message.update.item_id == item_id
            })
            .map(|index| self.buffered.remove(index));
        let text = final_text
            .map(str::to_owned)
            .unwrap_or_else(|| existing.map_or_else(String::new, |message| message.update.text));
        (emit_empty || !text.is_empty()).then(|| ProviderTextDeltaUpdate {
            turn_id: turn_id.into(),
            item_id: item_id.into(),
            text,
            completed: true,
        })
    }
    pub fn flush_turn(&mut self, turn_id: &str) -> Vec<ProviderTextDeltaUpdate> {
        let mut updates = Vec::new();
        self.buffered.retain(|message| {
            if message.update.turn_id != turn_id {
                return true;
            }
            let mut update = message.update.clone();
            update.completed = true;
            updates.push(update);
            false
        });
        updates
    }
}
