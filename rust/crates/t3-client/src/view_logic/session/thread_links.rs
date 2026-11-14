use super::markdown_link_presentation::{
    ThreadLink, parse_thread_link_href, thread_link_candidates,
};
use crate::types::{EnvironmentId, ThreadKey};

const HOSTED: &str = "https://app.t3.codes/threads/";

fn labeled_destination(source: &str) -> Option<(usize, &str)> {
    let tail = source.strip_prefix("(<")?;
    let end = tail.find('>')?;
    tail[end + 1..]
        .starts_with(')')
        .then_some((end + 4, &tail[..end]))
}

fn unrelated_destination_end(source: &str) -> Option<usize> {
    let tail = source.strip_prefix('(')?;
    let mut at = 0;
    if tail.starts_with('<') {
        at = tail.find('>')? + 1;
        if tail[..at].contains('\n') || tail[1..at - 1].contains('<') {
            return None;
        }
    } else {
        let mut nested = false;
        for (i, ch) in tail.char_indices() {
            match ch {
                '(' if nested => return None,
                '(' => nested = true,
                ')' if nested => nested = false,
                ')' => return Some(i + 2),
                c if c.is_whitespace() => {
                    if nested {
                        return None;
                    }
                    at = i;
                    break;
                }
                _ => {}
            }
            at = i + ch.len_utf8();
        }
        if nested {
            return None;
        }
    }
    let mut end = at;
    while tail
        .as_bytes()
        .get(end)
        .is_some_and(|c| matches!(c, b' ' | b'\t'))
    {
        end += 1;
    }
    if end > at
        && let Some(&quote) = tail
            .as_bytes()
            .get(end)
            .filter(|c| matches!(c, b'\'' | b'"'))
    {
        let rest = &tail[end + 1..];
        let close = rest.find(char::from(quote))?;
        if rest[..close].contains('\n') {
            return None;
        }
        end += close + 2;
    }
    tail[end..].starts_with(')').then_some(end + 2)
}

fn hosted_end(source: &str) -> Option<usize> {
    let tail = source.strip_prefix(HOSTED)?;
    let segment = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'%' | b'-');
    let env = tail.bytes().take_while(|c| segment(*c)).count();
    let tail = tail.get(env..)?.strip_prefix('/')?;
    let thread = tail.bytes().take_while(|c| segment(*c)).count();
    (env > 0 && thread > 0).then_some(HOSTED.len() + env + 1 + thread)
}

pub fn replace_thread_links(
    markdown: &str,
    mut replace: impl FnMut(&ThreadLink, Option<&str>) -> Option<String>,
) -> String {
    let mut result = String::new();
    let mut at = 0;
    let mut copied = 0;
    while at < markdown.len() {
        let source = &markdown[at..];
        let Some(ch) = source.chars().next() else {
            break;
        };
        if ch == '\\'
            && source
                .as_bytes()
                .get(1)
                .is_some_and(u8::is_ascii_punctuation)
        {
            at += 2;
            continue;
        }
        if matches!(ch, '`' | '~') {
            let run = source.bytes().take_while(|b| *b == ch as u8).count();
            if run >= 3 {
                at += source[run..]
                    .find(&source[..run])
                    .map_or(source.len(), |i| i + 2 * run);
                continue;
            }
            if ch == '`'
                && let Some(close) = source[run..].find(&source[..run])
                && !source[run..run + close].contains('\n')
            {
                at += close + 2 * run;
                continue;
            }
        }
        let mut matched = None;
        let mut skip = None;
        if ch == '[' {
            let label_end = source[1..].find(['[', ']', '\n']).map(|i| i + 1);
            if let Some(end) = label_end.filter(|end| source.as_bytes()[*end] == b']') {
                let destination = &source[end + 1..];
                let thread = labeled_destination(destination).or_else(|| {
                    let tail = destination.strip_prefix('(')?;
                    let close = tail.find(')')?;
                    Some((close + 2, &tail[..close]))
                });
                if let Some((length, href)) =
                    thread.filter(|(_, href)| !href.chars().any(char::is_whitespace))
                    && let Some(link) = parse_thread_link_href(href)
                {
                    matched = Some((end + 1 + length, link, Some(&source[1..end])));
                } else {
                    skip = unrelated_destination_end(destination).map(|length| end + 1 + length);
                }
            }
        } else if ch == '<'
            && let Some(end) = source.find('>')
        {
            let href = &source[1..end];
            if !href.contains(['<', '>'])
                && !href.chars().any(char::is_whitespace)
                && let Some(link) = parse_thread_link_href(href)
            {
                matched = Some((end + 1, link, None));
            }
        }
        if matched.is_none() && skip.is_none() && source.starts_with(HOSTED) {
            let url_char = |c: char| c.is_ascii_alphanumeric() || "%/?#=&@+-".contains(c);
            if !markdown[..at].chars().next_back().is_some_and(url_char)
                && let Some(end) = hosted_end(source)
                && !source[end..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || "_%/?#-".contains(c))
                && let Some(link) = parse_thread_link_href(&source[..end])
            {
                matched = Some((end, link, None));
            }
        }
        if let Some((length, link, label)) = matched {
            if let Some(replacement) = replace(&link, label) {
                result.push_str(&markdown[copied..at]);
                result.push_str(&replacement);
                copied = at + length;
            }
            at += length;
        } else {
            at += skip.unwrap_or(ch.len_utf8());
        }
    }
    result.push_str(&markdown[copied..]);
    result
}

