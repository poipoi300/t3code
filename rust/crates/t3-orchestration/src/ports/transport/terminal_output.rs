//! Native port of shared/terminalOutput.ts. Cursor columns and budgets are UTF-16 units.
use super::{TERMINAL_OUTPUT_TAIL_CHARS, TerminalOutputFold, TerminalText};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const ESC: u16 = 27;
const LIVE_LINES: usize = 64;
#[derive(Clone, Default, Serialize, Deserialize)]
struct Run {
    pen: String,
    #[serde(with = "utf16_text")]
    text: Vec<u16>,
}
mod utf16_text {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<S: Serializer>(
        units: &[u16],
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match String::from_utf16(units) {
            Ok(text) => serializer.serialize_str(&text),
            Err(_) => units.serialize(serializer),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Vec<u16>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Text {
            String(String),
            Units(Vec<u16>),
        }
        Ok(match Text::deserialize(deserializer)? {
            Text::String(text) => text.encode_utf16().collect(),
            Text::Units(units) => units,
        })
    }
}
type Line = Vec<Run>;
#[derive(Clone, Copy, Serialize, Deserialize)]
struct Cursor {
    row: usize,
    col: usize,
}
#[derive(Clone, Serialize, Deserialize)]
struct Screen {
    settled: [String; 2],
    lines: Vec<Line>,
    row: usize,
    col: usize,
    pen: String,
    saved: Option<Cursor>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct TerminalOutputState {
    pub text: String,
    pub truncated: bool,
    pub pending: String,
    screen: Screen,
}
impl Default for TerminalOutputState {
    fn default() -> Self {
        Self {
            text: String::new(),
            truncated: false,
            pending: String::new(),
            screen: Screen {
                settled: Default::default(),
                lines: vec![vec![]],
                row: 0,
                col: 0,
                pen: String::new(),
                saved: None,
            },
        }
    }
}
#[derive(Default)]
pub struct NativeTerminalOutputFold;
impl TerminalOutputFold for NativeTerminalOutputFold {
    fn empty(&self) -> Value {
        serde_json::json!({"text":"","truncated":false,"pending":"","screen":{"settled":["",""],"lines":[[]],"row":0,"col":0,"pen":"","saved":null}})
    }
    fn append(&self, state: &Value, chunk: &str) -> Result<Value> {
        let state: TerminalOutputState = serde_json::from_value(state.clone())?;
        validate(&state)?;
        Ok(serde_json::to_value(append_terminal_output(
            &state,
            chunk,
            TERMINAL_OUTPUT_TAIL_CHARS,
        ))?)
    }
    fn normalize(&self, text: &str) -> Result<TerminalText> {
        Ok(normalize_terminal_output(text, TERMINAL_OUTPUT_TAIL_CHARS))
    }
    fn resume(&self, state: &Value) -> Result<TerminalText> {
        let state: TerminalOutputState = serde_json::from_value(state.clone())?;
        validate(&state)?;
        Ok(TerminalText {
            text: terminal_output_resume_text(&state),
            truncated: state.truncated,
        })
    }
    fn settled(&self, state: &Value) -> Result<TerminalText> {
        let state: TerminalOutputState = serde_json::from_value(state.clone())?;
        validate(&state)?;
        Ok(TerminalText {
            text: state.text,
            truncated: state.truncated,
        })
    }
}
fn validate(state: &TerminalOutputState) -> Result<()> {
    if state.screen.lines.is_empty()
        || state.screen.lines.len() > LIVE_LINES
        || state.screen.row >= state.screen.lines.len()
        || state.screen.col > TERMINAL_OUTPUT_TAIL_CHARS
        || state
            .screen
            .saved
            .is_some_and(|saved| saved.col > TERMINAL_OUTPUT_TAIL_CHARS)
    {
        return Err(Error::Decode {
            entity: "terminal output state",
            detail: "Invalid cursor or live window".into(),
        });
    }
    Ok(())
}
fn units(text: &str) -> usize {
    text.encode_utf16().count()
}
fn sgr(pen: &str) -> String {
    if pen.is_empty() {
        "\x1b[0m".into()
    } else {
        format!("\x1b[0;{pen}m")
    }
}
fn line_len(line: &Line) -> usize {
    line.iter().map(|run| run.text.len()).sum()
}
fn slice_line(line: &Line, start: usize, end: usize) -> Line {
    let mut offset = 0;
    let mut out = vec![];
    for run in line {
        let next = offset + run.text.len();
        if next > start && offset < end {
            out.push(Run {
                pen: run.pen.clone(),
                text: run.text[start.saturating_sub(offset)..run.text.len().min(end - offset)]
                    .to_vec(),
            });
        }
        offset = next;
        if offset >= end {
            break;
        }
    }
    out
}
fn push_run(line: &mut Line, run: Run) {
    if run.text.is_empty() {
        return;
    }
    if let Some(last) = line.last_mut()
        && last.pen == run.pen
    {
        last.text.extend(run.text);
    } else {
        line.push(run);
    }
}
fn write_line(line: &Line, col: usize, text: &[u16], pen: &str) -> Line {
    let len = line_len(line);
    let mut out = slice_line(line, 0, col.min(len));
    if col > len {
        push_run(
            &mut out,
            Run {
                pen: String::new(),
                text: vec![32; col - len],
            },
        );
    }
    push_run(
        &mut out,
        Run {
            pen: pen.into(),
            text: text.to_vec(),
        },
    );
    for run in slice_line(line, col + text.len(), len) {
        push_run(&mut out, run);
    }
    out
}
fn erase_line(line: &Line, col: usize, mode: u64) -> Line {
    let len = line_len(line);
    match mode {
        0 => slice_line(line, 0, col),
        1 => {
            let mut out = vec![];
            push_run(
                &mut out,
                Run {
                    pen: String::new(),
                    text: vec![32; (col + 1).min(len)],
                },
            );
            for run in slice_line(line, col + 1, len) {
                push_run(&mut out, run);
            }
            out
        }
        2 => vec![],
        _ => line.clone(),
    }
}
fn serialize_line(line: &Line) -> String {
    let mut out = Vec::new();
    let mut current = "";
    for run in line {
        if run.pen != current {
            out.extend(sgr(&run.pen).encode_utf16());
            current = &run.pen;
        }
        out.extend(&run.text);
    }
    if !current.is_empty() {
        out.extend(sgr("").encode_utf16());
    }
    String::from_utf16_lossy(&out)
}
#[derive(Default)]
struct Attributes {
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    inverse: bool,
    fg: String,
    bg: String,
}
impl Attributes {
    fn from_pen(pen: &str) -> Self {
        let mut value = Self::default();
        if !pen.is_empty() {
            value.apply(&csi_params(pen));
        }
        value
    }
    fn pen(&self) -> String {
        let mut parts = vec![];
        for (set, code) in [
            (self.bold, "1"),
            (self.dim, "2"),
            (self.italic, "3"),
            (self.underline, "4"),
            (self.inverse, "7"),
            (self.strike, "9"),
        ] {
            if set {
                parts.push(code);
            }
        }
        if !self.fg.is_empty() {
            parts.push(&self.fg);
        }
        if !self.bg.is_empty() {
            parts.push(&self.bg);
        }
        parts.join(";")
    }
    fn apply(&mut self, params: &[u64]) {
        if params.is_empty() {
            *self = Self::default();
            return;
        }
        let mut i = 0;
        while i < params.len() {
            let code = params[i];
            match code {
                0 => *self = Self::default(),
                1 => self.bold = true,
                2 => self.dim = true,
                3 => self.italic = true,
                4 => self.underline = true,
                7 => self.inverse = true,
                9 => self.strike = true,
                22 => {
                    self.bold = false;
                    self.dim = false;
                }
                23 => self.italic = false,
                24 => self.underline = false,
                27 => self.inverse = false,
                29 => self.strike = false,
                30..=37 | 90..=97 => self.fg = code.to_string(),
                40..=47 | 100..=107 => self.bg = code.to_string(),
                39 => self.fg.clear(),
                49 => self.bg.clear(),
                38 | 48 | 58 => {
                    let (color, next) = extended_color(code, params, i);
                    if let Some(color) = color
                        && code != 58
                    {
                        if code == 38 {
                            self.fg = color;
                        } else {
                            self.bg = color;
                        }
                    }
                    i = next;
                    continue;
                }
                _ => {}
            }
            i += 1;
        }
    }
}
fn extended_color(code: u64, params: &[u64], i: usize) -> (Option<String>, usize) {
    match params.get(i + 1) {
        Some(5) => (
            params
                .get(i + 2)
                .filter(|v| **v <= 255)
                .map(|v| format!("{code};5;{v}")),
            i + 3,
        ),
        Some(2) => {
            let color = if let (Some(r), Some(g), Some(b)) =
                (params.get(i + 2), params.get(i + 3), params.get(i + 4))
            {
                Some(format!("{code};2;{};{};{}", r & 255, g & 255, b & 255))
            } else {
                None
            };
            (color, i + 5)
        }
        _ => (None, i + 2),
    }
}
fn csi_params(text: &str) -> Vec<u64> {
    let mut out = vec![];
    let mut pending = 0i32;
    for part in text.split(';') {
        let mut sub: Vec<_> = part
            .split(':')
            .map(|value| {
                value
                    .parse::<u64>()
                    .unwrap_or(if value.is_empty() { 0 } else { u64::MAX })
            })
            .collect();
        let head = sub[0];
        if pending > 0 {
            out.push(head);
            pending -= 1;
            continue;
        }
        if pending == -1 {
            pending = 0;
            if sub.len() == 1 {
                out.push(head);
                if head == 5 || head == 2 {
                    pending = if head == 5 { 1 } else { 3 };
                }
                continue;
            }
            if head == 2 && sub.len() >= 5 {
                sub.remove(1);
            }
            out.extend(sub.into_iter().take(if head == 5 {
                2
            } else if head == 2 {
                4
            } else {
                1
            }));
            continue;
        }
        if sub.len() == 1 {
            out.push(head);
            if matches!(head, 38 | 48 | 58) {
                pending = -1;
            }
            continue;
        }
        if !matches!(head, 38 | 48 | 58) {
            out.push(if head == 4 && sub[1] == 0 { 24 } else { head });
            continue;
        }
        if sub[1] == 2 && sub.len() >= 6 {
            sub.remove(2);
        }
        let take = if sub[1] == 5 {
            3
        } else if sub[1] == 2 {
            5
        } else {
            2
        };
        out.extend(sub.into_iter().take(take));
    }
    out
}
fn string_introducer(ch: u16) -> bool {
    matches!(ch, 93 | 80 | 88 | 95 | 94)
}
fn escape_length(input: &[u16], index: usize) -> Option<usize> {
    let next = *input.get(index + 1)?;
    if next == ESC {
        return Some(1);
    }
    if next == 91 {
        return input[index + 2..]
            .iter()
            .position(|ch| (0x40..=0x7e).contains(ch))
            .map(|end| end + 3);
    }
    if string_introducer(next) {
        for cursor in index + 2..input.len() {
            match input[cursor] {
                7 => return Some(cursor - index + 1),
                10 => return Some(cursor - index),
                ESC => {
                    return input
                        .get(cursor + 1)
                        .map(|next| cursor - index + if *next == 92 { 2 } else { 0 });
                }
                _ => {}
            }
        }
        return None;
    }
    let mut cursor = index + 1;
    while input
        .get(cursor)
        .is_some_and(|ch| (0x20..=0x2f).contains(ch))
    {
        cursor += 1;
    }
    let final_byte = *input.get(cursor)?;
    Some(cursor - index + usize::from(cursor == index + 1 || (0x30..=0x7e).contains(&final_byte)))
}
struct Terminal {
    screen: Screen,
    dropped: bool,
    max: usize,
}
impl Terminal {
    fn write(&mut self, text: &[u16]) {
        let row = self.screen.row;
        let mut line = if self.screen.col == line_len(&self.screen.lines[row]) {
            let mut line = std::mem::take(&mut self.screen.lines[row]);
            push_run(
                &mut line,
                Run {
                    pen: self.screen.pen.clone(),
                    text: text.to_vec(),
                },
            );
            line
        } else {
            write_line(
                &self.screen.lines[row],
                self.screen.col,
                text,
                &self.screen.pen,
            )
        };
        self.screen.col += text.len();
        if line.len() > 2048 {
            let keep = line.len() - 256;
            let plain = line[..keep]
                .iter()
                .flat_map(|run| run.text.iter().copied())
                .collect();
            line.splice(
                ..keep,
                [Run {
                    pen: String::new(),
                    text: plain,
                }],
            );
        }
        let length = line_len(&line);
        if length > self.max {
            let mut removed = length - self.max;
            line = slice_line(&line, removed, length);
            if line
                .first()
                .and_then(|run| run.text.first())
                .is_some_and(|ch| (0xdc00..=0xdfff).contains(ch))
            {
                line = slice_line(&line, 1, length);
                removed += 1;
            }
            self.dropped = true;
            self.screen.col = self.screen.col.saturating_sub(removed);
            if let Some(saved) = &mut self.screen.saved
                && saved.row == row
            {
                saved.col = saved.col.saturating_sub(removed);
            }
        }
        self.screen.lines[row] = line;
    }
    fn scroll_out(&mut self) -> usize {
        let line = self.screen.lines.remove(0);
        let length = line_len(&line);
        self.screen.settled[1].push_str(&serialize_line(&line));
        self.screen.settled[1].push('\n');
        if units(&self.screen.settled[0]) + units(&self.screen.settled[1]) > 2 * self.max {
            let (parts, _) = tail_parts(&self.screen.settled, self.max);
            self.screen.settled = [
                parts.first().cloned().unwrap_or_default(),
                parts.get(1).cloned().unwrap_or_default(),
            ];
            self.dropped = true;
        }
        self.screen.row -= 1;
        if let Some(saved) = &mut self.screen.saved {
            saved.row = saved.row.saturating_sub(1);
        }
        length
    }
    fn newline(&mut self) {
        self.screen.row += 1;
        self.screen.col = 0;
        if self.screen.row == self.screen.lines.len() {
            self.screen.lines.push(vec![]);
        }
        while self.screen.lines.len() > LIVE_LINES {
            self.scroll_out();
        }
    }
    fn move_to(&mut self, row: i128, col: i128) {
        self.screen.row = row.clamp(0, (self.screen.lines.len() - 1) as i128) as usize;
        self.screen.col = col.clamp(0, TERMINAL_OUTPUT_TAIL_CHARS as i128) as usize;
    }
    fn csi(&mut self, params: &str, final_byte: u16) {
        if !params
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b';' || b == b':')
            || (final_byte != 109 && params.contains(':'))
        {
            return;
        }
        let numbers = if params.is_empty() {
            vec![]
        } else {
            csi_params(params)
        };
        if final_byte == 109 {
            let mut attributes = Attributes::from_pen(&self.screen.pen);
            attributes.apply(&numbers);
            self.screen.pen = attributes.pen();
            return;
        }
        let count = numbers.first().copied().unwrap_or(1).max(1) as i128;
        let row = self.screen.row as i128;
        let col = self.screen.col as i128;
        match final_byte {
            65 => self.move_to(row - count, col),
            66 => self.move_to(row + count, col),
            67 => self.move_to(row, col + count),
            68 => self.move_to(row, col - count),
            69 => self.move_to(row + count, 0),
            70 => self.move_to(row - count, 0),
            71 => self.move_to(row, count - 1),
            72 | 102 => self.move_to(
                count - 1,
                numbers.get(1).copied().unwrap_or(1).max(1) as i128 - 1,
            ),
            75 => {
                self.screen.lines[self.screen.row] = erase_line(
                    &self.screen.lines[self.screen.row],
                    self.screen.col,
                    numbers.first().copied().unwrap_or(0),
                )
            }
            74 => match numbers.first().copied().unwrap_or(0) {
                0 => {
                    self.screen.lines[self.screen.row] =
                        erase_line(&self.screen.lines[self.screen.row], self.screen.col, 0);
                    self.screen.lines.truncate(self.screen.row + 1);
                }
                1 => {
                    for line in &mut self.screen.lines[..self.screen.row] {
                        line.clear();
                    }
                    self.screen.lines[self.screen.row] =
                        erase_line(&self.screen.lines[self.screen.row], self.screen.col, 1);
                }
                2 => {
                    self.screen.lines = vec![vec![]];
                    self.screen.row = 0;
                    self.screen.col = 0;
                }
                3 => {
                    if self.screen.settled.iter().any(|part| !part.is_empty()) {
                        self.dropped = true;
                    }
                    self.screen.settled = Default::default();
                }
                _ => {}
            },
            115 => {
                self.screen.saved = Some(Cursor {
                    row: self.screen.row,
                    col: self.screen.col,
                })
            }
            117 => {
                if let Some(saved) = self.screen.saved {
                    self.move_to(saved.row as i128, saved.col as i128)
                }
            }
            _ => {}
        }
    }
    fn escape(&mut self, sequence: &[u16]) {
        if sequence.get(1) == Some(&91) {
            if let Some(final_byte) = sequence.last() {
                self.csi(
                    &String::from_utf16_lossy(&sequence[2..sequence.len() - 1]),
                    *final_byte,
                );
            }
        } else if sequence == [ESC, 55] {
            self.screen.saved = Some(Cursor {
                row: self.screen.row,
                col: self.screen.col,
            });
        } else if sequence == [ESC, 56] {
            if let Some(saved) = self.screen.saved {
                self.move_to(saved.row as i128, saved.col as i128);
            }
        } else if sequence == [ESC, 77] {
            self.move_to(self.screen.row as i128 - 1, self.screen.col as i128);
        }
    }
    fn feed(&mut self, input: &[u16]) -> String {
        let mut printable = 0;
        let mut index = 0;
        while index < input.len() {
            let code = input[index];
            if code == 9 || (code >= 32 && code != 127 && !(0x80..=0x9f).contains(&code)) {
                index += 1;
                continue;
            }
            if index > printable {
                self.write(&input[printable..index]);
            }
            printable = index + 1;
            match code {
                10 => self.newline(),
                13 => self.screen.col = 0,
                8 => self.screen.col = self.screen.col.saturating_sub(1),
                ESC => {
                    let Some(length) = escape_length(input, index) else {
                        let fragment = &input[index..];
                        if fragment.len() <= 64 {
                            return String::from_utf16_lossy(fragment);
                        }
                        if fragment.get(1).is_some_and(|ch| string_introducer(*ch)) {
                            let mut pending = fragment[..2].to_vec();
                            if fragment.last() == Some(&ESC) {
                                pending.push(ESC);
                            }
                            return String::from_utf16_lossy(&pending);
                        }
                        return String::new();
                    };
                    self.escape(&input[index..index + length]);
                    index += length;
                    printable = index;
                    continue;
                }
                _ => {}
            }
            index += 1;
        }
        if input.len() > printable {
            self.write(&input[printable..]);
        }
        String::new()
    }
}
pub fn terminal_output_tail(text: &str, max: usize) -> TerminalText {
    let input: Vec<_> = text.encode_utf16().collect();
    if input.len() <= max {
        return TerminalText {
            text: text.into(),
            truncated: false,
        };
    }
    let mut start = input.len() - max;
    if let Some(newline) = input[start..].iter().position(|ch| *ch == 10)
        && (newline as f64) < max as f64 / 4.0
    {
        return TerminalText {
            text: String::from_utf16_lossy(&input[start + newline + 1..]),
            truncated: true,
        };
    }
    let line_start = input[..start]
        .iter()
        .rposition(|ch| *ch == 10)
        .map_or(0, |i| i + 1);
    let mut prefix = vec![];
    let mut cursor = line_start;
    let mut cut_inside_escape = false;
    while cursor < start {
        let Some(offset) = input[cursor..].windows(2).position(|ch| ch == [ESC, 91]) else {
            break;
        };
        let found = cursor + offset;
        if found >= start {
            break;
        }
        let Some(end) = input[found..]
            .iter()
            .position(|ch| *ch == 109)
            .map(|i| i + found)
        else {
            break;
        };
        prefix = input[found..=end].to_vec();
        if end >= start {
            start = end + 1;
            cut_inside_escape = true;
            break;
        }
        cursor = end + 1;
    }
    if !cut_inside_escape && prefix == sgr("").encode_utf16().collect::<Vec<_>>() {
        prefix.clear();
    }
    if input
        .get(start)
        .is_some_and(|ch| (0xdc00..=0xdfff).contains(ch))
    {
        start += 1;
    }
    prefix.extend(&input[start..]);
    TerminalText {
        text: String::from_utf16_lossy(&prefix),
        truncated: true,
    }
}
fn tail_parts(parts: &[String], max: usize) -> (Vec<String>, bool) {
    let total: usize = parts.iter().map(|part| units(part)).sum();
    if total <= max {
        return (parts.to_vec(), false);
    }
    let start = total - max;
    let mut offset = 0;
    for (index, part) in parts.iter().enumerate() {
        let input: Vec<_> = part.encode_utf16().collect();
        if offset + input.len() > start
            && let Some(newline) = input[start.saturating_sub(offset)..]
                .iter()
                .position(|ch| *ch == 10)
                .map(|i| i + start.saturating_sub(offset))
        {
            if (offset + newline - start) as f64 >= max as f64 / 4.0 {
                break;
            }
            let mut result = vec![String::from_utf16_lossy(&input[newline + 1..])];
            result.extend_from_slice(&parts[index + 1..]);
            return (result, true);
        }
        offset += input.len();
    }
    (vec![terminal_output_tail(&parts.concat(), max).text], true)
}
pub fn append_terminal_output(
    state: &TerminalOutputState,
    chunk: &str,
    max: usize,
) -> TerminalOutputState {
    let input: Vec<_> = state
        .pending
        .encode_utf16()
        .chain(chunk.encode_utf16())
        .collect();
    if input.is_empty() {
        return state.clone();
    }
    let mut terminal = Terminal {
        screen: state.screen.clone(),
        dropped: false,
        max,
    };
    let pending = terminal.feed(&input);
    let mut truncated = state.truncated;
    let mut window: usize = terminal.screen.lines.iter().map(line_len).sum();
    while window > max && terminal.screen.row > 0 {
        window -= terminal.scroll_out();
    }
    if window > max {
        let used: usize = terminal.screen.lines[..=terminal.screen.row]
            .iter()
            .map(line_len)
            .sum();
        let mut budget = max.saturating_sub(used);
        for row in terminal.screen.row + 1..terminal.screen.lines.len() {
            let line = &terminal.screen.lines[row];
            let len = line_len(line);
            let keep = len.min(budget);
            if keep < len {
                truncated = true;
                terminal.screen.lines[row] = slice_line(line, len - keep, len);
                if let Some(saved) = &mut terminal.screen.saved
                    && saved.row == row
                {
                    saved.col = saved.col.saturating_sub(len - keep);
                }
            }
            budget -= keep;
        }
    }
    let (settled, settled_truncated) = tail_parts(&terminal.screen.settled, max);
    let window = terminal
        .screen
        .lines
        .iter()
        .map(serialize_line)
        .collect::<Vec<_>>()
        .join("\n");
    let mut parts = settled.clone();
    parts.push(window);
    let (tail, tail_truncated) = tail_parts(&parts, max);
    terminal.screen.settled = [
        settled.first().cloned().unwrap_or_default(),
        settled.get(1).cloned().unwrap_or_default(),
    ];
    TerminalOutputState {
        text: tail.concat(),
        truncated: truncated || terminal.dropped || settled_truncated || tail_truncated,
        pending,
        screen: terminal.screen,
    }
}
pub fn normalize_terminal_output(text: &str, max: usize) -> TerminalText {
    let state = append_terminal_output(&TerminalOutputState::default(), text, max);
    TerminalText {
        text: state.text,
        truncated: state.truncated,
    }
}
pub fn terminal_output_resume_text(state: &TerminalOutputState) -> String {
    let screen = &state.screen;
    let last = screen.lines.len() - 1;
    let reachable = state.text.matches('\n').count().min(LIVE_LINES - 1);
    let up = |row: usize| last.saturating_sub(row).min(reachable);
    let go = |row: usize, col: usize| {
        format!(
            "{}\x1b[{}G",
            if up(row) > 0 {
                format!("\x1b[{}A", up(row))
            } else {
                String::new()
            },
            col + 1
        )
    };
    let mut out = state.text.clone();
    if let Some(saved) = screen.saved {
        out.push_str(&go(saved.row, saved.col));
        out.push_str("\x1b7");
        if up(saved.row) > 0 {
            out.push_str(&format!("\x1b[{}B", up(saved.row)));
        }
    }
    if screen.saved.is_some()
        || screen.row < last
        || screen.col != line_len(&screen.lines[screen.row])
    {
        out.push_str(&go(screen.row, screen.col));
    }
    if !screen.pen.is_empty() {
        out.push_str(&sgr(&screen.pen));
    }
    out.push_str(&state.pending);
    out
}
pub fn terminal_output_plain_text(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut cursor = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 27 && bytes.get(i + 1) == Some(&b'[') {
            let mut end = i + 2;
            while bytes
                .get(end)
                .is_some_and(|ch| ch.is_ascii_digit() || *ch == b';')
            {
                end += 1;
            }
            if bytes.get(end) == Some(&b'm') {
                out.push_str(&text[cursor..i]);
                i = end + 1;
                cursor = i;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&text[cursor..]);
    out
}
