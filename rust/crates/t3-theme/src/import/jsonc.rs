//! JSON with comments and trailing commas, as VS Code theme files are written.
//!
//! Strict otherwise: any other deviation is an error rather than something to
//! recover from, because a half-read theme would silently import wrong colours.

use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsoncError;

const MAX_NESTING: usize = 128;

struct Parser<'a> {
    text: &'a str,
    pos: usize,
}

/// Parses one JSONC document.
pub fn parse_jsonc(text: &str) -> Result<Value, JsoncError> {
    let mut parser = Parser { text, pos: 0 };
    parser.skip_trivia()?;
    let value = parser.value(0)?;
    parser.skip_trivia()?;
    if parser.pos != text.len() {
        return Err(JsoncError);
    }
    Ok(value)
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn skip_trivia(&mut self) -> Result<(), JsoncError> {
        loop {
            let rest = &self.text[self.pos..];
            if let Some(c) = rest.chars().next()
                && (c.is_whitespace() || c == '\u{feff}')
            {
                self.pos += c.len_utf8();
            } else if rest.starts_with("//") {
                self.pos += rest.find(['\n', '\r']).unwrap_or(rest.len());
            } else if let Some(comment) = rest.strip_prefix("/*") {
                match comment.find("*/") {
                    Some(end) => self.pos += end + 4,
                    None => return Err(JsoncError),
                }
            } else {
                return Ok(());
            }
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, JsoncError> {
        match self.peek().ok_or(JsoncError)? {
            '{' | '[' if depth >= MAX_NESTING => Err(JsoncError),
            '{' => self.object(depth + 1),
            '[' => self.array(depth + 1),
            '"' => self.string().map(Value::String),
            't' => self.literal("true", Value::Bool(true)),
            'f' => self.literal("false", Value::Bool(false)),
            'n' => self.literal("null", Value::Null),
            '-' | '0'..='9' => self.number(),
            _ => Err(JsoncError),
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, JsoncError> {
        if self.text[self.pos..].starts_with(word) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(JsoncError)
        }
    }

    fn number(&mut self) -> Result<Value, JsoncError> {
        let start = self.pos;
        while matches!(self.peek(), Some('-' | '+' | '.' | 'e' | 'E' | '0'..='9')) {
            self.pos += 1;
        }
        serde_json::from_str::<Value>(&self.text[start..self.pos])
            .ok()
            .filter(Value::is_number)
            .ok_or(JsoncError)
    }

    fn string(&mut self) -> Result<String, JsoncError> {
        let start = self.pos;
        self.bump();
        loop {
            match self.bump().ok_or(JsoncError)? {
                '"' => break,
                '\\' => {
                    self.bump().ok_or(JsoncError)?;
                }
                c if (c as u32) < 0x20 => return Err(JsoncError),
                _ => {}
            }
        }
        serde_json::from_str::<String>(&self.text[start..self.pos]).map_err(|_| JsoncError)
    }

    fn array(&mut self, depth: usize) -> Result<Value, JsoncError> {
        self.bump();
        let mut items = Vec::new();
        loop {
            self.skip_trivia()?;
            if self.peek() == Some(']') {
                self.bump();
                return Ok(Value::Array(items));
            }
            items.push(self.value(depth)?);
            self.skip_trivia()?;
            match self.bump() {
                Some(',') => {}
                Some(']') => return Ok(Value::Array(items)),
                _ => return Err(JsoncError),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, JsoncError> {
        self.bump();
        let mut map = Map::new();
        loop {
            self.skip_trivia()?;
            if self.peek() == Some('}') {
                self.bump();
                return Ok(Value::Object(map));
            }
            if self.peek() != Some('"') {
                return Err(JsoncError);
            }
            let key = self.string()?;
            self.skip_trivia()?;
            if self.bump() != Some(':') {
                return Err(JsoncError);
            }
            self.skip_trivia()?;
            let value = self.value(depth)?;
            map.insert(key, value);
            self.skip_trivia()?;
            match self.bump() {
                Some(',') => {}
                Some('}') => return Ok(Value::Object(map)),
                _ => return Err(JsoncError),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_comments_and_trailing_commas() {
        let value = parse_jsonc("{ // c\n \"a\": [1, 2,], /* b */ \"b\": null, }").expect("parses");
        assert_eq!(value["a"][1], 2);
        assert!(value["b"].is_null());
    }

    #[test]
    fn rejects_excessive_nesting_as_invalid_jsonc() {
        let nested = |depth| format!("{}null{}", "[".repeat(depth), "]".repeat(depth));
        assert!(parse_jsonc(&nested(128)).is_ok());
        assert_eq!(parse_jsonc(&nested(129)), Err(JsoncError));
        let file = "[".repeat(256 * 1024);
        assert_eq!(parse_jsonc(&file), Err(JsoncError));
        let error = crate::import::parse_theme_import(&file).expect_err("invalid theme");
        assert_eq!(error.to_string(), "That theme file is not valid JSON.");
        let object = format!("{}null{}", r#"{"x":"#.repeat(129), "}".repeat(129));
        assert_eq!(parse_jsonc(&object), Err(JsoncError));
    }

    #[test]
    fn rejects_everything_else() {
        for bad in [
            "",
            "{",
            "{\"a\" 1}",
            "{} /* open",
            "{} extra",
            "[1 2]",
            "{\"a\":}",
            "'x'",
        ] {
            assert!(parse_jsonc(bad).is_err(), "{bad:?}");
        }
    }
}
