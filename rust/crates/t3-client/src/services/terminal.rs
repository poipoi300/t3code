use crate::types::{EnvironmentId, Generation, ThreadKey};
use std::{
    collections::{BTreeMap, VecDeque},
    rc::Rc,
};
pub use t3_contracts::terminal::DEFAULT_MAX_TERMINAL_BUFFER_BYTES;
use t3_contracts::terminal::{
    TerminalAttachInput, TerminalAttachStreamEvent, TerminalMetadataStreamEvent,
    TerminalSessionSnapshot, TerminalSessionStatus, TerminalSummary,
};

const CHUNK_BYTES: usize = 16 * 1024;
const MAX_CHUNKS: usize = 1024;
fn units(text: &str) -> u64 {
    text.encode_utf16().count() as u64
}
fn suffix_at(text: &str, offset: u64) -> &str {
    let mut units = 0;
    for (index, character) in text.char_indices() {
        if units >= offset {
            return &text[index..];
        }
        units += character.len_utf16() as u64;
    }
    ""
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalOutputChunk {
    pub start_offset: u64,
    pub data: Rc<str>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalOutputState {
    pub generation: u64,
    pub chunks: VecDeque<TerminalOutputChunk>,
    pub retained_bytes: usize,
    pub reset_version: u64,
    pub next_offset: u64,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalOutputCursor {
    pub generation: Option<u64>,
    pub reset_version: Option<u64>,
    pub offset: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalOutputUpdate {
    None(TerminalOutputCursor),
    Reset {
        data: String,
        cursor: TerminalOutputCursor,
    },
    Append {
        data: String,
        cursor: TerminalOutputCursor,
    },
}
impl TerminalOutputUpdate {
    pub fn cursor(&self) -> TerminalOutputCursor {
        match self {
            Self::None(cursor) | Self::Reset { cursor, .. } | Self::Append { cursor, .. } => {
                *cursor
            }
        }
    }
}
impl TerminalOutputState {
    pub fn text(&self) -> String {
        self.chunks
            .iter()
            .map(|chunk| chunk.data.as_ref())
            .collect()
    }
    fn push(&mut self, data: &str, budget: usize) {
        let mut offset = 0;
        let chunk_bytes = CHUNK_BYTES.min(budget.max(1));
        while offset < data.len() {
            let mut end = (offset + chunk_bytes).min(data.len());
            while !data.is_char_boundary(end) {
                end -= 1;
            }
            if end == offset {
                end = (offset + chunk_bytes).min(data.len());
                while !data.is_char_boundary(end) {
                    end += 1;
                }
            }
            let piece = &data[offset..end];
            self.chunks.push_back(TerminalOutputChunk {
                start_offset: self.next_offset,
                data: Rc::from(piece),
            });
            self.next_offset += units(piece);
            self.retained_bytes += piece.len();
            offset = end;
        }
    }
    pub fn append(&mut self, data: &str, budget: usize) {
        if data.is_empty() {
            return;
        }
        if budget == 0 {
            self.chunks.clear();
            self.retained_bytes = 0;
            self.reset_version += 1;
            self.next_offset += units(data);
            return;
        }
        self.push(data, budget);
        while self.retained_bytes > budget {
            let Some(mut first) = self.chunks.pop_front() else {
                break;
            };
            let excess = self.retained_bytes - budget;
            if excess >= first.data.len() {
                self.retained_bytes -= first.data.len();
                continue;
            }
            let mut cut = excess;
            while !first.data.is_char_boundary(cut) {
                cut += 1;
            }
            first.start_offset += units(&first.data[..cut]);
            self.retained_bytes -= cut;
            first.data = Rc::from(&first.data[cut..]);
            if !first.data.is_empty() {
                self.chunks.push_front(first);
            }
        }
        if self.chunks.len() > MAX_CHUNKS {
            let mut compacted: VecDeque<TerminalOutputChunk> = VecDeque::new();
            for chunk in self.chunks.drain(..) {
                if let Some(previous) = compacted.back_mut()
                    && previous.start_offset + units(&previous.data) == chunk.start_offset
                    && previous.data.len() + chunk.data.len() <= CHUNK_BYTES
                {
                    previous.data = Rc::from(format!("{}{}", previous.data, chunk.data));
                    continue;
                }
                compacted.push_back(chunk);
            }
            while compacted.len() > MAX_CHUNKS {
                if let Some(first) = compacted.pop_front() {
                    self.retained_bytes -= first.data.len();
                }
            }
            self.chunks = compacted;
        }
    }
    pub fn reset(&mut self, data: &str, budget: usize) {
        self.chunks.clear();
        self.next_offset = 0;
        self.retained_bytes = 0;
        self.reset_version += 1;
        let mut cut = data.len().saturating_sub(budget);
        while !data.is_char_boundary(cut) {
            cut += 1;
        }
        self.push(&data[cut..], budget);
    }
    pub fn read(&self, cursor: TerminalOutputCursor) -> TerminalOutputUpdate {
        let next = TerminalOutputCursor {
            generation: Some(self.generation),
            reset_version: Some(self.reset_version),
            offset: self.next_offset,
        };
        if cursor.generation != next.generation
            || cursor.reset_version != next.reset_version
            || cursor.offset
                < self
                    .chunks
                    .front()
                    .map_or(self.next_offset, |chunk| chunk.start_offset)
        {
            return TerminalOutputUpdate::Reset {
                data: self.text(),
                cursor: next,
            };
        }
        let mut start = self.chunks.len();
        while start > 0 {
            let chunk = &self.chunks[start - 1];
            if chunk.start_offset + units(&chunk.data) <= cursor.offset {
                break;
            }
            start -= 1;
        }
        if start == self.chunks.len() {
            return TerminalOutputUpdate::None(next);
        }
        TerminalOutputUpdate::Append {
            data: self
                .chunks
                .iter()
                .skip(start)
                .map(|chunk| {
                    suffix_at(
                        &chunk.data,
                        cursor.offset.saturating_sub(chunk.start_offset),
                    )
                })
                .collect(),
            cursor: next,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TerminalStatus {
    #[default]
    Closed,
    Starting,
    Running,
    Exited,
    Error,
}
impl From<TerminalSessionStatus> for TerminalStatus {
    fn from(status: TerminalSessionStatus) -> Self {
        match status {
            TerminalSessionStatus::Starting => Self::Starting,
            TerminalSessionStatus::Running => Self::Running,
            TerminalSessionStatus::Exited => Self::Exited,
            TerminalSessionStatus::Error => Self::Error,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalGrid {
    pub cols: u64,
    pub rows: u64,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalBufferState {
    pub output: TerminalOutputState,
    pub grid: Option<TerminalGrid>,
    pub status: TerminalStatus,
    pub error: Option<String>,
    pub updated_at: Option<String>,
    pub version: u64,
    pub lifecycle_version: u64,
}
#[derive(Debug, Clone, Default)]
pub struct AttachGenerations(u64);
impl AttachGenerations {
    fn next(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }
    pub fn seed(&mut self) -> TerminalBufferState {
        let mut buffer = TerminalBufferState::default();
        buffer.output.generation = self.next();
        buffer
    }
}
impl TerminalBufferState {
    fn grid(&mut self, cols: Option<Option<u64>>, rows: Option<Option<u64>>) {
        if let (Some(cols), Some(rows)) = (cols.flatten(), rows.flatten()) {
            self.grid = Some(TerminalGrid { cols, rows });
        }
    }
    fn snapshot(&mut self, snapshot: &TerminalSessionSnapshot, budget: usize) {
        self.output.reset(&snapshot.history, budget);
        self.grid(snapshot.cols, snapshot.rows);
        self.status = snapshot.status.into();
        self.error = None;
        self.updated_at = Some(snapshot.updated_at.clone());
        self.version += 1;
    }
    pub fn apply(
        &mut self,
        event: &TerminalAttachStreamEvent,
        budget: usize,
        generations: &mut AttachGenerations,
    ) {
        match event {
            TerminalAttachStreamEvent::TerminalAttachSnapshotEvent(event) => {
                if self.version > 0 {
                    self.output.generation = generations.next();
                    self.lifecycle_version += 1;
                }
                self.snapshot(&event.snapshot, budget);
            }
            TerminalAttachStreamEvent::TerminalRestartedEvent(event) => {
                self.snapshot(&event.snapshot, budget);
                self.lifecycle_version += 1;
            }
            TerminalAttachStreamEvent::TerminalActivityEvent(event) => {
                self.grid(event.cols, event.rows);
            }
            TerminalAttachStreamEvent::TerminalOutputEvent(event) => {
                self.output.append(&event.data, budget);
                self.error = None;
                self.version += 1;
                if self.status == TerminalStatus::Closed {
                    self.status = TerminalStatus::Running;
                }
            }
            TerminalAttachStreamEvent::TerminalClearedEvent(_) => {
                self.output.reset("", budget);
                self.error = None;
                self.version += 1;
            }
            TerminalAttachStreamEvent::TerminalExitedEvent(_) => {
                self.status = TerminalStatus::Exited;
                self.error = None;
                self.version += 1;
            }
            TerminalAttachStreamEvent::TerminalClosedEvent(_) => {
                self.status = TerminalStatus::Closed;
                self.error = None;
                self.version += 1;
            }
            TerminalAttachStreamEvent::TerminalErrorEvent(event) => {
                self.status = TerminalStatus::Error;
                self.error = Some(event.message.clone());
                self.version += 1;
            }
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSessionState {
    pub summary: Option<TerminalSummary>,
    pub buffer: TerminalBufferState,
    pub status: TerminalStatus,
    pub has_running_subprocess: bool,
    pub updated_at: Option<String>,
}
pub fn combine_terminal_session(
    summary: Option<TerminalSummary>,
    buffer: TerminalBufferState,
) -> TerminalSessionState {
    let updated_at = match (
        summary.as_ref().map(|summary| summary.updated_at.clone()),
        buffer.updated_at.clone(),
    ) {
        (Some(left), Some(right)) => Some(
            if crate::time::epoch_ms(&left)
                .zip(crate::time::epoch_ms(&right))
                .is_some_and(|(left, right)| left >= right)
            {
                left
            } else {
                right
            },
        ),
        (left, right) => left.or(right),
    };
    TerminalSessionState {
        status: if buffer.version > 0 {
            buffer.status
        } else {
            summary
                .as_ref()
                .map_or(buffer.status, |summary| summary.status.into())
        },
        has_running_subprocess: summary
            .as_ref()
            .is_some_and(|summary| summary.has_running_subprocess),
        summary,
        buffer,
        updated_at,
    }
}
pub fn apply_terminal_metadata(
    current: &mut Vec<TerminalSummary>,
    event: TerminalMetadataStreamEvent,
) {
    match event {
        TerminalMetadataStreamEvent::TerminalMetadataSnapshotEvent(event) => {
            *current = event.terminals
        }
        TerminalMetadataStreamEvent::TerminalMetadataRemoveEvent(event) => {
            current.retain(|entry| {
                entry.thread_id != event.thread_id || entry.terminal_id != event.terminal_id
            })
        }
        TerminalMetadataStreamEvent::TerminalMetadataUpsertEvent(event) => {
            current.retain(|entry| {
                entry.thread_id != event.terminal.thread_id
                    || entry.terminal_id != event.terminal.terminal_id
            });
            current.push(event.terminal);
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TerminalKey {
    pub thread: ThreadKey,
    pub terminal: String,
}
#[derive(Default)]
pub struct TerminalResources {
    pub buffers: BTreeMap<TerminalKey, TerminalBufferState>,
    pub metadata: BTreeMap<EnvironmentId, Vec<TerminalSummary>>,
    sessions: BTreeMap<EnvironmentId, Generation>,
    generations: AttachGenerations,
}
impl TerminalResources {
    pub fn begin_session(&mut self, environment: EnvironmentId, generation: Generation) {
        self.sessions.insert(environment, generation);
    }
    pub fn disconnect(&mut self, environment: &EnvironmentId) {
        self.sessions.remove(environment);
    }
    pub fn seed(&mut self, key: TerminalKey) {
        self.buffers.insert(key, self.generations.seed());
    }
    pub fn apply(
        &mut self,
        key: TerminalKey,
        generation: Generation,
        event: &TerminalAttachStreamEvent,
        budget: usize,
    ) -> bool {
        if self.sessions.get(&key.thread.environment) != Some(&generation) {
            return false;
        }
        let buffer = self
            .buffers
            .entry(key)
            .or_insert_with(|| self.generations.seed());
        buffer.apply(event, budget, &mut self.generations);
        true
    }
    pub fn remove_environment(&mut self, environment: &EnvironmentId) {
        self.buffers
            .retain(|key, _| &key.thread.environment != environment);
        self.metadata.remove(environment);
        self.sessions.remove(environment);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalLifecycle {
    pub was_running: bool,
    pub observed: bool,
    pub reopened: bool,
}
#[derive(Debug, Clone, Copy)]
pub enum TerminalLifecycleEvent {
    Observed,
    Detached {
        can_operate: bool,
    },
    Status {
        status: TerminalStatus,
        version: u64,
    },
    ReopenFailed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalLifecycleCommand {
    Exit,
    Reopen,
}
impl TerminalLifecycle {
    pub fn transition(
        &mut self,
        event: TerminalLifecycleEvent,
    ) -> Option<TerminalLifecycleCommand> {
        match event {
            TerminalLifecycleEvent::Observed => self.observed = true,
            TerminalLifecycleEvent::Detached { can_operate } => {
                self.observed |= !can_operate && self.was_running;
                self.was_running = false;
            }
            TerminalLifecycleEvent::ReopenFailed => self.reopened = false,
            TerminalLifecycleEvent::Status { version: 0, .. } => {}
            TerminalLifecycleEvent::Status {
                status: TerminalStatus::Running | TerminalStatus::Starting,
                ..
            } => {
                self.was_running = true;
                self.reopened = false;
            }
            TerminalLifecycleEvent::Status {
                status: TerminalStatus::Closed | TerminalStatus::Exited,
                ..
            } => {
                if self.was_running {
                    self.was_running = false;
                    self.reopened = true;
                    return Some(TerminalLifecycleCommand::Exit);
                }
                if !self.observed && !self.reopened {
                    self.reopened = true;
                    return Some(TerminalLifecycleCommand::Reopen);
                }
            }
            _ => {}
        }
        None
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalSessionAction {
    Resize(TerminalGrid),
    Write(String),
    Reopen,
    CloseAndReportExit,
    LiveChanged(bool),
}
pub struct TerminalSession {
    pub lifecycle: TerminalLifecycle,
    pub grid: TerminalGrid,
    generation: Option<u64>,
    live_lifecycle: Option<u64>,
    initial_input: Option<String>,
}
impl TerminalSession {
    pub fn new(grid: TerminalGrid, initial_input: Option<String>) -> Self {
        Self {
            lifecycle: TerminalLifecycle::default(),
            grid,
            generation: None,
            live_lifecycle: None,
            initial_input: initial_input.filter(|input| !input.is_empty()),
        }
    }
    pub fn observe(&mut self) {
        self.lifecycle.transition(TerminalLifecycleEvent::Observed);
    }
    pub fn detach(&mut self, can_operate: bool) -> Vec<TerminalSessionAction> {
        self.generation = None;
        self.lifecycle
            .transition(TerminalLifecycleEvent::Detached { can_operate });
        if self.live_lifecycle.take().is_some() {
            vec![TerminalSessionAction::LiveChanged(false)]
        } else {
            Vec::new()
        }
    }
    pub fn report_grid(
        &mut self,
        grid: TerminalGrid,
        can_operate: bool,
    ) -> Option<TerminalSessionAction> {
        if self.grid == grid {
            return None;
        }
        self.grid = grid;
        (can_operate && self.generation.is_some()).then_some(TerminalSessionAction::Resize(grid))
    }
    pub fn attachment(
        &mut self,
        buffer: &TerminalBufferState,
        failed: bool,
        can_operate: bool,
        can_reopen: bool,
    ) -> Vec<TerminalSessionAction> {
        let status = if failed {
            TerminalStatus::Error
        } else {
            buffer.status
        };
        let mut actions = Vec::new();
        let live = matches!(status, TerminalStatus::Running | TerminalStatus::Starting)
            .then_some(buffer.lifecycle_version);
        if live != self.live_lifecycle {
            self.live_lifecycle = live;
            actions.push(TerminalSessionAction::LiveChanged(live.is_some()));
        }
        let generation = (buffer.version > 0 && status == TerminalStatus::Running)
            .then_some(buffer.output.generation);
        if generation != self.generation {
            self.generation = generation;
            if generation.is_some() && can_operate {
                actions.push(TerminalSessionAction::Resize(self.grid));
            }
        }
        if buffer.version > 0
            && can_operate
            && let Some(input) = self.initial_input.take()
        {
            actions.push(TerminalSessionAction::Write(input));
        }
        match self.lifecycle.transition(TerminalLifecycleEvent::Status {
            status,
            version: buffer.version,
        }) {
            Some(TerminalLifecycleCommand::Exit) if can_operate => {
                actions.push(TerminalSessionAction::CloseAndReportExit)
            }
            Some(TerminalLifecycleCommand::Reopen) if can_operate && can_reopen => {
                actions.push(TerminalSessionAction::Reopen)
            }
            Some(TerminalLifecycleCommand::Reopen) => {
                self.lifecycle
                    .transition(TerminalLifecycleEvent::ReopenFailed);
            }
            _ => {}
        }
        actions
    }
}
#[derive(Default)]
pub struct TerminalAttachResume {
    replied: bool,
}
impl TerminalAttachResume {
    pub fn input(&self, mut input: TerminalAttachInput) -> TerminalAttachInput {
        if self.replied {
            input.resume = Some(Some(true));
        }
        input.max_history_bytes = Some(Some(DEFAULT_MAX_TERMINAL_BUFFER_BYTES as u64));
        input
    }
    pub fn received(&mut self) {
        self.replied = true;
    }
}
