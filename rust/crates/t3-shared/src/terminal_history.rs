//! UTF-8 history retention outside cut control sequences, from contracts/terminal.ts.
pub const TERMINAL_SEQUENCE_GROUND: u32 = 0;

fn next(state: u32, unit: u16) -> u32 {
    if unit == 0x1b {
        return 1;
    }
    if state == 0 || matches!(unit, 0x18 | 0x1a) {
        return 0;
    }
    let mut mode = state & 7;
    if unit < 0x80 {
        return match mode {
            1 => match unit {
                0..=0x1f | 0x7f => mode,
                0x20..=0x2f => 2,
                0x5b => 3,
                0x5d => 6,
                0x50 => 4,
                0x58 | 0x5e | 0x5f => 5,
                _ => 0,
            },
            2 if unit >= 0x30 && unit != 0x7f => 0,
            3 if unit >= 0x40 && unit != 0x7f => 0,
            6 if unit == 7 => 0,
            _ => mode,
        };
    }
    if mode == 6 {
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
        point = 0x10000 + (((state >> 3) - 0xd800) << 10) + (u32::from(unit) - 0xdc00);
    }
    let shift = if point > 0xffff {
        12
    } else if point > 0x7ff {
        6
    } else {
        0
    };
    for shift in (0..=shift / 6).rev().map(|i| i * 6) {
        let byte = 0x80 | ((point >> shift) & 0x3f);
        if byte >= 0xa0 {
            continue;
        }
        match byte {
            0x9d => return 6,
            0x90 => mode = 4,
            0x9b => mode = 3,
            0x98 | 0x9e..=0x9f => mode = 5,
            _ => return 0,
        }
    }
    mode
}

/// `start` and `end` are UTF-16 unit offsets, as in JavaScript slice.
pub fn terminal_sequence_state_after_units(
    mut state: u32,
    text: &[u16],
    start: usize,
    end: usize,
) -> u32 {
    let end = end.min(text.len());
    if end <= start {
        return state;
    }
    let mut start = start;
    if let Some(at) = text[start..end].iter().rposition(|u| *u == 0x1b) {
        state = 1;
        start += at + 1;
    }
    for &unit in &text[start..end] {
        if state == 0 {
            break;
        }
        state = next(state, unit);
    }
    state
}
pub fn terminal_sequence_state_after(state: u32, text: &str) -> u32 {
    let units: Vec<_> = text.encode_utf16().collect();
    terminal_sequence_state_after_units(state, &units, 0, units.len())
}
pub fn terminal_sequence_end_units(mut state: u32, text: &[u16]) -> usize {
    let mut index = 0;
    while state != 0 && index < text.len() {
        state = next(state, text[index]);
        index += 1;
    }
    index
}
/// Returns a UTF-16 offset, including a terminator consumed to return to ground.
pub fn terminal_sequence_end(state: u32, text: &str) -> usize {
    terminal_sequence_end_units(state, &text.encode_utf16().collect::<Vec<_>>())
}
pub fn retained_terminal_history(history: &str, max_bytes: usize) -> String {
    if max_bytes == 0 {
        return String::new();
    }
    if history.len() <= max_bytes {
        return history.into();
    }
    let mut start = history.len() - max_bytes;
    while !history.is_char_boundary(start) {
        start += 1;
    }
    let state = terminal_sequence_state_after(0, &history[..start]);
    let retained: Vec<_> = history[start..].encode_utf16().collect();
    String::from_utf16_lossy(&retained[terminal_sequence_end_units(state, &retained)..])
}
