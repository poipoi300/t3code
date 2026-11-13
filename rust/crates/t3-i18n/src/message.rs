//! ICU MessageFormat, ported from `@generaltranslation/icu` (the parser and formatter behind
//! `gt-react`'s `useGT`, version 0.1.2).
//!
//! Supported, with the TypeScript's semantics: literals with `'` quoting, `{name}`, `select`,
//! `plural` and `selectordinal` (with `offset:` and `=N` selectors), `#`, and the parse
//! errors it reports. `{n, number}` and `{n, number, integer}` format a number; the other named
//! and skeleton styles (`percent`, `currency`, `date`, `time`, `::skeleton`) and rich-text tags
//! are parsed but report [`MessageError::Unsupported`] when formatted, which GT treats like any
//! other failed formatting. The product's catalogues use none of them.

use std::collections::BTreeMap;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Error)]
pub enum MessageError {
    #[error("ICU syntax error {code} at byte {offset}")]
    Syntax { code: &'static str, offset: usize },
    #[error("The ICU message variable \"{0}\" was not provided.")]
    MissingVariable(String),
    #[error("The ICU variable \"{name}\" value \"{value}\" did not match any of: {options}.")]
    NoMatchingOption {
        name: String,
        value: String,
        options: String,
    },
    #[error("unsupported ICU feature: {0}")]
    Unsupported(&'static str),
}

/// A message variable. `gt("…", { name })` passes strings and numbers.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Text(String),
    Int(i64),
    Float(f64),
}

impl Value {
    /// JavaScript's `String(value)`.
    fn display(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Int(n) => n.to_string(),
            Self::Float(n) => js_number_to_string(*n),
        }
    }

    /// JavaScript's `Number(value)`.
    fn number(&self) -> f64 {
        match self {
            Self::Text(text) => {
                let trimmed = text.trim_matches(crate::direction::is_js_space);
                if trimmed.is_empty() {
                    0.0
                } else {
                    trimmed.parse().unwrap_or(f64::NAN)
                }
            }
            Self::Int(n) => *n as f64,
            Self::Float(n) => *n,
        }
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}
impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}
impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}
impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Self::Int(i64::from(value))
    }
}
impl From<u32> for Value {
    fn from(value: u32) -> Self {
        Self::Int(i64::from(value))
    }
}
impl From<usize> for Value {
    fn from(value: usize) -> Self {
        Self::Int(value as i64)
    }
}
impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

/// `String(number)` for the ranges a UI shows: integers print without a fraction, and the
/// exponent forms JavaScript uses below 1e-6 and from 1e21 are not reproduced.
fn js_number_to_string(n: f64) -> String {
    if n.is_nan() {
        "NaN".into()
    } else if n.is_infinite() {
        if n > 0.0 { "Infinity" } else { "-Infinity" }.into()
    } else if n == 0.0 {
        "0".into()
    } else {
        format!("{n}")
    }
}

pub type Args<'a> = [(&'a str, Value)];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluralKind {
    Cardinal,
    Ordinal,
}

/// The locale-dependent parts of formatting, supplied by the caller.
pub trait Runtime {
    /// The CLDR plural category (`zero`, `one`, `two`, `few`, `many`, `other`).
    fn plural_category(&self, kind: PluralKind, number: f64) -> &'static str;
    /// `new Intl.NumberFormat(locale).format(number)`.
    fn format_number(&self, number: f64) -> String;
    /// `new Intl.NumberFormat(locale, { maximumFractionDigits: 0 }).format(number)`.
    fn format_integer(&self, number: f64) -> String;
}

#[derive(Debug, Clone, PartialEq)]
enum Element {
    Literal(String),
    Argument(String),
    Number {
        name: String,
        integer: bool,
        unsupported: bool,
    },
    DateTime(String),
    Select {
        name: String,
        options: Options,
    },
    Plural {
        name: String,
        options: Options,
        offset: i64,
        kind: PluralKind,
    },
    Pound,
    Tag(String),
}

type Options = Vec<(String, Vec<Element>)>;

/// A parsed message.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    elements: Vec<Element>,
}

/// `\t-\r`, space, U+0085, U+200E/F, U+2028/9.
fn is_grammar_space(c: char) -> bool {
    matches!(
        c,
        '\t'..='\r' | ' ' | '\u{85}' | '\u{200E}' | '\u{200F}' | '\u{2028}' | '\u{2029}'
    )
}

