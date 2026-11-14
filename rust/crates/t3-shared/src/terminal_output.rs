//! Native port of shared/terminalOutput.ts. Cursor columns and budgets are UTF-16 units.
pub const TERMINAL_OUTPUT_TAIL_CHARS: usize = 64 * 1024;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalText {
    pub text: String,
    pub truncated: bool,
}
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;

const ESC: u16 = 27;
const LIVE_LINES: usize = 64;
/// One line: all its units in one buffer, and TypeScript's runs over them as pen and length.
/// A write overwrites units in place and splits, merges or flattens runs by changing lengths,
/// so it never copies text it does not change, as TypeScript's sliced and concatenated strings.
#[derive(Clone, Default)]
struct Line {
    text: VecDeque<u16>,
    runs: Vec<Run>,
}
#[derive(Clone)]
struct Run {
    pen: String,
    len: usize,
}
/// A run as the JSON state spells it.
#[derive(Serialize, Deserialize)]
struct RunJson {
    pen: String,
    #[serde(with = "utf16_text")]
    text: Vec<u16>,
}
impl Serialize for Line {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        let mut offset = 0;
        serializer.collect_seq(self.runs.iter().map(|run| {
            let text = self.text.range(offset..offset + run.len).copied().collect();
            offset += run.len;
            RunJson {
                pen: run.pen.clone(),
                text,
            }
        }))
    }
}
impl<'de> Deserialize<'de> for Line {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let mut line = Line::default();
        for run in Vec::<RunJson>::deserialize(deserializer)? {
            line.runs.push(Run {
                pen: run.pen,
                len: run.text.len(),
            });
            line.text.extend(run.text);
        }
        Ok(line)
    }
}
impl<'a> IntoIterator for &'a Line {
    type Item = &'a Run;
    type IntoIter = std::slice::Iter<'a, Run>;
    fn into_iter(self) -> Self::IntoIter {
        self.runs.iter()
    }
}
impl Line {
    fn clear(&mut self) {
        self.text.clear();
        self.runs.clear();
    }
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
mod utf16_parts {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<S: Serializer>(
        parts: &[Vec<u16>; 2],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Text<'a>(#[serde(with = "super::utf16_text")] &'a [u16]);
        [Text(&parts[0]), Text(&parts[1])].serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<[Vec<u16>; 2], D::Error> {
        #[derive(Deserialize)]
        struct Text(#[serde(with = "super::utf16_text")] Vec<u16>);
        let [Text(head), Text(tail)] = <[Text; 2]>::deserialize(deserializer)?;
        Ok([head, tail])
    }
}
#[derive(Clone, Copy, Serialize, Deserialize)]
struct Cursor {
    row: usize,
    col: usize,
}
#[derive(Clone, Serialize, Deserialize)]
struct Screen {
    #[serde(with = "utf16_parts")]
    settled: [Vec<u16>; 2],
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
                lines: vec![Line::default()],
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
impl NativeTerminalOutputFold {
    pub fn empty(&self) -> Value {
        serde_json::json!({"text":"","truncated":false,"pending":"","screen":{"settled":["",""],"lines":[[]],"row":0,"col":0,"pen":"","saved":null}})
    }
    pub fn append(&self, state: &Value, chunk: &str) -> Result<Value> {
        let state: TerminalOutputState = serde_json::from_value(state.clone())?;
        validate(&state)?;
        Ok(serde_json::to_value(append_terminal_output(
            &state,
            chunk,
            TERMINAL_OUTPUT_TAIL_CHARS,
        ))?)
    }
    pub fn normalize(&self, text: &str) -> Result<TerminalText> {
        Ok(normalize_terminal_output(text, TERMINAL_OUTPUT_TAIL_CHARS))
    }
    pub fn resume(&self, state: &Value) -> Result<TerminalText> {
        let state: TerminalOutputState = serde_json::from_value(state.clone())?;
        validate(&state)?;
        Ok(TerminalText {
            text: terminal_output_resume_text(&state),
            truncated: state.truncated,
        })
    }
    pub fn settled(&self, state: &Value) -> Result<TerminalText> {
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

fn sgr(pen: &str) -> String {
    if pen.is_empty() {
        "\x1b[0m".into()
    } else {
        format!("\x1b[0;{pen}m")
    }
}
fn line_len(line: &Line) -> usize {
    line.text.len()
}
/// `sliceLine(line, start, end)` in place, keeping the same runs.
fn slice_in_place(line: &mut Line, start: usize, end: usize) {
    let mut offset = 0;
    let mut done = false;
    line.runs.retain_mut(|run| {
        if done {
            return false;
        }
        let run_start = offset;
        let run_end = run_start + run.len;
        offset = run_end;
        done = run_end >= end;
        let kept = run_end > start && run_start < end;
        if kept {
            run.len = run_end.min(end) - run_start.max(start);
        }
        kept
    });
    line.text.truncate(end);
    let start = start.min(line.text.len());
    line.text.drain(..start);
}
/// `trimLineStart`: drops `count` units from the front, and the runs that empties.
fn trim_line_start(line: &mut Line, count: usize) {
    let mut remaining = count;
    let mut dropped = 0;
    while remaining > 0 && dropped < line.runs.len() {
        let run = &mut line.runs[dropped];
        if run.len <= remaining {
            remaining -= run.len;
            dropped += 1;
        } else {
            run.len -= remaining;
            remaining = 0;
        }
    }
    line.runs.drain(..dropped);
    line.text.drain(..count - remaining);
}
/// `pushRun`: appends to the last run when the pen matches.
fn push_units(line: &mut Line, pen: &str, text: impl ExactSizeIterator<Item = u16>) {
    let added = text.len();
    if added == 0 {
        return;
    }
    match line.runs.last_mut() {
        Some(last) if last.pen == pen => last.len += added,
        _ => line.runs.push(Run {
            pen: pen.into(),
            len: added,
        }),
    }
    line.text.extend(text);
}
/// `writeLine` rebuilds the line through `pushRun`: no empty runs, no neighbours sharing a pen.
fn canonicalize(line: &mut Line) {
    line.runs.retain(|run| run.len > 0);
    line.runs.dedup_by(|next, kept| {
        let same = next.pen == kept.pen;
        if same {
            kept.len += next.len;
        }
        same
    });
}
/// The run holding unit `position` of a canonical line, and where that run starts.
fn run_at(line: &Line, position: usize) -> (usize, usize) {
    let mut start = 0;
    for (index, run) in line.runs.iter().enumerate() {
        if position < start + run.len {
            return (index, start);
        }
        start += run.len;
    }
    (line.runs.len(), start)
}
/// `writeLine` away from the line's end: the canonical runs of the units before `col`, padding,
/// `text` in `pen`, then the units after it. Units are overwritten in place; runs change length.
fn write_within(line: &mut Line, col: usize, text: &[u16], pen: &str) {
    canonicalize(line);
    let len = line_len(line);
    if col >= len {
        push_units(line, "", std::iter::repeat_n(32, col - len));
        push_units(line, pen, text.iter().copied());
        return;
    }
    let end = col + text.len();
    let overlap = end.min(len) - col;
    for (slot, unit) in line.text.range_mut(col..col + overlap).zip(text) {
        *slot = *unit;
    }
    line.text.extend(&text[overlap..]);
    let (first, first_start) = run_at(line, col);
    let (last, last_start) = if end < len {
        run_at(line, end)
    } else {
        (line.runs.len(), len)
    };
    if first == last && line.runs[first].pen == pen {
        return;
    }
    let left = Run {
        pen: line.runs[first].pen.clone(),
        len: col - first_start,
    };
    let right = match line.runs.get(last) {
        Some(run) => Run {
            pen: run.pen.clone(),
            len: last_start + run.len - end,
        },
        None => Run {
            pen: String::new(),
            len: 0,
        },
    };
    let written = Run {
        pen: pen.into(),
        len: text.len(),
    };
    let replaced = (last + 1).min(line.runs.len());
    line.runs.splice(first..replaced, [left, written, right]);
    canonicalize(line);
}
/// `eraseLine`: 0 cursor to end, 1 start through cursor, 2 whole line.
fn erase_in_line(line: &mut Line, col: usize, mode: f64) {
    if mode == 0.0 {
        slice_in_place(line, 0, col);
    } else if mode == 1.0 {
        let spaces = vec![32; (col + 1).min(line_len(line))];
        write_within(line, 0, &spaces, "");
    } else if mode == 2.0 {
        line.clear();
    }
}
fn serialize_line(line: &Line) -> Vec<u16> {
    let mut out = Vec::with_capacity(line.text.len());
    let mut current = "";
    let mut offset = 0;
    for run in &line.runs {
        if run.pen != current {
            out.extend(sgr(&run.pen).encode_utf16());
            current = &run.pen;
        }
        out.extend(line.text.range(offset..offset + run.len));
        offset += run.len;
    }
    if !current.is_empty() {
        out.extend(sgr("").encode_utf16());
    }
    out
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
    /// `parsePen`: a pen is `;`-separated numbers.
    fn from_pen(pen: &str) -> Self {
        let mut value = Self::default();
        if !pen.is_empty() {
            value.apply(&pen.split(';').map(js_number).collect::<Vec<_>>());
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
    /// `applySgr` over JavaScript numbers.
    fn apply(&mut self, params: &[f64]) {
        if params.is_empty() {
            *self = Self::default();
            return;
        }
        let mut i = 0;
        while i < params.len() {
            let code = params[i];
            let within = |low: f64, high: f64| (low..=high).contains(&code);
            if code == 0.0 || code.is_nan() {
                *self = Self::default();
            } else if code == 1.0 {
                self.bold = true;
            } else if code == 2.0 {
                self.dim = true;
            } else if code == 3.0 {
                self.italic = true;
            } else if code == 4.0 {
                self.underline = true;
            } else if code == 7.0 {
                self.inverse = true;
            } else if code == 9.0 {
                self.strike = true;
            } else if code == 22.0 {
                self.bold = false;
                self.dim = false;
            } else if code == 23.0 {
                self.italic = false;
            } else if code == 24.0 {
                self.underline = false;
            } else if code == 27.0 {
                self.inverse = false;
            } else if code == 29.0 {
                self.strike = false;
            } else if within(30.0, 37.0) || within(90.0, 97.0) {
                self.fg = code.to_string();
            } else if within(40.0, 47.0) || within(100.0, 107.0) {
                self.bg = code.to_string();
            } else if code == 39.0 {
                self.fg.clear();
            } else if code == 49.0 {
                self.bg.clear();
            } else if [38.0, 48.0, 58.0].contains(&code) {
                let (color, next) = extended_color(code, params, i);
                if let Some(color) = color
                    && code != 58.0
                {
                    if code == 38.0 {
                        self.fg = color;
                    } else {
                        self.bg = color;
                    }
                }
                i = next;
                continue;
            }
            i += 1;
        }
    }
}
/// JavaScript's `Number` for the digit strings a pen or CSI holds: the empty string is 0, and
/// anything else that is not plain digits is NaN (`Number` would also accept signs, spaces, hex
/// and exponents, which a CSI cannot carry).
fn js_number(text: &str) -> f64 {
    if text.is_empty() {
        0.0
    } else if text.bytes().all(|byte| byte.is_ascii_digit()) {
        text.parse().unwrap_or(f64::INFINITY)
    } else {
        f64::NAN
    }
}
/// JavaScript's `value & 255`, which first applies ToInt32.
fn low_byte(value: f64) -> u32 {
    if value.is_finite() {
        (value.trunc().rem_euclid(4_294_967_296.0) as u32) & 255
    } else {
        0
    }
}
fn extended_color(code: f64, params: &[f64], i: usize) -> (Option<String>, usize) {
    match params.get(i + 1) {
        Some(5.0) => (
            params
                .get(i + 2)
                // `value > 255` is false for NaN, which TypeScript keeps.
                .filter(|value| (**value).partial_cmp(&255.0) != Some(std::cmp::Ordering::Greater))
                .map(|value| format!("{code};5;{value}")),
            i + 3,
        ),
        Some(2.0) => {
            let color = if let (Some(r), Some(g), Some(b)) =
                (params.get(i + 2), params.get(i + 3), params.get(i + 4))
            {
                Some(format!(
                    "{code};2;{};{};{}",
                    low_byte(*r),
                    low_byte(*g),
                    low_byte(*b)
                ))
            } else {
                None
            };
            (color, i + 5)
        }
        _ => (None, i + 2),
    }
}
fn csi_params(text: &str) -> Vec<f64> {
    let mut out = vec![];
    let mut pending = 0i32;
    for part in text.split(';') {
        let mut sub: Vec<_> = part.split(':').map(js_number).collect();
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
                if head == 5.0 || head == 2.0 {
                    pending = if head == 5.0 { 1 } else { 3 };
                }
                continue;
            }
            if head == 2.0 && sub.len() >= 5 {
                sub.remove(1);
            }
            out.extend(sub.into_iter().take(if head == 5.0 {
                2
            } else if head == 2.0 {
                4
            } else {
                1
            }));
            continue;
        }
        let colour = [38.0, 48.0, 58.0].contains(&head);
        if sub.len() == 1 {
            out.push(head);
            if colour {
                pending = -1;
            }
            continue;
        }
        if !colour {
            out.push(if head == 4.0 && sub[1] == 0.0 {
                24.0
            } else {
                head
            });
            continue;
        }
        if sub[1] == 2.0 && sub.len() >= 6 {
            sub.remove(2);
        }
        let take = if sub[1] == 5.0 {
            3
        } else if sub[1] == 2.0 {
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
    settled_units: usize,
}
impl Terminal {
    /// TypeScript's `write`, in place: a line is never copied to change part of it.
    fn write(&mut self, text: &[u16]) {
        let row = self.screen.row;
        let line = &mut self.screen.lines[row];
        if self.screen.col == line_len(line) {
            push_units(line, &self.screen.pen, text.iter().copied());
        } else {
            write_within(line, self.screen.col, text, &self.screen.pen);
        }
        self.screen.col += text.len();
        if line.runs.len() > 2048 {
            // Flatten old runs so later trimming stays cheap: one unstyled run, no text moved.
            let keep = line.runs.len() - 256;
            let len = line.runs.drain(..keep).map(|run| run.len).sum();
            line.runs.insert(
                0,
                Run {
                    pen: String::new(),
                    len,
                },
            );
        }
        let length = line_len(line);
        if length > self.max {
            let mut removed = length - self.max;
            trim_line_start(line, removed);
            // `line[0]?.text.charCodeAt(0)`: an empty first run reads NaN.
            if line.runs.first().is_some_and(|run| run.len > 0)
                && line
                    .text
                    .front()
                    .is_some_and(|ch| (0xdc00..=0xdfff).contains(ch))
            {
                trim_line_start(line, 1);
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
    }
    fn scroll_out(&mut self) -> usize {
        let line = self.screen.lines.remove(0);
        let length = line_len(&line);
        let rendered = serialize_line(&line);
        self.settled_units += rendered.len() + 1;
        self.screen.settled[1].extend(rendered);
        self.screen.settled[1].push(10);
        if self.settled_units > 2 * self.max {
            let (parts, _) = tail_parts(&self.screen.settled, self.max);
            self.screen.settled = [
                parts.first().cloned().unwrap_or_default(),
                parts.get(1).cloned().unwrap_or_default(),
            ];
            self.settled_units = self.screen.settled.iter().map(Vec::len).sum();
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
            self.screen.lines.push(Line::default());
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
        // `Math.max(1, n || 1)`; a count past any window clamps the same in both.
        let count = |n: Option<f64>| {
            let n = n.filter(|n| *n != 0.0 && !n.is_nan()).unwrap_or(1.0);
            n.clamp(1.0, u64::MAX as f64) as i128
        };
        let count_at = |index: usize| count(numbers.get(index).copied());
        let count = count_at(0);
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
            72 | 102 => self.move_to(count - 1, count_at(1) - 1),
            75 => erase_in_line(
                &mut self.screen.lines[self.screen.row],
                self.screen.col,
                numbers.first().copied().unwrap_or(0.0),
            ),
            74 => {
                let mode = numbers.first().copied().unwrap_or(0.0);
                if mode == 0.0 {
                    erase_in_line(
                        &mut self.screen.lines[self.screen.row],
                        self.screen.col,
                        0.0,
                    );
                    self.screen.lines.truncate(self.screen.row + 1);
                } else if mode == 1.0 {
                    for line in &mut self.screen.lines[..self.screen.row] {
                        line.clear();
                    }
                    erase_in_line(
                        &mut self.screen.lines[self.screen.row],
                        self.screen.col,
                        1.0,
                    );
                } else if mode == 3.0 {
                    if self.screen.settled.iter().any(|part| !part.is_empty()) {
                        self.dropped = true;
                    }
                    self.screen.settled = Default::default();
                    self.settled_units = 0;
                } else if mode == 2.0 {
                    self.screen.lines = vec![Line::default()];
                    self.screen.row = 0;
                    self.screen.col = 0;
                }
            }
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
    terminal_output_tail_units(&text.encode_utf16().collect::<Vec<_>>(), max)
}
pub fn terminal_output_tail_units(input: &[u16], max: usize) -> TerminalText {
    let (tail, truncated) = tail_units(input, max);
    TerminalText {
        text: String::from_utf16_lossy(&tail),
        truncated,
    }
}
fn tail_units(input: &[u16], max: usize) -> (Vec<u16>, bool) {
    if input.len() <= max {
        return (input.to_vec(), false);
    }
    let mut start = input.len() - max;
    if let Some(newline) = input[start..].iter().position(|ch| *ch == 10)
        && (newline as f64) < max as f64 / 4.0
    {
        return (input[start + newline + 1..].to_vec(), true);
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
    (prefix, true)
}
fn tail_parts(parts: &[Vec<u16>], max: usize) -> (Vec<Vec<u16>>, bool) {
    let total: usize = parts.iter().map(Vec::len).sum();
    if total <= max {
        return (parts.to_vec(), false);
    }
    let start = total - max;
    let mut offset = 0;
    for (index, part) in parts.iter().enumerate() {
        let input = part;
        if offset + input.len() > start
            && let Some(newline) = input[start.saturating_sub(offset)..]
                .iter()
                .position(|ch| *ch == 10)
                .map(|i| i + start.saturating_sub(offset))
        {
            if (offset + newline - start) as f64 >= max as f64 / 4.0 {
                break;
            }
            let mut result = vec![input[newline + 1..].to_vec()];
            result.extend_from_slice(&parts[index + 1..]);
            return (result, true);
        }
        offset += input.len();
    }
    (vec![tail_units(&parts.concat(), max).0], true)
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
        settled_units: state.screen.settled.iter().map(Vec::len).sum(),
    };
    let pending = terminal.feed(&input);
    let mut truncated = state.truncated;
    bound_window(&mut terminal, &mut truncated, max);
    let (settled, settled_truncated) = tail_parts(&terminal.screen.settled, max);
    let window = terminal
        .screen
        .lines
        .iter()
        .map(serialize_line)
        .collect::<Vec<_>>()
        .join(&10);
    let mut parts = settled.clone();
    parts.push(window);
    let (tail, tail_truncated) = tail_parts(&parts, max);
    terminal.screen.settled = [
        settled.first().cloned().unwrap_or_default(),
        settled.get(1).cloned().unwrap_or_default(),
    ];
    TerminalOutputState {
        text: String::from_utf16_lossy(&tail.concat()),
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

mod spans;
pub use spans::*;

fn bound_window(terminal: &mut Terminal, truncated: &mut bool, max: usize) {
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
            let len = line_len(&terminal.screen.lines[row]);
            let keep = len.min(budget);
            if keep < len {
                *truncated = true;
                slice_in_place(&mut terminal.screen.lines[row], len - keep, len);
                if let Some(saved) = &mut terminal.screen.saved
                    && saved.row == row
                {
                    saved.col = saved.col.saturating_sub(len - keep);
                }
            }
            budget -= keep;
        }
    }
}
mod incremental;
pub use incremental::{SettledChange, SettledCursor, TerminalOutput};
