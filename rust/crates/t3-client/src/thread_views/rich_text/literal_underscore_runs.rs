//! Port of apps/web/src/composer-rich-text.ts literalUnderscoreRuns.
use std::collections::HashSet;
use unicode_general_category::{GeneralCategory as Category, get_general_category};

fn token_char(ch: char, previous: Option<char>, next: Option<char>) -> bool {
    if ch == '~' {
        return previous != Some('~') && next != Some('~');
    }
    matches!(
        ch,
        '_' | '/' | '.' | ':' | '@' | '%' | '+' | '=' | '&' | '?' | '#' | '-'
    ) || matches!(
        get_general_category(ch),
        Category::UppercaseLetter
            | Category::LowercaseLetter
            | Category::TitlecaseLetter
            | Category::ModifierLetter
            | Category::OtherLetter
            | Category::DecimalNumber
            | Category::LetterNumber
            | Category::OtherNumber
    )
}
fn file_extension(value: &str) -> bool {
    let Some((_, suffix)) = value.rsplit_once('.') else {
        return false;
    };
    let mut parts = suffix.split(':');
    let extension = parts.next().unwrap_or_default();
    let mut letters = extension.bytes();
    if !letters.next().is_some_and(|b| b.is_ascii_alphabetic())
        || !letters.all(|b| b.is_ascii_alphanumeric())
    {
        return false;
    }
    let locations: Vec<_> = parts.collect();
    locations.len() <= 2
        && locations
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}
fn collect(token: &str, offset: usize, literal: &mut HashSet<usize>) {
    let trimmed = token.trim_end_matches(['.', ':', '?']);
    let alternatives: Vec<_> = trimmed.split('/').collect();
    if alternatives.len() >= 2
        && alternatives
            .iter()
            .all(|part| part.len() >= 3 && part.starts_with('_') && part.ends_with('_'))
    {
        return;
    }
    let body = trimmed.trim_end_matches('_');
    let is_path = if body.contains('/') {
        body.starts_with('/') || body.starts_with("~/") || body.contains(['.', ':'])
    } else {
        file_extension(body)
    };
    if !is_path {
        return;
    }
    let body: Vec<_> = body.encode_utf16().collect();
    let mut closes_inside = false;
    let mut index = 0;
    while index < body.len() {
        if body[index] != 95 {
            index += 1;
            continue;
        }
        let start = index;
        while body.get(index) == Some(&95) {
            index += 1;
        }
        if start > 0 {
            literal.insert(offset + start);
            closes_inside |= !body
                .get(index)
                .is_some_and(|u| matches!(u, 48..=57 | 65..=90 | 97..=122 | 95));
        }
    }
    if closes_inside && body.first() == Some(&95) {
        literal.insert(offset);
    }
}
pub(super) fn starts(text: &str) -> HashSet<usize> {
    let mut literal = HashSet::new();
    if !text.contains('_') {
        return literal;
    }
    let chars: Vec<_> = text.char_indices().collect();
    let mut start = None;
    let mut utf16_offset = 0;
    for (index, &(byte, ch)) in chars.iter().enumerate() {
        if token_char(
            ch,
            index.checked_sub(1).map(|i| chars[i].1),
            chars.get(index + 1).map(|(_, ch)| *ch),
        ) {
            start.get_or_insert((byte, utf16_offset));
        } else if let Some((from, offset)) = start.take() {
            collect(&text[from..byte], offset, &mut literal);
        }
        utf16_offset += ch.len_utf16();
    }
    if let Some((from, offset)) = start {
        collect(&text[from..], offset, &mut literal);
    }
    literal
}

#[cfg(test)]
mod tests {
    use super::super::{Mark, Span, parse_inline_markdown};
    fn span(text: &str, marks: &[Mark]) -> Span {
        Span {
            text: text.into(),
            marks: marks.into(),
        }
    }
    #[test]
    fn web_url_path_filename_underscores_and_surrounding_emphasis_cases() {
        for text in [
            "Read https://example.com/_handle_/status/1 and open src/__test__.tsx",
            "https://example.com/~_user_/a",
            "_build_/file.ts",
            "edit __init__.py, then",
            "see __init__.py:42",
            "open src/_a.ts_ and example.com/_handle_.",
        ] {
            assert_eq!(parse_inline_markdown(text), vec![span(text, &[])], "{text}");
        }
        let long = format!("/{}a", "_".repeat(50_000));
        assert_eq!(parse_inline_markdown(&long), vec![span(&long, &[])]);
        for (text, styled, mark, rest) in [
            (
                "_see src/a_b.ts_ now",
                "see src/a_b.ts",
                Mark::Italic,
                " now",
            ),
            (
                "_read/write access_ now",
                "read/write access",
                Mark::Italic,
                " now",
            ),
            (
                "__read/write access__ now",
                "read/write access",
                Mark::Bold,
                " now",
            ),
            (
                "_see src/_a.ts_(optional)",
                "see src/_a.ts",
                Mark::Italic,
                "(optional)",
            ),
            (
                "__see src/__a.ts__(optional)",
                "see src/__a.ts",
                Mark::Bold,
                "(optional)",
            ),
            ("_see src/_a.ts_.", "see src/_a.ts", Mark::Italic, "."),
            ("_notes.md_ here", "notes.md", Mark::Italic, " here"),
            ("_e.g._ this", "e.g.", Mark::Italic, " this"),
            ("_and_/or", "and", Mark::Italic, "/or"),
        ] {
            assert_eq!(
                parse_inline_markdown(text),
                vec![span(styled, &[mark]), span(rest, &[])],
                "{text}"
            );
        }
        for (text, first, second, mark) in [
            ("_yes_/_no_", "yes", "no", Mark::Italic),
            ("__yes__/__no__.", "yes", "no", Mark::Bold),
            ("_a_b_/_no_", "a_b", "no", Mark::Italic),
        ] {
            assert_eq!(
                &parse_inline_markdown(text)[..3],
                &[span(first, &[mark]), span("/", &[]), span(second, &[mark])]
            );
        }
        assert_eq!(
            parse_inline_markdown("and/_or_"),
            vec![span("and/", &[]), span("or", &[Mark::Italic])]
        );
        assert_eq!(
            parse_inline_markdown("_build_/file.ts and _x_, __init__.py and __y__"),
            vec![
                span("_build_/file.ts and ", &[]),
                span("x", &[Mark::Italic]),
                span(", __init__.py and ", &[]),
                span("y", &[Mark::Bold])
            ]
        );
    }
}