fn is_identifier_boundary(c: char) -> bool {
    matches!(
        c,
        '\t'..='\r'
            | ' '
            | '\u{85}'
            | '\u{A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{21}'..='\u{2F}'
            | '\u{3A}'..='\u{40}'
            | '\u{5B}'..='\u{5E}'
            | '\u{60}'
            | '\u{7B}'..='\u{7E}'
            | '\u{A1}'..='\u{A7}'
            | '\u{A9}'
            | '\u{AB}'
            | '\u{AC}'
            | '\u{AE}'
            | '\u{B0}'
            | '\u{B1}'
            | '\u{B6}'
            | '\u{BB}'
            | '\u{BF}'
            | '\u{D7}'
            | '\u{F7}'
            | '\u{2010}'..='\u{2027}'
            | '\u{2030}'..='\u{203E}'
            | '\u{2041}'..='\u{2053}'
            | '\u{2055}'..='\u{205E}'
            | '\u{2190}'..='\u{245F}'
            | '\u{2500}'..='\u{2775}'
            | '\u{2794}'..='\u{2BFF}'
            | '\u{2E00}'..='\u{2E7F}'
            | '\u{3001}'..='\u{3003}'
            | '\u{3008}'..='\u{3020}'
            | '\u{3030}'
            | '\u{FD3E}'..='\u{FD3F}'
            | '\u{FE45}'..='\u{FE46}'
    )
}

fn is_tag_name_char(c: char) -> bool {
    matches!(
        c,
        '-' | '.' | '0'..='9' | '_' | 'A'..='Z' | 'a'..='z'
            | '\u{B7}'
            | '\u{C0}'..='\u{D6}'
            | '\u{D8}'..='\u{F6}'
            | '\u{F8}'..='\u{37D}'
            | '\u{37F}'..='\u{1FFF}'
            | '\u{200C}'..='\u{200D}'
            | '\u{203F}'..='\u{2040}'
            | '\u{2070}'..='\u{218F}'
            | '\u{2C00}'..='\u{2FEF}'
            | '\u{3001}'..='\u{D7FF}'
            | '\u{F900}'..='\u{FDCF}'
            | '\u{FDF0}'..='\u{FFFD}'
            | '\u{10000}'..='\u{EFFFF}'
    )
}

struct Parser<'a> {
    message: &'a str,
    index: usize,
}

type ParseResult<T> = Result<T, MessageError>;

impl<'a> Parser<'a> {
    fn fail<T>(&self, code: &'static str, offset: usize) -> ParseResult<T> {
        Err(MessageError::Syntax { code, offset })
    }

    fn at_end(&self) -> bool {
        self.index >= self.message.len()
    }

    fn current(&self) -> Option<char> {
        self.message[self.index..].chars().next()
    }

    fn peek(&self) -> Option<char> {
        let mut chars = self.message[self.index..].chars();
        chars.next()?;
        chars.next()
    }

    fn advance(&mut self, c: char) {
        self.index += c.len_utf8();
    }

    fn consume(&mut self, value: &str) -> bool {
        if self.message[self.index..].starts_with(value) {
            self.index += value.len();
            true
        } else {
            false
        }
    }

    fn skip_space(&mut self) {
        while let Some(c) = self.current() {
            if !is_grammar_space(c) {
                break;
            }
            self.advance(c);
        }
    }

