use crate::Frame;

pub const FRAME_PART_BYTES: usize = 48 * 1024;
const PART_PREFIX: char = '\u{1e}';

/// Ordered parts must be written atomically relative to other messages.
pub fn split(frame: Frame) -> Vec<Frame> {
    let Frame::Text(text) = frame else {
        return vec![frame];
    };
    if text.len() < FRAME_PART_BYTES
        || (text.len() == FRAME_PART_BYTES && text.encode_utf16().count() <= FRAME_PART_BYTES / 3)
    {
        return vec![Frame::Text(text)];
    }
    let mut parts = Vec::new();
    let mut start = 0;
    let mut bytes = 1;
    for (index, c) in text.char_indices() {
        if bytes + c.len_utf8() > FRAME_PART_BYTES {
            parts.push(Frame::Text(format!("{PART_PREFIX}{}", &text[start..index])));
            start = index;
            bytes = 1;
        }
        bytes += c.len_utf8();
    }
    parts.push(Frame::Text(text[start..].into()));
    parts
}

#[derive(Default)]
pub struct Joiner {
    pending: String,
}

impl Joiner {
    /// Partial frames become synthetic pongs, matching the TypeScript liveness shim.
    pub fn feed(&mut self, frame: Frame) -> Frame {
        let Frame::Text(text) = frame else {
            return frame;
        };
        if let Some(part) = text.strip_prefix(PART_PREFIX) {
            self.pending.push_str(part);
            return Frame::Text("{\"_tag\":\"Pong\"}".into());
        }
        if self.pending.is_empty() {
            return Frame::Text(text);
        }
        self.pending.push_str(&text);
        Frame::Text(std::mem::take(&mut self.pending))
    }

    pub fn reset(&mut self) {
        self.pending.clear();
    }
}
