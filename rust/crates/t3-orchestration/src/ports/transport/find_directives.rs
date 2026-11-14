use pulldown_cmark::{Event, Options, Parser, Tag};
use std::{collections::BTreeMap, ops::Range};

pub(super) enum Segment {
    Markdown(String),
    Template(String, &'static str),
    Literal(String),
}

struct Directive<'a> {
    end: usize,
    name: &'a str,
    label: &'a str,
    attributes: BTreeMap<String, String>,
}

fn decoded(text: &str) -> String {
    Parser::new(text)
        .filter_map(|event| match event {
            Event::Text(text) | Event::Code(text) => Some(text.into_string()),
            Event::SoftBreak | Event::HardBreak => Some("\n".into()),
            _ => None,
        })
        .collect()
}

fn directive(source: &str, start: usize, leaf: bool) -> Option<Directive<'_>> {
    let marker = if leaf { "::" } else { ":" };
    let rest = source.get(start..)?.strip_prefix(marker)?;
    let name = if leaf {
        "artifact-template"
    } else if rest.starts_with("codex-file-citation") {
        "codex-file-citation"
    } else {
        "codex-followup"
    };
    let rest = rest.strip_prefix(name)?;
    let mut cursor = source.len() - rest.len();
    let mut label = "";
    if source.as_bytes().get(cursor) == Some(&b'[') {
        let begin = cursor + 1;
        cursor += 1;
        let mut depth = 1;
        while let Some(byte) = source.as_bytes().get(cursor) {
            match byte {
                b'\\' => cursor += 1,
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        label = &source[begin..cursor];
                        cursor += 1;
                        break;
                    }
                }
                b'\r' | b'\n' => return None,
                _ => {}
            }
            cursor += 1;
        }
        if depth != 0 {
            return None;
        }
    }
    if source.as_bytes().get(cursor) != Some(&b'{') {
        return None;
    }
    cursor += 1;
    let mut attributes = BTreeMap::new();
    loop {
        while source
            .as_bytes()
            .get(cursor)
            .is_some_and(u8::is_ascii_whitespace)
        {
            cursor += 1;
        }
        let byte = *source.as_bytes().get(cursor)?;
        if byte == b'}' {
            return Some(Directive {
                end: cursor + 1,
                name,
                label,
                attributes,
            });
        }
        let begin = cursor;
        while source.as_bytes().get(cursor).is_some_and(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b':' | b'.')
        }) {
            cursor += 1;
        }
        if begin == cursor {
            return None;
        }
        let key = &source[begin..cursor];
        while source
            .as_bytes()
            .get(cursor)
            .is_some_and(u8::is_ascii_whitespace)
        {
            cursor += 1;
        }
        let mut value = String::new();
        if source.as_bytes().get(cursor) == Some(&b'=') {
            cursor += 1;
            while source
                .as_bytes()
                .get(cursor)
                .is_some_and(u8::is_ascii_whitespace)
            {
                cursor += 1;
            }
            let quote = source
                .as_bytes()
                .get(cursor)
                .copied()
                .filter(|byte| matches!(byte, b'\'' | b'"'));
            cursor += usize::from(quote.is_some());
            let begin = cursor;
            while source.as_bytes().get(cursor).is_some_and(|byte| {
                quote.map_or(!byte.is_ascii_whitespace() && *byte != b'}', |quote| {
                    *byte != quote
                })
            }) {
                cursor += 1;
            }
            value = decoded(&source[begin..cursor]);
            if quote.is_some() {
                source
                    .as_bytes()
                    .get(cursor)
                    .filter(|byte| Some(**byte) == quote)?;
                cursor += 1;
            }
        }
        attributes.insert(key.into(), value);
    }
}

fn template(attributes: &BTreeMap<String, String>) -> Option<(String, &'static str)> {
    let label = match attributes.get("artifact_kind")?.as_str() {
        "document" => "Document template",
        "presentation" => "Presentation template",
        "spreadsheet" => "Spreadsheet template",
        "site" => "Site template",
        "google-docs" => "Google Doc template",
        "google-slides" => "Google Slides template",
        "google-sheets" => "Google Sheet template",
        "image" => "Image template",
        "email" => "Email template",
        "slack" => "Slack template",
        _ => return None,
    };
    let name = t3_contracts::wire::trim_js(attributes.get("display_name")?);
    let directory = attributes.get("skill_directory")?;
    let absolute = (directory.starts_with('/') && !directory.starts_with("//"))
        || (directory
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
            && directory.as_bytes().get(1) == Some(&b':')
            && directory
                .as_bytes()
                .get(2)
                .is_some_and(|byte| matches!(byte, b'/' | b'\\')))
        || directory
            .strip_prefix("//")
            .or_else(|| directory.strip_prefix("\\\\"))
            .is_some_and(|path| {
                let mut parts = path.split(['/', '\\']);
                parts.next().is_some_and(|part| !part.is_empty())
                    && parts.next().is_some_and(|part| !part.is_empty())
            });
    if name.is_empty()
        || !absolute
        || !attributes
            .get("skill_name")?
            .starts_with("artifact-template-")
        || attributes
            .get("gallery_kind")
            .is_some_and(|kind| kind != "imagegen" && kind != "product-design")
    {
        return None;
    }
    Some((name.into(), label))
}