    fn read_identifier(&mut self) -> &'a str {
        let start = self.index;
        while let Some(c) = self.current() {
            if is_identifier_boundary(c) {
                break;
            }
            self.advance(c);
        }
        &self.message[start..self.index]
    }

    fn parse_message(
        &mut self,
        nested: bool,
        in_plural: bool,
        expecting_close_tag: bool,
    ) -> ParseResult<Vec<Element>> {
        let mut elements = Vec::new();
        while let Some(c) = self.current() {
            if c == '{' {
                elements.push(self.parse_argument(expecting_close_tag)?);
            } else if c == '}' && nested {
                break;
            } else if c == '#' && in_plural {
                self.index += 1;
                elements.push(Element::Pound);
            } else if c == '<' && self.peek() == Some('/') {
                if expecting_close_tag {
                    break;
                }
                return self.fail("UNMATCHED_CLOSING_TAG", self.index);
            } else if c == '<' && self.peek().is_some_and(|next| next.is_ascii_alphabetic()) {
                elements.push(self.parse_tag(in_plural)?);
            } else {
                elements.push(self.parse_literal(nested, in_plural));
            }
        }
        Ok(elements)
    }

    fn parse_literal(&mut self, nested: bool, in_plural: bool) -> Element {
        let mut value = String::new();
        while let Some(c) = self.current() {
            if let Some(quoted) = self.try_parse_quote(in_plural) {
                value.push_str(&quoted);
                continue;
            }
            let next = self.peek();
            if c == '{'
                || (c == '}' && nested)
                || (c == '#' && in_plural)
                || (c == '<'
                    && (next.is_some_and(|n| n.is_ascii_alphabetic()) || next == Some('/')))
            {
                break;
            }
            value.push(c);
            self.advance(c);
        }
        Element::Literal(value)
    }

    fn try_parse_quote(&mut self, in_plural: bool) -> Option<String> {
        if self.current() != Some('\'') {
            return None;
        }
        let next = self.peek();
        if next == Some('\'') {
            self.index += 2;
            return Some("'".into());
        }
        let quotes_next =
            matches!(next, Some('{' | '}' | '<' | '>')) || (next == Some('#') && in_plural);
        if !quotes_next {
            return None;
        }
        self.index += 1;
        let mut value = String::new();
        while let Some(c) = self.current() {
            if c == '\'' {
                if self.peek() == Some('\'') {
                    value.push('\'');
                    self.index += 2;
                    continue;
                }
                self.index += 1;
                break;
            }
            value.push(c);
            self.advance(c);
        }
        Some(value)
    }

    fn read_tag_name(&mut self) -> ParseResult<&'a str> {
        let start = self.index;
        match self.current() {
            Some(c) if c.is_ascii_alphabetic() => self.advance(c),
            _ => return self.fail("INVALID_TAG", start),
        }
        while let Some(c) = self.current() {
            if !is_tag_name_char(c) {
                break;
            }
            self.advance(c);
        }
        Ok(&self.message[start..self.index])
    }

    fn parse_tag(&mut self, in_plural: bool) -> ParseResult<Element> {
        let start = self.index;
        self.index += 1;
        let name = self.read_tag_name()?;
        self.skip_space();
        if self.consume("/>") {
            return Ok(Element::Literal(format!("<{name}/>")));
        }
        if !self.consume(">") {
            return self.fail("INVALID_TAG", start);
        }
        self.parse_message(true, in_plural, true)?;
        let closing_start = self.index;
        if !self.consume("</") {
            return self.fail("UNCLOSED_TAG", start);
        }
        let closing_name_start = self.index;
        if !self.current().is_some_and(|c| c.is_ascii_alphabetic()) {
            return self.fail("INVALID_TAG", closing_start);
        }
        if self.read_tag_name()? != name {
            return self.fail("UNMATCHED_CLOSING_TAG", closing_name_start);
        }
        self.skip_space();
        if !self.consume(">") {
            return self.fail("INVALID_TAG", closing_start);
        }
        Ok(Element::Tag(name.to_owned()))
    }

    fn parse_argument(&mut self, expecting_close_tag: bool) -> ParseResult<Element> {
        let start = self.index;
        self.index += 1;
        self.skip_space();
        if self.at_end() {
            return self.fail("EXPECT_ARGUMENT_CLOSING_BRACE", start);
        }
        if self.current() == Some('}') {
            self.index += 1;
            return self.fail("EMPTY_ARGUMENT", start);
        }
        let name = self.read_identifier();
        if name.is_empty() {
            return self.fail("MALFORMED_ARGUMENT", start);
        }
        self.skip_space();
        if self.consume("}") {
            return Ok(Element::Argument(name.to_owned()));
        }
        if self.at_end() {
            return self.fail("EXPECT_ARGUMENT_CLOSING_BRACE", start);
        }
        if !self.consume(",") {
            return self.fail("MALFORMED_ARGUMENT", start);
        }
        self.skip_space();
        if self.at_end() {
            return self.fail("EXPECT_ARGUMENT_CLOSING_BRACE", start);
        }
        let type_start = self.index;
        let argument_type = self.read_identifier();
        let type_end = self.index;
        if argument_type.is_empty() {
            return self.fail("EXPECT_ARGUMENT_TYPE", type_start);
        }
        match argument_type {
            "number" | "date" | "time" => self.parse_simple_argument(start, name, argument_type),
            "plural" | "selectordinal" | "select" => self.parse_choice_argument(
                start,
                name,
                argument_type,
                type_end,
                expecting_close_tag,
            ),
            _ => self.fail("INVALID_ARGUMENT_TYPE", type_start),
        }
    }

    fn parse_simple_argument(
        &mut self,
        start: usize,
        name: &str,
        argument_type: &str,
    ) -> ParseResult<Element> {
        self.skip_space();
        let mut style: Option<String> = None;
        if self.consume(",") {
            self.skip_space();
            let raw = self.read_simple_style()?;
            let raw = raw.trim_end_matches(|c: char| crate::direction::is_js_space(c));
            if raw.is_empty() {
                return self.fail("EXPECT_ARGUMENT_STYLE", self.index);
            }
            style = Some(raw.to_owned());
        }
        if !self.consume("}") {
            return self.fail("EXPECT_ARGUMENT_CLOSING_BRACE", start);
        }
        if argument_type != "number" {
            return Ok(Element::DateTime(name.to_owned()));
        }
        let (integer, unsupported) = match style.as_deref() {
            None => (false, false),
            Some("integer") => (true, false),
            // `NUMBER_STYLES[style] ?? {}`: an unknown named style is the default format.
            Some(s) if s.starts_with("::") || s == "currency" || s == "percent" => (false, true),
            Some(_) => (false, false),
        };
        Ok(Element::Number {
            name: name.to_owned(),
            integer,
            unsupported,
        })
    }

    fn read_simple_style(&mut self) -> ParseResult<&'a str> {
        let start = self.index;
        while let Some(c) = self.current() {
            if c == '\'' {
                self.index += 1;
                let quoted_start = self.index;
                while let Some(q) = self.current() {
                    if q == '\'' {
                        break;
                    }
                    self.advance(q);
                }
                if self.at_end() {
                    return self.fail("UNCLOSED_QUOTE_IN_ARGUMENT_STYLE", quoted_start);
                }
                self.index += 1;
            } else if c == '}' {
                break;
            } else {
                self.advance(c);
            }
        }
        Ok(&self.message[start..self.index])
    }

    fn read_integer_token(
        &mut self,
        expected: &'static str,
        invalid: &'static str,
    ) -> ParseResult<i64> {
        let start = self.index;
        if matches!(self.current(), Some('+' | '-')) {
            self.index += 1;
        }
        let digits_start = self.index;
        while self.current().is_some_and(|c| c.is_ascii_digit()) {
            self.index += 1;
        }
        if digits_start == self.index {
            return self.fail(expected, start);
        }
        match self.message[start..self.index]
            .trim_start_matches('+')
            .parse::<i64>()
        {
            Ok(n) if n.unsigned_abs() <= 9_007_199_254_740_991 => Ok(n),
            _ => self.fail(invalid, start),
        }
    }

    fn parse_choice_argument(
        &mut self,
        start: usize,
        name: &str,
        argument_type: &str,
        type_end: usize,
        expecting_close_tag: bool,
    ) -> ParseResult<Element> {
        let is_select = argument_type == "select";
        self.skip_space();
        if !self.consume(",") {
            return self.fail("EXPECT_SELECT_ARGUMENT_OPTIONS", type_end);
        }
        self.skip_space();
        let mut offset = 0;
        let mut selector = self.read_identifier().to_owned();
        if !is_select && selector == "offset" {
            if !self.consume(":") {
                return self.fail("EXPECT_PLURAL_ARGUMENT_OFFSET_VALUE", self.index);
            }
            self.skip_space();
            offset = self.read_integer_token(
                "EXPECT_PLURAL_ARGUMENT_OFFSET_VALUE",
                "INVALID_PLURAL_ARGUMENT_OFFSET_VALUE",
            )?;
            self.skip_space();
            selector = self.read_identifier().to_owned();
        }
        let mut options: Options = Vec::new();
        let mut selector_start = self.index;
        while !selector.is_empty() || (!is_select && self.current() == Some('=')) {
            if selector.is_empty() && self.consume("=") {
                let n = self.read_integer_token(
                    "EXPECT_PLURAL_ARGUMENT_SELECTOR",
                    "INVALID_PLURAL_ARGUMENT_SELECTOR",
                )?;
                selector = format!("={n}");
            }
            if options.iter().any(|(existing, _)| *existing == selector) {
                return self.fail(
                    if is_select {
                        "DUPLICATE_SELECT_ARGUMENT_SELECTOR"
                    } else {
                        "DUPLICATE_PLURAL_ARGUMENT_SELECTOR"
                    },
                    selector_start,
                );
            }
            self.skip_space();
            let option_start = self.index;
            if !self.consume("{") {
                return self.fail(
                    if is_select {
                        "EXPECT_SELECT_ARGUMENT_SELECTOR_FRAGMENT"
                    } else {
                        "EXPECT_PLURAL_ARGUMENT_SELECTOR_FRAGMENT"
                    },
                    self.index,
                );
            }
            let value = self.parse_message(true, !is_select, expecting_close_tag)?;
            if !self.consume("}") {
                return self.fail("EXPECT_ARGUMENT_CLOSING_BRACE", option_start);
            }
            options.push((std::mem::take(&mut selector), value));
            self.skip_space();
            selector_start = self.index;
            selector = self.read_identifier().to_owned();
        }
        if options.is_empty() {
            return self.fail(
                if is_select {
                    "EXPECT_SELECT_ARGUMENT_SELECTOR"
                } else {
                    "EXPECT_PLURAL_ARGUMENT_SELECTOR"
                },
                self.index,
            );
        }
        if !options.iter().any(|(selector, _)| selector == "other") {
            return self.fail("MISSING_OTHER_CLAUSE", selector_start);
        }
        if !self.consume("}") {
            return self.fail("EXPECT_ARGUMENT_CLOSING_BRACE", start);
        }
        let name = name.to_owned();
        Ok(if is_select {
            Element::Select { name, options }
        } else {
            Element::Plural {
                name,
                options,
                offset,
                kind: if argument_type == "plural" {
                    PluralKind::Cardinal
                } else {
                    PluralKind::Ordinal
                },
            }
        })
    }
}

