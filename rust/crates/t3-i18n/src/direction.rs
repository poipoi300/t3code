//! Right-to-left detection, ported from `packages/shared/src/textDirection.ts`.
//!
//! The TypeScript uses look-around regexes (`SENTENCE_END`, `MARKDOWN_CODE_OR_URL`) that
//! Rust's `regex` cannot express, so the scanners below reproduce their match semantics by hand,
//! including JavaScript's `\s` and `\b` (ASCII word characters) definitions.

use unicode_general_category::{GeneralCategory, get_general_category};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextDirection {
    Ltr,
    Rtl,
}

impl TextDirection {
    /// The value of the HTML `dir` attribute.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ltr => "ltr",
            Self::Rtl => "rtl",
        }
    }
}

/// Hebrew through Arabic Extended-A, their presentation forms, and the supplementary
/// right-to-left blocks.
fn is_rtl_char(c: char) -> bool {
    matches!(
        c,
        '\u{0590}'..='\u{08FF}'
            | '\u{FB1D}'..='\u{FDFF}'
            | '\u{FE70}'..='\u{FEFF}'
            | '\u{10800}'..='\u{10FFF}'
            | '\u{1E800}'..='\u{1EFFF}'
    )
}

/// `\p{L}`.
fn is_letter(c: char) -> bool {
    matches!(
        get_general_category(c),
        GeneralCategory::UppercaseLetter
            | GeneralCategory::LowercaseLetter
            | GeneralCategory::TitlecaseLetter
            | GeneralCategory::ModifierLetter
            | GeneralCategory::OtherLetter
    )
}

