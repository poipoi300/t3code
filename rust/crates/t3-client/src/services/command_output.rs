//! One command row's output while it is open. Ports `packages/client-runtime/src/state/
//! commandOutput.ts` and the `commandOutput` subscription rules of `state/orchestration.ts`:
//! subscribed only while a row holds a lease, ended once the command settles, and a settled
//! command's output kept for five minutes so reopening the row does not refetch.
//!
//! The display is stable bounded blocks, not one string: settled lines sit in blocks of at most
//! [`BLOCK_LINES`] lines that never change once full, and the live window (the lines a cursor can
//! still rewrite) is one `Rc<str>` per line. A reader asks for what changed since the revision it
//! last saw, so reading after an append costs what the append changed, never the whole output.
use crate::ClientError;
use futures::{Stream, StreamExt, channel::mpsc, stream::LocalBoxStream};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};
use t3_contracts::orchestration_v2::{CommandOutputFrameKind, OrchestrationV2CommandOutputFrame};
use t3_shared::terminal_output::{
    SettledChange, SettledCursor, TERMINAL_OUTPUT_TAIL_CHARS, TerminalOutput,
};

/// How long a settled command's output stays cached after its last reader closes.
pub const COMPLETED_OUTPUT_TTL_MS: u64 = 5 * 60_000;
/// The most lines one settled block holds (the timeline's long-output block size).
pub const BLOCK_LINES: usize = 40;

/// Up to [`BLOCK_LINES`] settled lines, each ending in `\n`, with SGR escapes as the fold wrote
/// them. A block that grows is replaced by a new `Rc`; a full one is never touched again.
#[derive(Debug, PartialEq, Eq)]
pub struct OutputBlock {
    /// Position in the output, increasing; head blocks trimmed by the cap drop off the front.
    pub seq: u64,
    /// The revision that last changed this block.
    pub revision: u64,
    pub text: Rc<str>,
    pub lines: usize,
}

/// What changed since a reader's revision ([`CommandOutputCell::read`]).
#[derive(Debug, Clone)]
pub struct OutputDelta {
    pub revision: u64,
    /// The reader's blocks are stale (first read, a replace frame, or a trimmed fold): take
    /// `blocks` as the whole settled output.
    pub reset: bool,
    /// Blocks before this sequence number are gone from the head; drop them.
    pub first_seq: u64,
    /// Blocks changed or added since the reader's revision, in order: replace the reader's
    /// blocks from `blocks[0].seq` on with these. Empty when no settled line changed.
    pub blocks: Vec<Rc<OutputBlock>>,
    /// The live window, one line each; a line that did not change keeps its `Rc`.
    pub live: Rc<[Rc<str>]>,
    pub live_changed: bool,
    pub truncated: bool,
    /// False once the command settled and the output is final.
    pub running: bool,
}

struct Display {
    fold: TerminalOutput,
    cursor: SettledCursor,
    blocks: VecDeque<Rc<OutputBlock>>,
    next_seq: u64,
    /// UTF-16 units held in `blocks`, to keep within the cap the snapshot text applies.
    settled_units: usize,
    live: Rc<[Rc<str>]>,
    live_revision: u64,
    reset_revision: u64,
    revision: u64,
    truncated_frames: bool,
    head_trimmed: bool,
    running: bool,
}

impl Display {
    fn new() -> Self {
        Self {
            fold: TerminalOutput::new(TERMINAL_OUTPUT_TAIL_CHARS),
            cursor: SettledCursor::default(),
            blocks: VecDeque::new(),
            next_seq: 0,
            settled_units: 0,
            live: Rc::from(Vec::new()),
            live_revision: 0,
            reset_revision: 0,
            revision: 0,
            truncated_frames: false,
            head_trimmed: false,
            running: true,
        }
    }

    fn clear_blocks(&mut self) {
        self.blocks.clear();
        self.settled_units = 0;
        self.reset_revision = self.revision;
    }

    /// Applies one server frame. A `replace` carries the whole bounded tail (the first frame,
    /// a viewer that fell behind, or the final output); an `append` is raw terminal text
    /// normalized with the same rules the server uses.
    fn apply(&mut self, frame: &OrchestrationV2CommandOutputFrame) {
        self.revision += 1;
        if frame.kind == CommandOutputFrameKind::Replace {
            self.fold = TerminalOutput::new(TERMINAL_OUTPUT_TAIL_CHARS);
            self.cursor = SettledCursor::default();
            self.truncated_frames = false;
            self.head_trimmed = false;
            self.clear_blocks();
        }
        self.truncated_frames |= frame.truncated;
        self.running = frame.running;
        self.fold.push(&frame.text);
        match self.fold.settled_since(&mut self.cursor) {
            SettledChange::Appended(text) => self.settle(&text),
            SettledChange::Reset(text) => {
                self.clear_blocks();
                self.settle(&text);
            }
        }
        self.update_live();
        self.keep_within_cap();
    }