impl Message {
    pub fn parse(message: &str) -> Result<Self, MessageError> {
        let mut parser = Parser { message, index: 0 };
        let elements = parser.parse_message(false, false, false)?;
        Ok(Self { elements })
    }

    /// Every variable the message reads, including those inside `select` and `plural` branches.
    pub fn argument_names(&self) -> std::collections::BTreeSet<String> {
        fn collect(elements: &[Element], names: &mut std::collections::BTreeSet<String>) {
            for element in elements {
                match element {
                    Element::Argument(name)
                    | Element::DateTime(name)
                    | Element::Number { name, .. } => {
                        names.insert(name.clone());
                    }
                    Element::Select { name, options } | Element::Plural { name, options, .. } => {
                        names.insert(name.clone());
                        for (_, branch) in options {
                            collect(branch, names);
                        }
                    }
                    Element::Literal(_) | Element::Pound | Element::Tag(_) => {}
                }
            }
        }
        let mut names = std::collections::BTreeSet::new();
        collect(&self.elements, &mut names);
        names
    }

    /// Format with `runtime` for the locale and `args` for variables.
    pub fn format(&self, runtime: &dyn Runtime, args: &Args<'_>) -> Result<String, MessageError> {
        let variables: BTreeMap<&str, &Value> =
            args.iter().map(|(name, value)| (*name, value)).collect();
        let mut out = String::new();
        format_elements(&self.elements, runtime, &variables, None, &mut out)?;
        Ok(out)
    }
}