pub fn thread_link_refs(markdown: &str, environment: &EnvironmentId) -> Vec<ThreadKey> {
    let mut refs = Vec::new();
    replace_thread_links(markdown, |link, _| {
        for key in thread_link_candidates(link, environment) {
            if !refs.contains(&key) {
                refs.push(key);
            }
        }
        None
    });
    refs
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upstream_links_inside_other_links_and_escapes_stay_written() {
        let hosted = "https://app.t3.codes/threads/there/t1";
        let labeled = "[There](t3-thread://v2/there/t1)";
        let cases = vec![
            (format!("https://example.com/?next={hosted}"), None),
            (
                format!("[Example](https://example.com/?next={hosted})"),
                None,
            ),
            (format!("[ {hosted} ](https://example.com)"), None),
            (
                format!("[{hosted}](<https://example.com/a b> \"Title\")"),
                None,
            ),
            (
                format!("[a](https://example.com/x_(y)) {hosted}"),
                Some(format!("[a](https://example.com/x_(y)) {labeled}")),
            ),
            (
                format!("({hosted}) **{hosted}**"),
                Some(format!("({labeled}) **{labeled}**")),
            ),
            (
                format!("[[[ [old]({hosted})"),
                Some(format!("[[[ {labeled}")),
            ),
            (format!(r"\\[old]({hosted})"), Some(format!(r"\\{labeled}"))),
            (
                format!(r"\\\\[old]({hosted})"),
                Some(format!(r"\\\\{labeled}")),
            ),
            (
                format!(r"\[old]({hosted})"),
                Some(format!(r"\[old]({labeled})")),
            ),
            (
                format!(r"\[ {hosted} ](https://example.com)"),
                Some(format!(r"\[ {labeled} ](https://example.com)")),
            ),
            (
                r"\[old](t3-thread://v2/there/t1) \<t3-thread://v2/there/t1>".into(),
                None,
            ),
        ];
        for (markdown, expected) in cases {
            let actual = replace_thread_links(&markdown, |_, _| Some(labeled.into()));
            assert_eq!(
                actual,
                expected.as_deref().unwrap_or(&markdown),
                "{markdown}"
            );
            let refs = thread_link_refs(&markdown, &EnvironmentId("local".into()));
            assert_eq!(refs.len(), usize::from(expected.is_some()), "{markdown}");
        }
        for literal in [
            format!("`{hosted}`"),
            format!("```\n{hosted}\n```"),
            format!("~~~\n{hosted}\n~~~"),
            format!("{hosted}/longer"),
        ] {
            assert!(thread_link_refs(&literal, &EnvironmentId("local".into())).is_empty());
        }
    }
    #[test]
    fn labeled_internal_links_angle_autolinks_and_hosted_urls_are_found() {
        for markdown in [
            "[title](t3-thread://v1/a)",
            "[title](<t3-thread://v2/env/a(b)>)",
            "<t3-thread://v1/a>",
            "https://app.t3.codes/threads/env/a",
        ] {
            let mut count = 0;
            assert_eq!(
                replace_thread_links(markdown, |link, _| {
                    assert!(
                        parse_thread_link_href(if link.environment.is_none() {
                            "t3-thread://v1/a"
                        } else {
                            "t3-thread://v2/env/a"
                        })
                        .is_some()
                    );
                    count += 1;
                    Some("REPLACED".into())
                }),
                "REPLACED"
            );
            assert_eq!(count, 1);
        }
    }
}