    fn settle(&mut self, text: &str) {
        let mut lines = text.split_inclusive('\n').peekable();
        if lines.peek().is_none() {
            return;
        }
        self.settled_units += text.encode_utf16().count();
        // The last block takes lines until it is full; it is replaced, never mutated.
        if let Some(last) = self.blocks.back().filter(|block| block.lines < BLOCK_LINES) {
            let mut grown = String::from(&*last.text);
            let mut count = last.lines;
            while count < BLOCK_LINES
                && let Some(line) = lines.next()
            {
                grown.push_str(line);
                count += 1;
            }
            let seq = last.seq;
            if let Some(back) = self.blocks.back_mut() {
                *back = Rc::new(OutputBlock {
                    seq,
                    revision: self.revision,
                    text: Rc::from(grown),
                    lines: count,
                });
            }
        }
        while lines.peek().is_some() {
            let mut text = String::new();
            let mut count = 0;
            while count < BLOCK_LINES
                && let Some(line) = lines.next()
            {
                text.push_str(line);
                count += 1;
            }
            self.blocks.push_back(Rc::new(OutputBlock {
                seq: self.next_seq,
                revision: self.revision,
                text: Rc::from(text),
                lines: count,
            }));
            self.next_seq += 1;
        }
    }

    fn update_live(&mut self) {
        let lines = self.fold.window_lines();
        let changed = lines.len() != self.live.len()
            || lines
                .iter()
                .zip(self.live.iter())
                .any(|(line, kept)| line.as_str() != &**kept);
        if !changed {
            return;
        }
        let live: Vec<Rc<str>> = lines
            .into_iter()
            .enumerate()
            .map(|(index, line)| match self.live.get(index) {
                Some(kept) if **kept == *line => kept.clone(),
                _ => Rc::from(line),
            })
            .collect();
        self.live = Rc::from(live);
        self.live_revision = self.revision;
    }

    /// The fold keeps up to twice the cap settled before it trims; drop whole head blocks so
    /// what is shown stays within the cap, as the snapshot text does.
    fn keep_within_cap(&mut self) {
        let live: usize = self
            .live
            .iter()
            .map(|line| line.encode_utf16().count() + 1)
            .sum();
        while self.blocks.len() > 1 && self.settled_units + live > TERMINAL_OUTPUT_TAIL_CHARS {
            if let Some(block) = self.blocks.pop_front() {
                self.settled_units = self
                    .settled_units
                    .saturating_sub(block.text.encode_utf16().count());
                self.head_trimmed = true;
            }
        }
    }

    fn read(&self, since: Option<u64>) -> OutputDelta {
        let reset = since.is_none_or(|since| since < self.reset_revision);
        let since = since.unwrap_or(0);
        let blocks = if reset {
            self.blocks.iter().cloned().collect()
        } else {
            let changed = self
                .blocks
                .iter()
                .rev()
                .take_while(|block| block.revision > since)
                .count();
            self.blocks
                .iter()
                .skip(self.blocks.len() - changed)
                .cloned()
                .collect()
        };
        OutputDelta {
            revision: self.revision,
            reset,
            first_seq: self.blocks.front().map_or(self.next_seq, |block| block.seq),
            blocks,
            live: self.live.clone(),
            live_changed: reset || self.live_revision > since,
            truncated: self.truncated_frames || self.head_trimmed || self.fold.truncated(),
            running: self.running,
        }
    }

    /// The whole output as one string, for copying; proportional to the output.
    fn text(&self) -> String {
        let mut text: String = self.blocks.iter().map(|block| &*block.text).collect();
        text.push_str(
            &self
                .live
                .iter()
                .map(|line| &**line)
                .collect::<Vec<_>>()
                .join("\n"),
        );
        text
    }
}

/// What every lease of one command row reads.
#[derive(Default)]
pub struct CommandOutputCell {
    display: RefCell<Option<Display>>,
    error: RefCell<Option<String>>,
    listeners: RefCell<Vec<mpsc::UnboundedSender<()>>>,
}