fn variable<'a>(
    variables: &BTreeMap<&str, &'a Value>,
    name: &str,
) -> Result<&'a Value, MessageError> {
    variables
        .get(name)
        .copied()
        .ok_or_else(|| MessageError::MissingVariable(name.to_owned()))
}

fn option_for<'a>(options: &'a Options, selector: &str) -> Option<&'a [Element]> {
    options
        .iter()
        .find(|(candidate, _)| candidate == selector)
        .map(|(_, elements)| elements.as_slice())
}

fn format_elements(
    elements: &[Element],
    runtime: &dyn Runtime,
    variables: &BTreeMap<&str, &Value>,
    plural_value: Option<f64>,
    out: &mut String,
) -> Result<(), MessageError> {
    for element in elements {
        match element {
            Element::Literal(text) => out.push_str(text),
            Element::Pound => {
                if let Some(value) = plural_value {
                    out.push_str(&runtime.format_number(value));
                }
            }
            Element::Argument(name) => out.push_str(&variable(variables, name)?.display()),
            Element::Number {
                name,
                integer,
                unsupported,
            } => {
                let value = variable(variables, name)?.number();
                if *unsupported {
                    return Err(MessageError::Unsupported("number style"));
                }
                out.push_str(&if *integer {
                    runtime.format_integer(value)
                } else {
                    runtime.format_number(value)
                });
            }
            Element::DateTime(name) => {
                variable(variables, name)?;
                return Err(MessageError::Unsupported("date and time arguments"));
            }
            Element::Select { name, options } => {
                let value = variable(variables, name)?.display();
                let chosen = option_for(options, &value)
                    .or_else(|| option_for(options, "other"))
                    .ok_or_else(|| no_match(name, &value, options))?;
                format_elements(chosen, runtime, variables, None, out)?;
            }
            Element::Plural {
                name,
                options,
                offset,
                kind,
            } => {
                let raw = variable(variables, name)?;
                let mut chosen = option_for(options, &format!("={}", raw.display()));
                let adjusted = raw.number() - *offset as f64;
                let has_category = options
                    .iter()
                    .any(|(selector, _)| selector != "other" && !selector.starts_with('='));
                if chosen.is_none() && has_category {
                    chosen = option_for(options, runtime.plural_category(*kind, adjusted));
                }
                let chosen = chosen
                    .or_else(|| option_for(options, "other"))
                    .ok_or_else(|| no_match(name, &raw.display(), options))?;
                format_elements(chosen, runtime, variables, Some(adjusted), out)?;
            }
            Element::Tag(_) => return Err(MessageError::Unsupported("rich-text tags")),
        }
    }
    Ok(())
}

