//! TanStack decodePath + encodePathLikeUrl; raw query/hash are supplied separately by routes.
use super::{decode_segment, encode_component};

fn escape_at(bytes: &[u8], index: usize) -> Option<u8> {
    if bytes.get(index) != Some(&b'%') {
        return None;
    }
    let high = char::from(*bytes.get(index + 1)?).to_digit(16)?;
    let low = char::from(*bytes.get(index + 2)?).to_digit(16)?;
    Some((high * 16 + low) as u8)
}

fn decode_uri(segment: &str) -> String {
    let valid = decode_segment(segment).is_some();
    let mut transformed = String::new();
    let bytes = segment.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if let Some(decoded) = escape_at(bytes, cursor) {
            if b";/?:@&=+$,#".contains(&decoded) {
                transformed.push_str(if valid { "%25" } else { "%" });
                transformed.push_str(&segment[cursor + 1..cursor + 3]);
            } else if valid {
                transformed.push_str(&segment[cursor..cursor + 3]);
            } else if decoded.is_ascii() {
                transformed.push(char::from(decoded));
            } else {
                transformed.push_str(&segment[cursor..cursor + 3]);
            }
            cursor += 3;
        } else {
            let ch = segment[cursor..]
                .chars()
                .next()
                .expect("character boundary");
            transformed.push(ch);
            cursor += ch.len_utf8();
        }
    }
    if valid {
        decode_segment(&transformed).unwrap_or(transformed)
    } else {
        transformed
    }
}

pub fn normalize_app_path(path: &str) -> String {
    let end = path.find(['?', '#']).unwrap_or(path.len());
    let pathname = &path[..end];
    let mut decoded = String::new();
    let mut start = 0;
    let mut cursor = 0;
    while cursor < pathname.len() {
        if matches!(escape_at(pathname.as_bytes(), cursor), Some(0x25 | 0x5c)) {
            decoded.push_str(&decode_uri(&pathname[start..cursor]));
            decoded.push_str(&pathname[cursor..cursor + 3]);
            cursor += 3;
            start = cursor;
        } else {
            cursor += 1;
        }
    }
    decoded.push_str(&decode_uri(&pathname[start..]));
    let decoded = decoded
        .chars()
        .filter(|ch| !ch.is_ascii_control())
        .collect::<String>();
    let decoded = if decoded.starts_with("//") {
        format!("/{}", decoded.trim_start_matches('/'))
    } else {
        decoded
    };
    let mut output = String::new();
    for ch in decoded.chars() {
        if !ch.is_ascii() || crate::view_logic::is_js_whitespace(ch) {
            output.push_str(&encode_component(&ch.to_string()));
        } else {
            output.push(ch);
        }
    }
    output.push_str(&path[end..]);
    output
}