impl CommandOutputCell {
    /// What changed since `since` (the `revision` of the reader's last delta; `None` on its
    /// first read). `None` while loading (no frame yet).
    pub fn read(&self, since: Option<u64>) -> Option<OutputDelta> {
        self.display
            .borrow()
            .as_ref()
            .map(|display| display.read(since))
    }
    /// False once the command settled; `None` while loading.
    pub fn running(&self) -> Option<bool> {
        self.display
            .borrow()
            .as_ref()
            .map(|display| display.running)
    }
    /// The whole output as one string (copying it): proportional to the output.
    pub fn text(&self) -> Option<String> {
        self.display.borrow().as_ref().map(Display::text)
    }
    /// Why the output could not be loaded, until a later frame arrives.
    pub fn error(&self) -> Option<String> {
        self.error.borrow().clone()
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        let (sender, receiver) = mpsc::unbounded();
        self.listeners.borrow_mut().push(sender);
        receiver.boxed_local()
    }
    fn notify(&self) {
        self.listeners
            .borrow_mut()
            .retain(|listener| listener.unbounded_send(()).is_ok());
    }
    /// Applies one frame; returns whether the command has settled.
    pub fn apply(&self, frame: &OrchestrationV2CommandOutputFrame) -> bool {
        let settled = {
            let mut display = self.display.borrow_mut();
            let display = display.get_or_insert_with(Display::new);
            display.apply(frame);
            !display.running
        };
        self.error.borrow_mut().take();
        self.notify();
        settled
    }
}

/// Feeds `frames` (each session's subscription, resubscribed after a reconnect, or another
/// source such as a shared thread) into `cell` until the command settles.
pub async fn drive_command_output(
    cell: Rc<CommandOutputCell>,
    mut frames: impl Stream<Item = Result<OrchestrationV2CommandOutputFrame, ClientError>> + Unpin,
) {
    while let Some(frame) = frames.next().await {
        match frame {
            Ok(frame) => {
                if cell.apply(&frame) {
                    return;
                }
            }
            Err(error) => {
                *cell.error.borrow_mut() = Some(error.to_string());
                cell.notify();
            }
        }
    }
}

/// Settled commands' final output by identity, each with the instant it may be dropped.
pub(crate) type SettledCells =
    Rc<RefCell<std::collections::BTreeMap<String, (Rc<CommandOutputCell>, u64)>>>;

/// Keeps a settled row's cell for [`COMPLETED_OUTPUT_TTL_MS`] after its last reader leaves.
pub(crate) struct SettledCache {
    pub identity: String,
    pub cells: SettledCells,
    pub now: Rc<dyn Fn() -> u64>,
}

pub(crate) struct CommandOutputShared {
    pub cell: Rc<CommandOutputCell>,
    pub driver: Option<futures::future::AbortHandle>,
    pub cache: Option<SettledCache>,
}

impl Drop for CommandOutputShared {
    fn drop(&mut self) {
        if let Some(driver) = &self.driver {
            driver.abort();
        }
        if let Some(cache) = self.cache.take()
            && self.cell.running() == Some(false)
        {
            cache.cells.borrow_mut().insert(
                cache.identity,
                (
                    self.cell.clone(),
                    (cache.now)().saturating_add(COMPLETED_OUTPUT_TTL_MS),
                ),
            );
        }
    }
}

/// One open command row's output; frames flow while a lease is held.
#[must_use = "the output streams only while its lease is held"]
#[derive(Clone)]
pub struct CommandOutputLease {
    pub(crate) shared: Rc<CommandOutputShared>,
}

