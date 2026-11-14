//! Ground-state tracking from packages/contracts/src/terminal.ts.

pub const TERMINAL_SEQUENCE_GROUND: u32 = 0;
const ESCAPE: u32 = 1;
const ESCAPE_INTERMEDIATE: u32 = 2;
const CSI: u32 = 3;
const DCS: u32 = 4;
const SOS_PM_APC: u32 = 5;
const OSC: u32 = 6;

fn next_state(state: u32, unit: u16) -> u32 {
    if unit == 0x1b {
        return ESCAPE;
    }
    if state == 0 || matches!(unit, 0x18 | 0x1a) {
        return 0;
    }
    let mut mode = state & 7;
    if unit < 0x80 {
        return match mode {
            ESCAPE => match unit {
                0..=0x1f | 0x7f => mode,
                0x20..=0x2f => ESCAPE_INTERMEDIATE,
                0x5b => CSI,
                0x5d => OSC,
                0x50 => DCS,
                0x58 | 0x5e | 0x5f => SOS_PM_APC,
                _ => 0,
            },
            ESCAPE_INTERMEDIATE if unit >= 0x30 && unit != 0x7f => 0,
            CSI if unit >= 0x40 && unit != 0x7f => 0,
            OSC if unit == 0x07 => 0,
            _ => mode,
        };
    }
    if mode == OSC {
        return mode;
    }
    if (0xd800..=0xdbff).contains(&unit) {
        return mode | (u32::from(unit) << 3);
    }
    let mut point = u32::from(unit);
    if (0xdc00..=0xdfff).contains(&unit) {
        if state < 8 {
            return mode;
        }
        point = 0x10000 + (((state >> 3) - 0xd800) << 10) + (point - 0xdc00);
    }
    for shift in continuation_shifts(point) {
        let byte = 0x80 | ((point >> shift) & 0x3f);
        match byte {
            0xa0.. => {}
            0x9d => return OSC,
            0x90 => mode = DCS,
            0x9b => mode = CSI,
            0x98 | 0x9e | 0x9f => mode = SOS_PM_APC,
            _ => return 0,
        }
    }
    mode
}

pub(crate) fn continuation_shifts(point: u32) -> impl Iterator<Item = u32> {
    (0..=if point > 0xffff {
        2
    } else if point > 0x7ff {
        1
    } else {
        0
    })
        .rev()
        .map(|n| n * 6)
}

/// UTF-16 entry point preserves a high surrogate when chunks split a pair.
pub fn terminal_sequence_state_after_utf16(mut state: u32, units: &[u16]) -> u32 {
    let start = match units.iter().rposition(|unit| *unit == 0x1b) {
        Some(at) => {
            state = ESCAPE;
            at + 1
        }
        None => 0,
    };
    for &unit in &units[start..] {
        if state == 0 {
            break;
        }
        state = next_state(state, unit);
    }
    state
}

/// The first UTF-16 offset back in ground state, or the end of this chunk.
pub fn terminal_sequence_end_utf16(mut state: u32, units: &[u16]) -> usize {
    let mut end = 0;
    while state != 0 && end < units.len() {
        state = next_state(state, units[end]);
        end += 1;
    }
    end
}

pub fn terminal_sequence_state_after(mut state: u32, text: &str) -> u32 {
    let start = match text.rfind('\x1b') {
        Some(at) => {
            state = ESCAPE;
            at + 1
        }
        None => 0,
    };
    for unit in text[start..].encode_utf16() {
        if state == 0 {
            break;
        }
        state = next_state(state, unit);
    }
    state
}

/// The first UTF-8 byte offset back in ground state, always on a scalar boundary.
pub fn terminal_sequence_end(mut state: u32, text: &str) -> usize {
    for (at, ch) in text.char_indices() {
        if state == 0 {
            return at;
        }
        for unit in ch.encode_utf16(&mut [0; 2]) {
            state = next_state(state, *unit);
        }
    }
    text.len()
}

#[derive(Default)]
pub(crate) struct Utf8SequenceState {
    pub state: u32,
    pending: Vec<u8>,
}

impl Utf8SequenceState {
    pub fn feed(&mut self, bytes: &[u8], finish: bool) {
        self.pending.extend_from_slice(bytes);
        let mut offset = 0;
        while offset < self.pending.len() {
            match std::str::from_utf8(&self.pending[offset..]) {
                Ok(text) => {
                    self.state = terminal_sequence_state_after(self.state, text);
                    offset = self.pending.len();
                }
                Err(error) => {
                    let valid_end = offset + error.valid_up_to();
                    if let Ok(text) = std::str::from_utf8(&self.pending[offset..valid_end]) {
                        self.state = terminal_sequence_state_after(self.state, text);
                    }
                    offset = valid_end;
                    if let Some(length) = error.error_len() {
                        self.state = terminal_sequence_state_after(self.state, "\u{fffd}");
                        offset += length;
                    } else {
                        if finish {
                            self.state = terminal_sequence_state_after(self.state, "\u{fffd}");
                            offset = self.pending.len();
                        }
                        break;
                    }
                }
            }
        }
        self.pending.drain(..offset);
    }
}
