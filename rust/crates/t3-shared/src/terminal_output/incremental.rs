use super::*;

/// Mutable feed API: appending does not clone, render or scan retained scrollback.
/// Materialize a snapshot only when a consumer needs to display or serialize it.
pub struct TerminalOutput {
    terminal: Terminal,
    pending: String,
    truncated: bool,
    utf8: Vec<u8>,
    /// Bumped when settled text is trimmed or cleared, so readers start over.
    epoch: u64,
}

/// A reader's place in the settled text, for [`TerminalOutput::settled_since`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SettledCursor {
    epoch: u64,
    offset: usize,
}

/// Settled text a reader has not seen yet, as whole lines each ending in `\n`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettledChange {
    /// Lines settled since the cursor; empty when nothing settled.
    Appended(String),
    /// Settled text was trimmed or cleared since the cursor: all of it now, from the start.
    Reset(String),
}
impl Default for TerminalOutput {
    fn default() -> Self {
        Self::new(TERMINAL_OUTPUT_TAIL_CHARS)
    }
}
impl TerminalOutput {
    pub fn new(max_chars: usize) -> Self {
        Self {
            terminal: Terminal {
                screen: TerminalOutputState::default().screen,
                dropped: false,
                max: max_chars,
                settled_units: 0,
            },
            pending: String::new(),
            truncated: false,
            utf8: vec![],
            epoch: 0,
        }
    }
    pub fn push(&mut self, chunk: &str) {
        self.push_units(&chunk.encode_utf16().collect::<Vec<_>>());
    }
    /// Also accepts JavaScript chunks split between the halves of a surrogate pair.
    pub fn push_units(&mut self, chunk: &[u16]) {
        let input: Vec<_> = self
            .pending
            .encode_utf16()
            .chain(chunk.iter().copied())
            .collect();
        if input.is_empty() {
            return;
        }
        let before = self.settled_lengths();
        self.pending = self.terminal.feed(&input);
        let max = self.terminal.max;
        bound_window(&mut self.terminal, &mut self.truncated, max);
        // Lines only ever append to the second part; anything else trimmed or cleared it.
        let after = self.settled_lengths();
        if after.0 != before.0 || after.1 < before.1 {
            self.epoch += 1;
        }
        self.truncated |= self.terminal.dropped;
        if !self.truncated {
            let window = self
                .terminal
                .screen
                .lines
                .iter()
                .map(|line| {
                    let mut current = "";
                    let mut units = 0;
                    for run in line {
                        if run.pen != current {
                            units += sgr(&run.pen).len();
                            current = &run.pen;
                        }
                        units += run.len;
                    }
                    units + if current.is_empty() { 0 } else { sgr("").len() }
                })
                .sum::<usize>()
                + self.terminal.screen.lines.len()
                - 1;
            self.truncated = self.terminal.settled_units + window > max;
        }
    }
    /// Streaming UTF-8 decoding retains at most three incomplete bytes between chunks.
    pub fn push_bytes(&mut self, chunk: &[u8]) {
        let mut input = std::mem::take(&mut self.utf8);
        input.extend_from_slice(chunk);
        let mut cursor = 0;
        while cursor < input.len() {
            match std::str::from_utf8(&input[cursor..]) {
                Ok(text) => {
                    self.push(text);
                    return;
                }
                Err(error) => {
                    let end = cursor + error.valid_up_to();
                    if let Ok(text) = std::str::from_utf8(&input[cursor..end]) {
                        self.push(text);
                    }
                    match error.error_len() {
                        Some(length) => {
                            self.push("\u{fffd}");
                            cursor = end + length;
                        }
                        None => {
                            self.utf8.extend_from_slice(&input[end..]);
                            return;
                        }
                    }
                }
            }
        }
    }
    pub fn finish_bytes(&mut self) {
        if !self.utf8.is_empty() {
            self.utf8.clear();
            self.push("\u{fffd}");
        }
    }
    fn settled_lengths(&self) -> (usize, usize) {
        let [first, second] = &self.terminal.screen.settled;
        (first.len(), second.len())
    }
    /// Settled lines since `cursor`, advancing it: work proportional to what settled since,
    /// except after a trim or clear, which hands back all settled text once.
    pub fn settled_since(&self, cursor: &mut SettledCursor) -> SettledChange {
        let [first, second] = &self.terminal.screen.settled;
        if cursor.epoch != self.epoch || cursor.offset > second.len() {
            *cursor = SettledCursor {
                epoch: self.epoch,
                offset: second.len(),
            };
            let mut all = first.clone();
            all.extend_from_slice(second);
            return SettledChange::Reset(String::from_utf16_lossy(&all));
        }
        let appended = String::from_utf16_lossy(&second[cursor.offset..]);
        cursor.offset = second.len();
        SettledChange::Appended(appended)
    }
    /// The live window (at most 64 lines the cursor can still change), one string per line.
    pub fn window_lines(&self) -> Vec<String> {
        self.terminal
            .screen
            .lines
            .iter()
            .map(|line| String::from_utf16_lossy(&serialize_line(line)))
            .collect()
    }
    /// Whether output was dropped to stay within the cap.
    pub fn truncated(&self) -> bool {
        self.truncated || self.terminal.dropped
    }
    pub fn snapshot(&self) -> TerminalOutputState {
        let (settled, dropped) = tail_parts(&self.terminal.screen.settled, self.terminal.max);
        let window = self
            .terminal
            .screen
            .lines
            .iter()
            .map(serialize_line)
            .collect::<Vec<_>>()
            .join(&10);
        let mut parts = settled;
        parts.push(window);
        let (tail, cut) = tail_parts(&parts, self.terminal.max);
        TerminalOutputState {
            text: String::from_utf16_lossy(&tail.concat()),
            truncated: self.truncated || dropped || cut,
            pending: self.pending.clone(),
            screen: self.terminal.screen.clone(),
        }
    }
}

