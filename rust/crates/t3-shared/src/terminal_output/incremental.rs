use super::*;

/// Mutable feed API: appending does not clone, render or scan retained scrollback.
/// Materialize a snapshot only when a consumer needs to display or serialize it.
pub struct TerminalOutput {
    terminal: Terminal,
    pending: String,
    truncated: bool,
    utf8: Vec<u8>,
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
        self.pending = self.terminal.feed(&input);
        let max = self.terminal.max;
        bound_window(&mut self.terminal, &mut self.truncated, max);
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
                        units += run.text.len();
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