fn escaped(source: &str, start: usize) -> bool {
    source.as_bytes()[..start]
        .iter()
        .rev()
        .take_while(|byte| **byte == b'\\')
        .count()
        % 2
        == 1
}
fn number(text: &str) -> Option<f64> {
    let text = t3_contracts::wire::trim_js(text);
    for (prefix, radix) in [("0x", 16), ("0b", 2), ("0o", 8)] {
        if text
            .get(..2)
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        {
            return u64::from_str_radix(&text[2..], radix)
                .ok()
                .map(|number| number as f64);
        }
    }
    text.parse().ok()
}

pub(super) fn segments(source: &str, options: Options) -> Vec<Segment> {
    let mut code = Vec::<Range<usize>>::new();
    let mut links = vec![];
    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        match event {
            Event::Code(_) | Event::Start(Tag::CodeBlock(_)) => code.push(range),
            Event::Start(Tag::Link { .. }) => links.push(range),
            _ => {}
        }
    }
    let mut out = vec![];
    let mut markdown = String::new();
    let mut cursor = 0;
    for (start, _) in source.match_indices(':') {
        if start < cursor
            || escaped(source, start)
            || source.as_bytes().get(start.wrapping_sub(1)) == Some(&b':')
            || code.iter().any(|range| range.contains(&start))
        {
            continue;
        }
        let line_start = source[..start].rfind('\n').map_or(0, |index| index + 1);
        let prefix = &source[line_start..start];
        let leaf = prefix.len() <= 3
            && prefix.bytes().all(|byte| byte == b' ')
            && source[start..].starts_with("::artifact-template");
        let Some(directive) = directive(source, start, leaf) else {
            continue;
        };
        if leaf {
            if !source[directive.end..]
                .split('\n')
                .next()
                .unwrap_or_default()
                .trim()
                .is_empty()
            {
                continue;
            }
            if let Some((name, label)) = template(&directive.attributes) {
                markdown.push_str(&source[cursor..line_start]);
                out.push(Segment::Markdown(std::mem::take(&mut markdown)));
                out.push(Segment::Template(name, label));
                cursor = directive.end;
            } else {
                markdown.push_str(&source[cursor..line_start]);
                out.push(Segment::Markdown(std::mem::take(&mut markdown)));
                out.push(Segment::Literal(
                    source[start..directive.end]
                        .replace("\r\n", " ")
                        .replace('\n', " "),
                ));
                cursor = directive.end;
            }
        } else if directive.name == "codex-followup" {
            if !directive
                .attributes
                .get("prompt")
                .is_none_or(|prompt| t3_contracts::wire::trim_js(prompt).is_empty())
                && !decoded(directive.label).trim().is_empty()
            {
                markdown.push_str(&source[cursor..start]);
                markdown.push_str(directive.label);
                cursor = directive.end;
            }
        } else if !links.iter().any(|range| range.contains(&start))
            && let Some(path) = directive
                .attributes
                .get("path")
                .map(|path| t3_contracts::wire::trim_js(path))
                .filter(|path| !path.is_empty())
        {
            let label_path = path.replace('\\', "/");
            let label_path = label_path.trim_end_matches('/');
            let label = label_path
                .rsplit('/')
                .next()
                .filter(|name| !name.is_empty())
                .unwrap_or("File");
            let label: String = label
                .chars()
                .flat_map(|c| {
                    if matches!(c, '\\' | '[' | ']' | '*' | '_' | '`' | '<' | '&') {
                        vec!['\\', c]
                    } else {
                        vec![c]
                    }
                })
                .collect();
            let mut destination = path
                .replace('%', "%25")
                .replace('#', "%23")
                .replace('?', "%3F");
            if let Some(line) = directive
                .attributes
                .get("line_range_start")
                .and_then(|value| number(value))
                .filter(|line| {
                    line.is_finite()
                        && line.fract() == 0.0
                        && *line > 0.0
                        && *line <= 9_007_199_254_740_991.0
                })
            {
                destination.push_str(&format!("#L{line:.0}"));
            }
            destination = destination
                .replace('\\', "%5C")
                .replace('<', "%3C")
                .replace('>', "%3E")
                .replace('\r', "%0D")
                .replace('\n', "%0A");
            markdown.push_str(&source[cursor..start]);
            markdown.push_str(&format!("[{label}](<{destination}>)"));
            cursor = directive.end;
        }
        if cursor < directive.end {
            markdown.push_str(&source[cursor..start]);
            for c in source[start..directive.end].chars() {
                if c.is_ascii_punctuation() {
                    markdown.push('\\');
                }
                markdown.push(c);
            }
            cursor = directive.end;
        }
    }
    markdown.push_str(&source[cursor..]);
    out.push(Segment::Markdown(markdown));
    out
}
