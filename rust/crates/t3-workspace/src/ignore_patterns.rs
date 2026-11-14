use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IgnoreDecision {
    Ignored,
    Included,
}
enum Token {
    Char(char),
    One,
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
    Star,
    Any,
    Folders,
}
struct Pattern {
    tokens: Vec<Token>,
    negated: bool,
    folder_only: bool,
}
pub struct IgnoreFile {
    patterns: Vec<Pattern>,
}
impl IgnoreFile {
    pub fn parse(contents: &str) -> Self {
        Self {
            patterns: contents.lines().filter_map(parse_line).collect(),
        }
    }
    pub fn decide(&self, path: &str, folder: bool) -> Option<IgnoreDecision> {
        self.patterns
            .iter()
            .rev()
            .find(|p| (!p.folder_only || folder) && matches(&p.tokens, path))
            .map(|p| {
                if p.negated {
                    IgnoreDecision::Included
                } else {
                    IgnoreDecision::Ignored
                }
            })
    }
}
fn parse_class(chars: &[char], start: usize) -> Option<(Token, usize)> {
    let mut index = start + 1;
    let negated = matches!(chars.get(index), Some('!' | '^'));
    if negated {
        index += 1;
    }
    let mut ranges = Vec::new();
    let mut first = true;
    while let Some(&ch) = chars.get(index) {
        if ch == ']' && !first {
            return Some((Token::Class { negated, ranges }, index));
        }
        let mut ch = ch;
        if ch == '\\' && index + 1 < chars.len() {
            index += 1;
            ch = chars[index];
        }
        if chars.get(index + 1) == Some(&'-') && chars.get(index + 2).is_some_and(|c| *c != ']') {
            let mut last = chars[index + 2];
            index += 2;
            if last == '\\' && index + 1 < chars.len() {
                index += 1;
                last = chars[index];
            }
            ranges.push((ch, last));
        } else {
            ranges.push((ch, ch));
        }
        index += 1;
        first = false;
    }
    None
}
fn parse_line(raw: &str) -> Option<Pattern> {
    let mut line = raw.strip_suffix('\r').unwrap_or(raw);
    while line.ends_with(' ') && !line.ends_with("\\ ") {
        line = &line[..line.len() - 1];
    }
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let negated = line.starts_with('!');
    if negated || line.starts_with("\\!") || line.starts_with("\\#") {
        line = &line[1..];
    }
    let folder_only = line.ends_with('/') && !line.ends_with("\\/");
    if folder_only {
        line = &line[..line.len() - 1];
    }
    if line.is_empty() {
        return None;
    }
    let anchored = line.chars().rev().skip(1).any(|c| c == '/');
    let line = line.strip_prefix('/').unwrap_or(line);
    let chars: Vec<_> = line.chars().collect();
    let mut tokens = if anchored {
        vec![]
    } else {
        vec![Token::Folders]
    };
    let mut classes_close = true;
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if ch == '*'
            && chars.get(index + 1) == Some(&'*')
            && (index == 0 || chars[index - 1] == '/')
        {
            match chars.get(index + 2) {
                Some('/') => {
                    tokens.push(Token::Folders);
                    index += 3;
                    continue;
                }
                None => {
                    tokens.push(Token::Any);
                    index += 2;
                    continue;
                }
                _ => {}
            }
        }
        tokens.push(match ch {
            '*' => Token::Star,
            '?' => Token::One,
            '[' => match if classes_close {
                parse_class(&chars, index)
            } else {
                None
            } {
                Some((token, end)) => {
                    index = end;
                    token
                }
                None => {
                    classes_close = false;
                    Token::Char(ch)
                }
            },
            '\\' if index + 1 < chars.len() => {
                index += 1;
                Token::Char(chars[index])
            }
            _ => Token::Char(ch),
        });
        index += 1;
    }
    Some(Pattern {
        tokens,
        negated,
        folder_only,
    })
}
fn add(states: &mut HashSet<isize>, mut position: usize, tokens: &[Token]) {
    while position <= tokens.len() && states.insert(position as isize) {
        if !matches!(
            tokens.get(position),
            Some(Token::Star | Token::Any | Token::Folders)
        ) {
            return;
        }
        position += 1;
    }
}
// NFA positions keep globstar segment boundaries; no repository pattern invokes backtracking.
fn matches(tokens: &[Token], path: &str) -> bool {
    let mut states = HashSet::new();
    add(&mut states, 0, tokens);
    for ch in path.chars() {
        let mut next = HashSet::new();
        for state in states {
            let position = if state < 0 { !state } else { state } as usize;
            let Some(token) = tokens.get(position) else {
                continue;
            };
            match token {
                Token::Char(c) if *c == ch => add(&mut next, position + 1, tokens),
                Token::One if ch != '/' => add(&mut next, position + 1, tokens),
                Token::Class { negated, ranges }
                    if ch != '/'
                        && ranges.iter().any(|(a, b)| *a <= ch && ch <= *b) != *negated =>
                {
                    add(&mut next, position + 1, tokens)
                }
                Token::Star if ch != '/' => add(&mut next, position, tokens),
                Token::Any => add(&mut next, position, tokens),
                Token::Folders => {
                    if ch == '/' {
                        add(&mut next, position, tokens);
                    } else {
                        next.insert(!(position as isize));
                    }
                }
                _ => {}
            }
        }
        if next.is_empty() {
            return false;
        }
        states = next;
    }
    states.contains(&(tokens.len() as isize))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn precedence_anchoring_globstars_classes_escapes_and_folder_rules() {
        use IgnoreDecision::{Ignored as I, Included as N};
        for (patterns, path, folder, expected) in [
            ("*keep*\n!*keep*\n", "sub/keep-link.ts", false, Some(N)),
            ("!*keep*\n*keep*\n", "sub/keep-link.ts", false, Some(I)),
            ("# comment\n\nother\n", "keep.ts", false, None),
            ("link", "a/b/link", false, Some(I)),
            ("/link", "a/link", false, None),
            ("/link", "link", false, Some(I)),
            ("a/link", "x/a/link", false, None),
            ("a/*.ts", "a/b/c.ts", false, None),
            ("link", "blink", false, None),
            ("link", "a/blink", false, None),
            ("**/gen/*.ts", "gen/a.ts", false, Some(I)),
            ("a/**/b", "a/x/y/b", false, Some(I)),
            ("a/**/b", "a/b", false, Some(I)),
            ("foo/**/bar", "foo/xbar", false, None),
            ("foo/**/bar", "foo/x/ybar", false, None),
            ("out/**", "out/x/y", false, Some(I)),
            ("link-[0-9]", "link-7", false, Some(I)),
            ("link-[!0-9]", "link-7", false, None),
            ("link-?", "link-/", false, None),
            ("\\!bang\n\\#hash\nstar\\*", "!bang", false, Some(I)),
            ("\\!bang\n\\#hash\nstar\\*", "#hash", false, Some(I)),
            ("\\!bang\n\\#hash\nstar\\*", "starx", false, None),
            ("dist/", "dist", false, None),
            ("dist/", "dist", true, Some(I)),
            ("[a][", "a[", false, Some(I)),
            ("name ", "name", false, Some(I)),
            ("name\\ ", "name ", false, Some(I)),
        ] {
            assert_eq!(
                IgnoreFile::parse(patterns).decide(path, folder),
                expected,
                "{patterns:?} {path}"
            );
        }
    }
    #[test]
    fn adversarial_patterns_and_unclosed_classes_do_not_backtrack() {
        let pattern = format!("{}b\n", "a*".repeat(20));
        assert_eq!(
            IgnoreFile::parse(&pattern).decide(&format!("{}c", "a".repeat(200)), false),
            None
        );
        assert_eq!(
            IgnoreFile::parse(&pattern.repeat(50)).decide(&format!("{}c", "a".repeat(1000)), false),
            None
        );
        assert_eq!(
            IgnoreFile::parse(&pattern).decide(&format!("{}b", "a".repeat(200)), false),
            Some(IgnoreDecision::Ignored)
        );
        assert_eq!(
            IgnoreFile::parse(&"[".repeat(30_000)).decide(&"[".repeat(30_000), false),
            Some(IgnoreDecision::Ignored)
        );
        assert_eq!(
            IgnoreFile::parse(&format!("{}]", "[".repeat(30_000))).decide("x", false),
            None
        );
    }
}
