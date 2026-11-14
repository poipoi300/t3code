use crate::sequence::continuation_shifts;

struct C1Effect {
    next: String,
    query: bool,
    safe: String,
}

fn c1_effect(ch: char) -> Option<C1Effect> {
    let point = u32::from(ch);
    let mut next: Option<String> = None;
    let mut query = false;
    let mut safe = String::new();
    for shift in continuation_shifts(point) {
        let byte = 0x80 | ((point >> shift) & 0x3f);
        if next.as_deref() == Some("") {
            safe.push('\u{fffd}');
        } else if next.as_ref().is_some_and(|s| s.starts_with("\x1b]")) {
            if let Some(value) = &mut next {
                value.push('\u{fffd}');
            }
        } else {
            match byte {
                0x9b => next = Some("\x1b[".into()),
                0x9d => next = Some("\x1b]".into()),
                0x90 => next = Some("\x1bP".into()),
                0x98 | 0x9e | 0x9f => next = Some("\x1b_".into()),
                0x80..=0x9f => {
                    next = Some(String::new());
                    query = byte == 0x9a;
                    safe = if query || byte == 0x9c {
                        String::new()
                    } else {
                        format!("\x1b{}", char::from((byte - 0x40) as u8))
                    };
                }
                _ => {}
            }
        }
    }
    next.map(|next| C1Effect { next, query, safe })
}

fn strip_csi(body: &str, ch: char) -> bool {
    let params = |value: &str, allowed: &str| {
        value
            .chars()
            .all(|c| c.is_ascii_digit() || allowed.contains(c))
    };
    match ch {
        'n' => true,
        'R' => params(body, ";?"),
        'c' => params(body, ">=;?"),
        'p' | 'y' => body.strip_suffix('$').is_some_and(|s| params(s, ";?")),
        'q' => body.strip_prefix('>').is_some_and(|s| params(s, ";")),
        'u' => body.starts_with('?'),
        _ => false,
    }
}

fn strip_string(kind: u8, content: &str) -> bool {
    if kind == b']' {
        ["10;", "11;", "12;"].iter().any(|p| {
            content
                .strip_prefix(p)
                .is_some_and(|s| s.starts_with('?') || s.starts_with("rgb:"))
        })
    } else if kind == b'P' {
        let content = content.strip_prefix(['0', '1']).unwrap_or(content);
        ["$q", "$r", "+q", "+r"]
            .iter()
            .any(|s| content.starts_with(s))
    } else {
        false
    }
}

fn find_string_end(
    input: &str,
    start: usize,
    osc: bool,
) -> Option<(usize, Option<(usize, C1Effect)>)> {
    for (at, ch) in input[start..].char_indices() {
        let at = start + at;
        if matches!(ch, '\x18' | '\x1a') || (osc && ch == '\x07') {
            return Some((at + ch.len_utf8(), None));
        }
        if ch == '\x1b' {
            let next = input.as_bytes().get(at + 1)?;
            return Some((if *next == b'\\' { at + 2 } else { at }, None));
        }
        if ch >= '\u{80}'
            && !osc
            && let Some(effect) = c1_effect(ch)
        {
            return Some((at + ch.len_utf8(), Some((at, effect))));
        }
    }
    None
}

#[derive(Default, Debug)]
pub struct HistorySanitizer {
    pending: String,
}

impl HistorySanitizer {
    pub fn clear(&mut self) {
        self.pending.clear();
    }