#[cfg(test)]
mod reader_tests {
    use super::*;

    fn read(output: &TerminalOutput, cursor: &mut SettledCursor, settled: &mut String) -> String {
        match output.settled_since(cursor) {
            SettledChange::Appended(text) => settled.push_str(&text),
            SettledChange::Reset(text) => *settled = text,
        }
        format!("{settled}{}", output.window_lines().join("\n"))
    }

    #[test]
    fn settled_lines_and_the_window_rebuild_the_snapshot_and_appends_hand_back_only_new_lines() {
        let mut output = TerminalOutput::new(TERMINAL_OUTPUT_TAIL_CHARS);
        let (mut cursor, mut settled) = (SettledCursor::default(), String::new());
        for line in 0..200 {
            output.push(&format!("line {line} \u{1b}[32mok\u{1b}[0m\n"));
            assert_eq!(
                read(&output, &mut cursor, &mut settled),
                output.snapshot().text
            );
        }
        // A redraw changes only the window.
        output.push("50%\r100%");
        assert_eq!(
            output.settled_since(&mut cursor),
            SettledChange::Appended(String::new())
        );
        assert_eq!(
            read(&output, &mut cursor, &mut settled),
            output.snapshot().text
        );
        // One more line settles exactly one line.
        let before = cursor;
        output.push("\n");
        let mut probe = before;
        match output.settled_since(&mut probe) {
            SettledChange::Appended(text) => assert_eq!(text.matches('\n').count(), 1, "{text:?}"),
            reset => panic!("{reset:?}"),
        }
        assert!(output.window_lines().len() <= 64);
    }

    #[test]
    fn a_trim_or_clear_resets_readers() {
        let mut output = TerminalOutput::new(400);
        let (mut cursor, mut settled) = (SettledCursor::default(), String::new());
        let mut resets = 0;
        for line in 0..400 {
            output.push(&format!("line {line}\n"));
            let mut probe = cursor;
            if matches!(output.settled_since(&mut probe), SettledChange::Reset(_)) {
                resets += 1;
            }
            read(&output, &mut cursor, &mut settled);
        }
        assert!(resets > 0, "the 400-unit cap trims settled text");
        assert!(output.truncated());
        output.push("\u{1b}[3J");
        assert!(
            matches!(output.settled_since(&mut cursor), SettledChange::Reset(text) if text.is_empty())
        );
    }
}