impl CommandOutputLease {
    /// A row whose frames come from another source than the environment's subscription (the
    /// web's `TurnItemSource.commandOutput`, such as a shared thread), with the same display
    /// and settle rules. `spawn` runs the feed until the lease is dropped.
    pub fn from_frames(
        frames: impl Stream<Item = Result<OrchestrationV2CommandOutputFrame, ClientError>>
        + Unpin
        + 'static,
        spawn: &crate::composition::Spawner,
    ) -> Self {
        use futures::FutureExt;
        let cell = Rc::new(CommandOutputCell::default());
        let (driver, registration) = futures::future::AbortHandle::new_pair();
        let driven = cell.clone();
        spawn(
            futures::future::Abortable::new(drive_command_output(driven, frames), registration)
                .map(|_| ())
                .boxed_local(),
        );
        Self {
            shared: Rc::new(CommandOutputShared {
                cell,
                driver: Some(driver),
                cache: None,
            }),
        }
    }
    /// What changed since the reader's last `revision` (`None` first); `None` while loading.
    pub fn read(&self, since: Option<u64>) -> Option<OutputDelta> {
        self.shared.cell.read(since)
    }
    /// The whole output as one string, for copying: proportional to the output.
    pub fn text(&self) -> Option<String> {
        self.shared.cell.text()
    }
    /// Why the output could not be loaded ("Couldn't load output: …" in the web).
    pub fn error(&self) -> Option<String> {
        self.shared.cell.error()
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        self.shared.cell.changes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(
        kind: CommandOutputFrameKind,
        text: &str,
        running: bool,
    ) -> OrchestrationV2CommandOutputFrame {
        OrchestrationV2CommandOutputFrame {
            kind,
            text: text.into(),
            truncated: false,
            running,
        }
    }

    #[test]
    fn continues_an_escape_and_a_redraw_split_across_appends() {
        // commandOutput.test.ts
        let cell = CommandOutputCell::default();
        cell.apply(&frame(
            CommandOutputFrameKind::Replace,
            "install\n12%\u{1b}[3",
            true,
        ));
        assert_eq!(cell.text().unwrap(), "install\n12%");
        cell.apply(&frame(
            CommandOutputFrameKind::Append,
            "2m\r100%\u{1b}[0m\n",
            true,
        ));
        // The colour opened before the redraw survives it.
        assert_eq!(cell.text().unwrap(), "install\n\u{1b}[0;32m100%\u{1b}[0m\n");
        assert_eq!(cell.running(), Some(true));
    }

    #[test]
    fn a_replace_starts_over_and_truncation_sticks_until_then() {
        let cell = CommandOutputCell::default();
        let mut truncated = frame(CommandOutputFrameKind::Replace, "old", true);
        truncated.truncated = true;
        cell.apply(&truncated);
        cell.apply(&frame(CommandOutputFrameKind::Append, " more", true));
        let delta = cell.read(None).unwrap();
        assert!(delta.truncated);
        assert_eq!(cell.text().unwrap(), "old more");
        let before = delta.revision;
        cell.apply(&frame(CommandOutputFrameKind::Replace, "final\n", false));
        let delta = cell.read(Some(before)).unwrap();
        assert!(delta.reset, "a replace is signalled as a reset");
        assert_eq!(cell.text().unwrap(), "final\n");
        assert!(!delta.truncated);
        assert!(!delta.running);
    }

    #[test]
    fn reading_after_an_append_costs_what_the_append_changed() {
        // Counted work: 5,000 lines, then one more line and a redraw of the live line.
        let cell = CommandOutputCell::default();
        let long: String = (0..5_000).map(|line| format!("line {line}\n")).collect();
        cell.apply(&frame(CommandOutputFrameKind::Replace, &long, true));
        let first = cell.read(None).unwrap();
        assert!(first.reset);
        assert!(first.blocks.iter().all(|block| block.lines <= BLOCK_LINES));
        let total: usize = first.blocks.iter().map(|block| block.lines).sum();
        assert_eq!(
            total + first.live.len(),
            cell.text().unwrap().lines().count() + 1
        );

        cell.apply(&frame(CommandOutputFrameKind::Append, "next\n50%", true));
        let delta = cell.read(Some(first.revision)).unwrap();
        assert!(!delta.reset);
        assert_eq!(
            delta.blocks.len(),
            1,
            "only the block the new line landed in"
        );
        assert!(delta.blocks[0].lines <= BLOCK_LINES);
        // The window keeps the newest 64 lines; the line it pushed out settles.
        assert!(
            delta.blocks[0].text.ends_with("line 4937\n"),
            "{:?}",
            delta.blocks[0].text
        );
        assert!(delta.live.iter().any(|line| &**line == "next"));
        assert!(delta.live_changed);
        assert_eq!(&*delta.live[delta.live.len() - 1], "50%");

        // A redraw changes only the live line; settled blocks are not handed back at all.
        cell.apply(&frame(CommandOutputFrameKind::Append, "\r90%", true));
        let redraw = cell.read(Some(delta.revision)).unwrap();
        assert!(redraw.blocks.is_empty());
        assert!(redraw.live_changed);
        assert_eq!(&*redraw.live[redraw.live.len() - 1], "90%");

        // Unchanged blocks are the same `Rc`s across reads.
        let again = cell.read(None).unwrap();
        let shared = first
            .blocks
            .iter()
            .zip(again.blocks.iter())
            .take(first.blocks.len() - 1)
            .all(|(a, b)| Rc::ptr_eq(a, b));
        assert!(shared);
    }

    #[test]
    fn output_beyond_the_cap_drops_whole_head_blocks_and_says_so() {
        let cell = CommandOutputCell::default();
        let line = "x".repeat(99) + "\n";
        let lines = TERMINAL_OUTPUT_TAIL_CHARS / line.len() + 400;
        for _ in 0..lines / 100 {
            cell.apply(&frame(
                CommandOutputFrameKind::Append,
                &line.repeat(100),
                true,
            ));
        }
        let delta = cell.read(None).unwrap();
        assert!(delta.truncated);
        assert!(delta.first_seq > 0);
        let shown: usize = delta.blocks.iter().map(|block| block.text.len()).sum();
        assert!(shown <= TERMINAL_OUTPUT_TAIL_CHARS, "{shown}");
    }
}