    pub fn push(&mut self, data: &str) -> String {
        if self.pending.is_empty() && !data.contains('\x1b') {
            return data.into();
        }
        let pending_scan_start = if self.pending.ends_with('\x1b') {
            self.pending.len() - 1
        } else {
            self.pending.len()
        };
        let mut input = std::mem::take(&mut self.pending);
        input.push_str(data);
        let bytes = input.as_bytes();
        let mut out = String::new();
        let mut i = 0;
        let mut c1_introducer = String::new();
        while i < input.len() {
            if !c1_introducer.is_empty() || bytes[i] == 27 {
                let prefix = std::mem::take(&mut c1_introducer);
                let content_start = if prefix.is_empty() { i + 2 } else { i };
                let kind = if prefix.is_empty() {
                    bytes.get(i + 1)
                } else {
                    prefix.as_bytes().get(1)
                };
                let Some(&kind) = kind else {
                    self.pending = input[i..].into();
                    break;
                };
                if kind == b'[' {
                    let mut cursor = content_start;
                    let mut body = String::new();
                    let mut controls = String::new();
                    while cursor < input.len() {
                        let ch = input[cursor..].chars().next().expect("scalar boundary");
                        let end = cursor + ch.len_utf8();
                        if matches!(ch, '\x18' | '\x1a') || ('@'..='~').contains(&ch) {
                            if matches!(ch, '\x18' | '\x1a') || !strip_csi(&body, ch) {
                                out.push_str(&prefix);
                                out.push_str(&input[i..end]);
                            } else {
                                out.push_str(&controls);
                            }
                            i = end;
                            break;
                        }
                        if ch >= '\u{80}' {
                            if let Some(effect) = c1_effect(ch) {
                                if effect.next.is_empty() && !effect.query {
                                    out.push_str(&prefix);
                                    out.push_str(&input[i..end]);
                                } else {
                                    out.push_str(&controls);
                                    out.push_str(&effect.safe);
                                }
                                c1_introducer = effect.next;
                                i = end;
                                break;
                            }
                            cursor = end;
                            continue;
                        }
                        if ch == '\x1b' {
                            out.push_str(&controls);
                            i = cursor;
                            break;
                        }
                        if ch < '\x10' {
                            controls.push(ch);
                        } else if ch >= ' ' && ch != '\x7f' {
                            body.push(ch);
                        }
                        cursor = end;
                    }
                    if cursor >= input.len() {
                        self.pending = prefix + &input[i..];
                        break;
                    }
                    continue;
                }
                if matches!(kind, b']' | b'P' | b'^' | b'_') {
                    let Some((end, c1)) = find_string_end(
                        &input,
                        content_start.max(pending_scan_start),
                        kind == b']',
                    ) else {
                        self.pending = prefix + &input[i..];
                        break;
                    };
                    let content_end = c1.as_ref().map_or(end, |(at, _)| *at);
                    let content = prefix.get(2..).unwrap_or("").to_owned()
                        + &input[content_start..content_end];
                    let content = content
                        .strip_suffix("\x1b\\")
                        .or_else(|| content.strip_suffix(['\x07', '\u{9c}']))
                        .unwrap_or(&content);
                    let strip = strip_string(kind, content);
                    if let Some((at, effect)) = c1 {
                        if !strip && effect.next.is_empty() && !effect.query {
                            out.push_str(&prefix);
                            out.push_str(&input[i..end]);
                        } else {
                            if !strip {
                                out.push_str(&prefix);
                                out.push_str(&input[i..at]);
                                out.push('\u{9c}');
                            }
                            out.push_str(&effect.safe);
                            c1_introducer = effect.next;
                        }
                    } else if !strip {
                        out.push_str(&prefix);
                        out.push_str(&input[i..end]);
                    }
                    i = end;
                    continue;
                }
                let mut cursor = i + 1;
                while cursor < input.len() && (0x20..=0x2f).contains(&bytes[cursor]) {
                    cursor += 1;
                }
                if cursor == input.len() {
                    self.pending = input[i..].into();
                    break;
                }
                let end = if bytes[cursor] == 27 {
                    cursor
                } else if (0x30..=0x7e).contains(&bytes[cursor]) {
                    cursor + 1
                } else {
                    i + 1 + input[i + 1..].chars().next().map_or(0, char::len_utf8)
                };
                if kind != b'Z' {
                    out.push_str(&input[i..end]);
                }
                i = end;
            } else {
                let end = input[i..].find('\x1b').map_or(input.len(), |at| i + at);
                out.push_str(&input[i..end]);
                i = end;
            }
        }
        if self.pending.is_empty() {
            self.pending = c1_introducer;
        }
        out
    }
}