fn no_match(name: &str, value: &str, options: &Options) -> MessageError {
    MessageError::NoMatchingOption {
        name: name.to_owned(),
        value: value.to_owned(),
        options: options
            .iter()
            .map(|(selector, _)| selector.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct English;
    impl Runtime for English {
        fn plural_category(&self, kind: PluralKind, n: f64) -> &'static str {
            match (kind, n) {
                (PluralKind::Cardinal, 1.0) => "one",
                (PluralKind::Ordinal, n) if n % 10.0 == 1.0 && n % 100.0 != 11.0 => "one",
                (PluralKind::Ordinal, n) if n % 10.0 == 2.0 && n % 100.0 != 12.0 => "two",
                (PluralKind::Ordinal, n) if n % 10.0 == 3.0 && n % 100.0 != 13.0 => "few",
                _ => "other",
            }
        }
        fn format_number(&self, n: f64) -> String {
            format!("<{n}>")
        }
        fn format_integer(&self, n: f64) -> String {
            format!("<{}>", n.round())
        }
    }

    fn run(message: &str, args: &[(&str, Value)]) -> Result<String, MessageError> {
        Message::parse(message)?.format(&English, args)
    }

    #[test]
    fn interpolates_arguments() {
        assert_eq!(
            run("No settings match “{query}”.", &[("query", "abc".into())]).as_deref(),
            Ok("No settings match “abc”.")
        );
        assert_eq!(run("{ n }!", &[("n", 3.into())]).as_deref(), Ok("3!"));
        assert_eq!(run("{n}", &[("n", 1.5.into())]).as_deref(), Ok("1.5"));
        assert_eq!(run("{n}", &[("n", 2.0.into())]).as_deref(), Ok("2"));
    }

    #[test]
    fn apostrophes_quote_only_before_syntax_characters() {
        assert_eq!(
            run("Notifications dans l'app", &[]).as_deref(),
            Ok("Notifications dans l'app")
        );
        assert_eq!(run("it''s", &[]).as_deref(), Ok("it's"));
        assert_eq!(run("'{'literal'}'", &[]).as_deref(), Ok("{literal}"));
        assert_eq!(
            run("'{n}' and {n}", &[("n", 1.into())]).as_deref(),
            Ok("{n} and 1")
        );
        assert_eq!(run("a '<b>' c", &[]).as_deref(), Ok("a <b> c"));
        assert_eq!(run("#", &[]).as_deref(), Ok("#"));
    }

    #[test]
    fn selects_by_value_with_other_fallback() {
        let message = "{g, select, female {She} male {He} other {They}} left";
        assert_eq!(
            run(message, &[("g", "female".into())]).as_deref(),
            Ok("She left")
        );
        assert_eq!(
            run(message, &[("g", "x".into())]).as_deref(),
            Ok("They left")
        );
    }

    #[test]
    fn plurals_use_exact_selectors_categories_offset_and_pound() {
        let message = "{n, plural, =0 {none} one {# file} other {# files}}";
        assert_eq!(run(message, &[("n", 0.into())]).as_deref(), Ok("none"));
        assert_eq!(run(message, &[("n", 1.into())]).as_deref(), Ok("<1> file"));
        assert_eq!(run(message, &[("n", 5.into())]).as_deref(), Ok("<5> files"));
        let offset = "{n, plural, offset:1 =0 {nobody} =1 {you} one {you and # other} other {you and # others}}";
        assert_eq!(
            run(offset, &[("n", 2.into())]).as_deref(),
            Ok("you and <1> other")
        );
        assert_eq!(
            run(offset, &[("n", 4.into())]).as_deref(),
            Ok("you and <3> others")
        );
        assert_eq!(
            run("{n, plural, other {x}}", &[("n", "abc".into())]).as_deref(),
            Ok("x")
        );
        let ordinal = "{n, selectordinal, one {#st} two {#nd} few {#rd} other {#th}}";
        assert_eq!(run(ordinal, &[("n", 22.into())]).as_deref(), Ok("<22>nd"));
        assert_eq!(run(ordinal, &[("n", 11.into())]).as_deref(), Ok("<11>th"));
    }

    #[test]
    fn pound_outside_a_plural_branch_is_literal() {
        assert_eq!(
            run("{g, select, other {# stays}}", &[("g", "a".into())]).as_deref(),
            Ok("# stays")
        );
    }

    #[test]
    fn number_arguments() {
        assert_eq!(run("{n, number}", &[("n", 3.into())]).as_deref(), Ok("<3>"));
        assert_eq!(
            run("{n, number, integer}", &[("n", 2.6.into())]).as_deref(),
            Ok("<3>")
        );
        assert!(matches!(
            run("{n, number, percent}", &[("n", 3.into())]),
            Err(MessageError::Unsupported(_))
        ));
        assert!(matches!(
            run("{d, date, short}", &[("d", 3.into())]),
            Err(MessageError::Unsupported(_))
        ));
    }

    #[test]
    fn reports_missing_variables_and_syntax_errors() {
        assert_eq!(
            run("{x}", &[]),
            Err(MessageError::MissingVariable("x".into()))
        );
        for (message, code) in [
            ("{", "EXPECT_ARGUMENT_CLOSING_BRACE"),
            ("{}", "EMPTY_ARGUMENT"),
            ("{a b}", "MALFORMED_ARGUMENT"),
            ("{a, wat}", "INVALID_ARGUMENT_TYPE"),
            ("{a, select, x {y}}", "MISSING_OTHER_CLAUSE"),
            (
                "{a, select, other {y} other {z}}",
                "DUPLICATE_SELECT_ARGUMENT_SELECTOR",
            ),
            ("{a, plural, =x {y}}", "EXPECT_PLURAL_ARGUMENT_SELECTOR"),
            ("</b>", "UNMATCHED_CLOSING_TAG"),
            ("<b>text", "UNCLOSED_TAG"),
        ] {
            match Message::parse(message) {
                Err(MessageError::Syntax { code: actual, .. }) => {
                    assert_eq!(actual, code, "{message}")
                }
                other => panic!("{message}: {other:?}"),
            }
        }
    }

    #[test]
    fn tags_parse_but_do_not_format() {
        assert!(matches!(
            run("<b>bold</b>", &[]),
            Err(MessageError::Unsupported(_))
        ));
        assert_eq!(run("a <br/> b", &[]).as_deref(), Ok("a <br/> b"));
    }
}
