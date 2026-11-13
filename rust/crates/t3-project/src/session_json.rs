use serde_json::{Map, Value};
use std::io::BufRead;

#[derive(Debug, thiserror::Error)]
pub enum TranscriptJsonError {
    #[error("Transcript selected history exceeds the allocation budget")]
    Budget,
    #[error("Transcript JSON nesting exceeds the depth limit")]
    Depth,
    #[error("Transcript I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Malformed transcript JSON")]
    Malformed,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonPathPart {
    Key(String),
    Index(usize),
}

pub fn read_projected_json<R: BufRead>(
    reader: R,
    budget: &mut usize,
    select: impl Fn(&[JsonPathPart]) -> bool,
) -> std::result::Result<Option<Value>, TranscriptJsonError> {
    let mut parser = Parser {
        reader,
        budget,
        select,
        path: vec![],
    };
    let result = (|| {
        parser.whitespace()?;
        if !matches!(parser.peek()?, Some(b'{' | b'[')) {
            return Err(TranscriptJsonError::Malformed);
        }
        let value = parser.value(true, 0)?;
        parser.whitespace()?;
        if parser.peek()?.is_some() {
            return Err(TranscriptJsonError::Malformed);
        }
        Ok(value)
    })();
    match result {
        Err(TranscriptJsonError::Malformed) => Ok(None),
        r => r,
    }
}
struct Parser<'a, R, F> {
    reader: R,
    budget: &'a mut usize,
    select: F,
    path: Vec<JsonPathPart>,
}
impl<R: BufRead, F: Fn(&[JsonPathPart]) -> bool> Parser<'_, R, F> {
    fn reserve(&mut self, n: usize) -> std::result::Result<(), TranscriptJsonError> {
        *self.budget = self
            .budget
            .checked_sub(n)
            .ok_or(TranscriptJsonError::Budget)?;
        Ok(())
    }
    fn peek(&mut self) -> std::result::Result<Option<u8>, TranscriptJsonError> {
        Ok(self.reader.fill_buf()?.first().copied())
    }
    fn byte(&mut self) -> std::result::Result<u8, TranscriptJsonError> {
        let b = self.peek()?.ok_or(TranscriptJsonError::Malformed)?;
        self.reader.consume(1);
        Ok(b)
    }
    fn whitespace(&mut self) -> std::result::Result<(), TranscriptJsonError> {
        while self
            .peek()?
            .is_some_and(|b| matches!(b, b' ' | b'\r' | b'\n' | b'\t'))
        {
            self.byte()?;
        }
        Ok(())
    }
    fn expect(&mut self, b: u8) -> std::result::Result<(), TranscriptJsonError> {
        if self.byte()? != b {
            return Err(TranscriptJsonError::Malformed);
        }
        Ok(())
    }
    fn string(&mut self, keep: bool) -> std::result::Result<Option<String>, TranscriptJsonError> {
        self.expect(b'"')?;
        let mut raw = Vec::new();
        if keep {
            self.reserve(64)?;
            raw.push(b'"');
        }
        let mut escaped = false;
        let mut unicode = 0;
        let mut utf8_remaining = 0;
        loop {
            let b = self.byte()?;
            if keep {
                self.reserve(2)?;
                raw.push(b);
            }
            if unicode > 0 {
                if !b.is_ascii_hexdigit() {
                    return Err(TranscriptJsonError::Malformed);
                }
                unicode -= 1;
                continue;
            }
            if escaped {
                if b == b'u' {
                    unicode = 4;
                } else if !matches!(b, b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') {
                    return Err(TranscriptJsonError::Malformed);
                }
                escaped = false;
                continue;
            }
            if utf8_remaining > 0 {
                if !(0x80..=0xbf).contains(&b) {
                    return Err(TranscriptJsonError::Malformed);
                }
                utf8_remaining -= 1;
                continue;
            }
            match b {
                b'"' => break,
                b'\\' => escaped = true,
                0..=31 => return Err(TranscriptJsonError::Malformed),
                0xc2..=0xdf => utf8_remaining = 1,
                0xe0..=0xef => utf8_remaining = 2,
                0xf0..=0xf4 => utf8_remaining = 3,
                0x80..=0xff => return Err(TranscriptJsonError::Malformed),
                _ => {}
            }
        }
        if keep {
            Ok(Some(
                serde_json::from_slice(&raw).map_err(|_| TranscriptJsonError::Malformed)?,
            ))
        } else {
            Ok(None)
        }
    }
    fn value(
        &mut self,
        parent_selected: bool,
        depth: usize,
    ) -> std::result::Result<Option<Value>, TranscriptJsonError> {
        self.whitespace()?;
        let selected = parent_selected && (self.select)(&self.path);
        if selected {
            self.reserve(64)?;
        }
        match self.peek()?.ok_or(TranscriptJsonError::Malformed)? {
            b'{' | b'[' => {
                if depth >= 128 {
                    return Err(TranscriptJsonError::Depth);
                }
                let object = self.byte()? == b'{';
                let close = if object { b'}' } else { b']' };
                let mut map = Map::new();
                let mut array = vec![];
                let mut index = 0;
                self.whitespace()?;
                if self.peek()? == Some(close) {
                    self.byte()?;
                    return Ok(selected.then(|| {
                        if object {
                            Value::Object(map)
                        } else {
                            Value::Array(array)
                        }
                    }));
                }
                loop {
                    let key = if object {
                        self.whitespace()?;
                        let key = self.string(true)?.ok_or(TranscriptJsonError::Malformed)?;
                        self.whitespace()?;
                        self.expect(b':')?;
                        JsonPathPart::Key(key)
                    } else {
                        JsonPathPart::Index(index)
                    };
                    self.path.push(key.clone());
                    let value = self.value(selected, depth + 1)?;
                    self.path.pop();
                    if let Some(value) = value {
                        match key {
                            JsonPathPart::Key(key) => {
                                map.insert(key, value);
                            }
                            JsonPathPart::Index(_) => array.push(value),
                        }
                    }
                    index += 1;
                    self.whitespace()?;
                    match self.byte()? {
                        b if b == close => break,
                        b',' => {}
                        _ => return Err(TranscriptJsonError::Malformed),
                    }
                }
                Ok(selected.then(|| {
                    if object {
                        Value::Object(map)
                    } else {
                        Value::Array(array)
                    }
                }))
            }
            b'"' => Ok(self.string(selected)?.map(Value::String)),
            b't' | b'f' | b'n' => {
                let (literal, value) = match self.peek()? {
                    Some(b't') => (b"true".as_slice(), Value::Bool(true)),
                    Some(b'f') => (b"false".as_slice(), Value::Bool(false)),
                    _ => (b"null".as_slice(), Value::Null),
                };
                for b in literal {
                    self.expect(*b)?;
                }
                Ok(selected.then_some(value))
            }
            b'-' | b'0'..=b'9' => {
                let mut raw = Vec::new();
                while self.peek()?.is_some_and(|b| {
                    b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.' | b'e' | b'E')
                }) {
                    self.reserve(2)?;
                    raw.push(self.byte()?);
                }
                let value: Value =
                    serde_json::from_slice(&raw).map_err(|_| TranscriptJsonError::Malformed)?;
                Ok(selected.then_some(value))
            }
            _ => Err(TranscriptJsonError::Malformed),
        }
    }
}
pub fn select_transcript_path(path: &[JsonPathPart]) -> bool {
    fn names(p: &JsonPathPart) -> Option<&str> {
        if let JsonPathPart::Key(k) = p {
            Some(k.as_str())
        } else {
            None
        }
    }
    let Some(first) = path.first().and_then(names) else {
        return true;
    };
    let root = [
        "type",
        "id",
        "parentId",
        "name",
        "timestamp",
        "cwd",
        "sessionId",
        "aiTitle",
        "customTitle",
        "isSidechain",
        "isMeta",
        "isCompactSummary",
        "message",
        "payload",
    ];
    if !root.contains(&first) {
        return false;
    }
    if path.len() < 2 {
        return true;
    }
    let second = path.get(1).and_then(names).unwrap_or("");
    match first {
        "message" => match second {
            "role" | "model" => path.len() == 2,
            "content" => content_path(&path[2..]),
            _ => false,
        },
        "payload" => match second {
            "history_base" | "internal_chat_message_metadata_passthrough" => true,
            "id" | "session_id" | "type" | "role" | "message" | "model" | "cwd" => path.len() == 2,
            "content" => content_path(&path[2..]),
            _ => false,
        },
        _ => false,
    }
}
fn content_path(path: &[JsonPathPart]) -> bool {
    match path {
        [] => true,
        [JsonPathPart::Index(_)] => true,
        [JsonPathPart::Index(_), JsonPathPart::Key(k)] => k == "type" || k == "text",
        _ => false,
    }
}