/// JavaScript's `\s`: Rust's `char::is_whitespace` differs (it includes U+0085 and omits U+FEFF).
pub(crate) fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{000B}' | '\u{000C}' | '\r' | ' ' | '\u{00A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// JavaScript's `\w` without the `u` flag.
fn is_js_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `majority` from the TypeScript: ties go to `tie` when given, else to the first direction seen.
fn majority(
    units: impl IntoIterator<Item = Option<TextDirection>>,
    tie: Option<&dyn Fn() -> Option<TextDirection>>,
) -> Option<TextDirection> {
    let mut rtl = 0usize;
    let mut ltr = 0usize;
    let mut first = None;
    for direction in units.into_iter().flatten() {
        first.get_or_insert(direction);
        match direction {
            TextDirection::Rtl => rtl += 1,
            TextDirection::Ltr => ltr += 1,
        }
    }
    if rtl == ltr {
        return tie.map_or(first, |tie| tie());
    }
    Some(if rtl > ltr {
        TextDirection::Rtl
    } else {
        TextDirection::Ltr
    })
}

fn word_direction(word: &str) -> Option<TextDirection> {
    let letter = word.chars().find(|c| is_letter(*c))?;
    Some(if is_rtl_char(letter) {
        TextDirection::Rtl
    } else {
        TextDirection::Ltr
    })
}

fn words_direction(text: &str) -> Option<TextDirection> {
    majority(text.split(is_js_space).map(word_direction), None)
}

/// Whether the text before byte offset `end` ends with `e.g.` or `i.e.` that starts on a
/// `\b` boundary, case-insensitively.
fn ends_with_abbreviation(text: &str, end: usize) -> bool {
    let before = &text[..end];
    let bytes = before.as_bytes();
    if bytes.len() < 4 {
        return false;
    }
    let tail = &bytes[bytes.len() - 4..];
    let lower = [
        tail[0].to_ascii_lowercase(),
        tail[1],
        tail[2].to_ascii_lowercase(),
        tail[3],
    ];
    if lower != *b"e.g." && lower != *b"i.e." {
        return false;
    }
    match before[..before.len() - 4].chars().next_back() {
        Some(previous) => !is_js_word(previous),
        None => true,
    }
}

/// `text.split(/(?<=[.!?؟])(?<!\b(?:e\.g|i\.e)\.)\s+|\n+/i)`.
fn split_sentences(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut previous: Option<char> = None;
    let mut iter = text.char_indices().peekable();
    while let Some((index, c)) = iter.next() {
        let sentence_break = previous.is_some_and(|p| matches!(p, '.' | '!' | '?' | '\u{061F}'))
            && is_js_space(c)
            && !ends_with_abbreviation(text, index);
        let end = if sentence_break {
            Some(run_end(text, index, is_js_space))
        } else if c == '\n' {
            Some(run_end(text, index, |c| c == '\n'))
        } else {
            None
        };
        match end {
            Some(end) => {
                parts.push(&text[start..index]);
                start = end;
                previous = text[..end].chars().next_back();
                while iter.peek().is_some_and(|(next, _)| *next < end) {
                    iter.next();
                }
            }
            None => previous = Some(c),
        }
    }
    parts.push(&text[start..]);
    parts
}

fn run_end(text: &str, from: usize, keep: impl Fn(char) -> bool) -> usize {
    text[from..]
        .char_indices()
        .find(|(_, c)| !keep(*c))
        .map_or(text.len(), |(offset, _)| from + offset)
}

/// `dir="auto"` misreads Arabic prose that opens with an English term. Each sentence follows its
/// main language by word count, and the text follows most of its sentences, so one long English
/// sentence cannot outvote the Arabic ones around it. Sentence ties use the whole-text word
/// majority, then the first word.
pub fn text_direction(text: &str) -> Option<TextDirection> {
    if !text.chars().any(is_rtl_char) {
        return text.chars().any(is_letter).then_some(TextDirection::Ltr);
    }
    majority(
        split_sentences(text).into_iter().map(words_direction),
        Some(&|| words_direction(text)),
    )
}

/// The length of the Markdown code span, fence or URL starting at `at`, if one does.
fn code_or_url_len(text: &str, at: usize) -> Option<usize> {
    let rest = &text[at..];
    let first = rest.chars().next()?;
    if first == '`' || first == '~' {
        let run = rest.chars().take_while(|c| *c == first).count();
        if run >= 3 {
            let fence = &rest[..run];
            return Some(match rest[run..].find(fence) {
                Some(close) => run + close + run,
                None => rest.len(),
            });
        }
    }
    if first == '`' {
        for ticks in [2usize, 1] {
            if rest.len() < ticks || !rest.as_bytes()[..ticks].iter().all(|b| *b == b'`') {
                continue;
            }
            let opening = &rest[..ticks];
            if rest[ticks..].starts_with('`') {
                continue;
            }
            let mut search = ticks;
            while let Some(found) = rest[search..].find(opening) {
                let close = search + found;
                let after = close + ticks;
                let preceded = rest[..close].ends_with('`');
                let followed = rest[after..].starts_with('`');
                if !preceded && !followed {
                    return Some(after);
                }
                search = close + 1;
            }
        }
    }
    let scheme = ["https://", "http://"]
        .into_iter()
        .find(|scheme| rest.starts_with(scheme))?;
    let body = run_end(rest, scheme.len(), |c| !is_js_space(c));
    (body > scheme.len()).then_some(body)
}

/// The direction of Markdown prose, ignoring fenced and inline code and URLs.
pub fn markdown_prose_direction(markdown: &str) -> Option<TextDirection> {
    let mut prose = String::with_capacity(markdown.len());
    let mut at = 0;
    while at < markdown.len() {
        if let Some(len) = code_or_url_len(markdown, at) {
            prose.push(' ');
            at += len;
        } else if let Some(c) = markdown[at..].chars().next() {
            prose.push(c);
            at += c.len_utf8();
        } else {
            break;
        }
    }
    text_direction(&prose)
}

/// One top-level composer block, as the direction rules see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectionBlock<'a> {
    /// The block's prose with inline code and atoms already replaced by a space.
    pub text: &'a str,
    /// Code blocks stay left to right and do not continue the direction above.
    pub is_code: bool,
}

/// The `composerTextDirections` rule over blocks: a block follows its own letters, a block
/// without letters continues the one above, and code blocks stay left to right.
pub fn composer_block_directions(blocks: &[DirectionBlock<'_>]) -> Vec<Option<TextDirection>> {
    let mut previous = None;
    blocks
        .iter()
        .map(|block| {
            if block.is_code {
                return Some(TextDirection::Ltr);
            }
            let direction = text_direction(block.text).or(previous);
            previous = direction;
            direction
        })
        .collect()
}
