//! Rendered find text from shared/threadFindText.ts, using CommonMark block boundaries.
use crate::Result;
use crate::regex;
use crate::tool_output::{html_render_from_tool_item, mcp_app_from_tool_item};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::orchestration_v2::{OrchestrationV2TurnItem, SearchThreadInputSkillsItem};
#[path = "find_directives.rs"]
mod directives;
#[path = "find_legacy.rs"]
mod legacy;

#[derive(Default)]
pub struct NativeThreadFindText;
fn space(c: char) -> bool {
    t3_contracts::wire::trim_js(&c.to_string()).is_empty()
}
fn percent_decode(text: &str, form: bool) -> Option<String> {
    let mut bytes = vec![];
    let mut cursor = 0;
    let input = text.as_bytes();
    while cursor < input.len() {
        if input[cursor] == b'%' {
            let encoded = std::str::from_utf8(input.get(cursor + 1..cursor + 3)?).ok()?;
            bytes.push(u8::from_str_radix(encoded, 16).ok()?);
            cursor += 3;
        } else {
            bytes.push(if form && input[cursor] == b'+' {
                b' '
            } else {
                input[cursor]
            });
            cursor += 1;
        }
    }
    String::from_utf8(bytes).ok()
}
pub fn citation(href: &str) -> Option<(String, Option<String>)> {
    use t3_contracts::assistant_citations::*;
    if href.encode_utf16().count()
        > 9 * (ASSISTANT_CITATION_MAX_TEXT_LENGTH
            + ASSISTANT_CITATION_MAX_COMMENT_LENGTH
            + 3 * ASSISTANT_CITATION_MAX_ID_LENGTH
            + 2 * ASSISTANT_CITATION_CONTEXT_LENGTH)
            + 1000
    {
        return None;
    }
    let path = href.strip_prefix("t3-citation://v1/")?;
    if path.contains('#') {
        return None;
    }
    let (path, query) = path.split_once('?')?;
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() != 3
        || parts.iter().any(|part| {
            percent_decode(part, false).is_none_or(|part| {
                part.trim_matches(space).is_empty()
                    || part.encode_utf16().count() > ASSISTANT_CITATION_MAX_ID_LENGTH
            })
        })
    {
        return None;
    }
    let mut params = BTreeMap::new();
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if params
            .insert(percent_decode(key, true)?, percent_decode(value, true)?)
            .is_some()
        {
            return None;
        }
    }
    if params.len() != 5 + usize::from(params.contains_key("comment"))
        || ["text", "start", "end", "prefix", "suffix"]
            .iter()
            .any(|key| !params.contains_key(*key))
    {
        return None;
    }
    let integer = |key: &str| {
        let value = &params[key];
        if value.is_empty() || value.len() > 16 || !value.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        value
            .parse::<u64>()
            .ok()
            .filter(|value| *value <= 9_007_199_254_740_991)
    };
    let start = integer("start")?;
    let end = integer("end")?;
    let text = params.get("text")?;
    if text.is_empty()
        || text.encode_utf16().count() > ASSISTANT_CITATION_MAX_TEXT_LENGTH
        || end < start
        || end - start != text.encode_utf16().count() as u64
        || params["prefix"].encode_utf16().count() > 32
        || params["suffix"].encode_utf16().count() > 32
        || params.get("comment").is_some_and(|comment| {
            comment.encode_utf16().count() > ASSISTANT_CITATION_MAX_COMMENT_LENGTH
        })
    {
        return None;
    }
    Some((text.clone(), params.remove("comment")))
}
pub fn readable_citations(text: &str) -> String {
    regex(r"\[Assistant quote\]\((t3-citation://v1/[^\s)]+)\)")
        .replace_all(text, |captures: &regex::Captures<'_>| {
            citation(&captures[1]).map_or_else(
                || captures[0].to_owned(),
                |(text, comment)| {
                    comment.map_or_else(
                        || text.clone(),
                        |comment| format!("{text}\nComment: {comment}"),
                    )
                },
            )
        })
        .into_owned()
}
pub use crate::thread_search::count_thread_search_occurrences as occurrences;
fn file_target(raw: &str, cwd: Option<&str>) -> Option<(String, Option<String>)> {
    if raw.starts_with("http:")
        || raw.starts_with("https:")
        || raw.starts_with("t3-")
        || raw.starts_with('#')
        || raw.starts_with("//")
        || raw.starts_with("\\\\")
    {
        return None;
    }
    let raw = raw.strip_prefix("file://").unwrap_or(raw);
    let raw = percent_decode(raw, false)?.replace('\\', "/");
    let (path, position) = if let Some(captures) = regex(
        r"(?::([0-9]+)(?::([0-9]+))?(?:-([0-9]+))?|#L([0-9]+)(?:C([0-9]+))?(?:-L?([0-9]+))?)$",
    )
    .captures(&raw)
    {
        let matched = captures.get(0)?;
        let line = captures
            .get(1)
            .or(captures.get(4))?
            .as_str()
            .parse::<u64>()
            .ok()?;
        let column = captures.get(2).or(captures.get(5)).map(|c| c.as_str());
        let end = captures.get(3).or(captures.get(6)).map(|c| c.as_str());
        let position = (line > 0).then(|| {
            format!(
                "L{line}{}",
                end.filter(|end| end.parse::<u64>().ok().is_some_and(|end| end > line))
                    .map_or_else(
                        || column
                            .filter(|column| *column != "0")
                            .map_or(String::new(), |column| format!(":C{column}")),
                        |end| format!("–{end}")
                    )
            )
        });
        (raw[..matched.start()].to_owned(), position)
    } else {
        (raw, None)
    };
    let absolute = path.starts_with('/') || path.as_bytes().get(1) == Some(&b':');
    if !absolute && cwd.is_none() {
        return None;
    }
    if (!path.contains('/') && !path.contains('.') && !extensionless(&path))
        || (!path.contains('/') && path.contains('@'))
        || regex(r"(?:^|/)(?:\.{3,}|…)(?:/|$)").is_match(&path)
        || (path.contains(':') && !absolute)
    {
        return None;
    }
    if path.contains("://") || path.trim().is_empty() {
        return None;
    }
    let path = if absolute {
        path
    } else {
        format!(
            "{}/{}",
            cwd.unwrap_or_default()
                .replace('\\', "/")
                .trim_end_matches('/'),
            path
        )
    };
    let mut parts = vec![];
    for part in path.split('/') {
        match part {
            "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    Some((parts.join("/"), position))
}
fn extensionless(path: &str) -> bool {
    matches!(
        path,
        "Makefile"
            | "makefile"
            | "GNUmakefile"
            | "Dockerfile"
            | "Containerfile"
            | "Justfile"
            | "justfile"
            | "Rakefile"
            | "Gemfile"
            | "Procfile"
            | "Brewfile"
            | "Caddyfile"
            | "Vagrantfile"
            | "Jenkinsfile"
            | "Podfile"
            | "Fastfile"
            | "BUILD"
            | "WORKSPACE"
            | "LICENSE"
            | "LICENCE"
            | "COPYING"
            | "NOTICE"
            | "AUTHORS"
            | "CONTRIBUTORS"
            | "CHANGELOG"
            | "README"
            | "CODEOWNERS"
    )
}
fn code_target(code: &str, cwd: Option<&str>) -> Option<(String, Option<String>)> {
    let code = code.trim_matches(space);
    if code.chars().any(|c| space(c) || c == '`') {
        return None;
    }
    let code = code.replace('\\', "/");
    let position = regex(r":[0-9]+(?::[0-9]+|-[0-9]+)?$");
    let has_position = position.is_match(&code);
    if !has_position && !code.contains('/') {
        return None;
    }
    let path = position.replace(&code, "");
    if !code.starts_with('/')
        && !code.starts_with("./")
        && !code.starts_with("../")
        && !code.starts_with("~/")
        && code.as_bytes().get(1) != Some(&b':')
    {
        if path.ends_with('/') {
            return None;
        }
        let basename = path.rsplit('/').next().unwrap_or_default();
        let first = path.split('/').next().unwrap_or_default();
        if first == "localhost" || regex(r"(?i)\.(?:com|net|org|io|dev|app|ai|co|edu|gov|info|biz|xyz|me|tv|gg|chat|cloud|site|online|tech|store|link|uk|de|fr|nl|se|no|fi|dk|pl|ch|at|be|es|it|pt)$").is_match(first)
            || regex(r"[0-9]\.[0-9][^.]*$").is_match(basename)
            || (!regex(r"\.[A-Za-z0-9_-]+$").is_match(basename) && (!has_position || (path.contains('/') && !regex(r"\.[^./\\]+$").is_match(basename) && !extensionless(basename)))) {
            return None;
        }
    }
    file_target(&code, cwd)
}
fn parents(paths: &[(String, Option<String>)]) -> BTreeMap<String, String> {
    let mut groups: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (path, _) in paths {
        groups
            .entry(path.rsplit('/').next().unwrap_or(path).into())
            .or_default()
            .insert(path.clone());
    }
    let mut out = BTreeMap::new();
    for group in groups.values().filter(|group| group.len() > 1) {
        let parents: Vec<_> = group
            .iter()
            .map(|path| {
                (
                    path,
                    path.split('/')
                        .filter(|part| !part.is_empty())
                        .rev()
                        .skip(1)
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        for (path, parts) in &parents {
            let mut depth = parts.len();
            for size in 1..=parts.len() {
                let suffix = &parts[..size];
                if parents
                    .iter()
                    .filter(|(_, parts)| parts.get(..size) == Some(suffix))
                    .count()
                    == 1
                {
                    depth = size;
                    break;
                }
            }
            let depth = depth.max(2).min(parts.len());
            if depth > 0 {
                out.insert(
                    (*path).clone(),
                    parts[..depth]
                        .iter()
                        .rev()
                        .copied()
                        .collect::<Vec<_>>()
                        .join("/"),
                );
            }
        }
    }
    out
}
fn label(target: &(String, Option<String>), parents: &BTreeMap<String, String>) -> String {
    let mut parts = vec![target.0.rsplit('/').next().unwrap_or(&target.0).to_owned()];
    if let Some(parent) = parents.get(&target.0) {
        parts.push(parent.clone());
    }
    parts.extend(target.1.clone());
    parts.join(" · ")
}
fn same_file_label(description: &str, href: &str) -> bool {
    let normalized = |path: &str| {
        path.replace('\\', "/")
            .trim_start_matches("./")
            .trim_end_matches('/')
            .to_owned()
    };
    let position = regex(r"(?::[0-9]+(?::[0-9]+|-[0-9]+)?|#L[0-9]+(?:C[0-9]+)?(?:-L?[0-9]+)?)$");
    let description = description.trim_matches(space);
    let href = percent_decode(href, false).unwrap_or_else(|| href.into());
    let (Some((path, at)), Some((label, label_at))) = (
        file_target(&href, Some("/")),
        file_target(description, Some("/")),
    ) else {
        return description.is_empty();
    };
    if position.is_match(description) && label_at != at {
        return false;
    }
    let mut path = normalized(&path);
    let mut label = normalized(&label);
    if href.as_bytes().get(1) == Some(&b':') {
        path = path.to_lowercase();
        label = label.to_lowercase();
    }
    path == label || path.ends_with(&format!("/{}", label.trim_start_matches('/')))
}
fn skills(text: &str, skills: &[SearchThreadInputSkillsItem]) -> String {
    let token = regex(r"(^|\s)(\p{Sc})([a-zA-Z0-9][a-zA-Z0-9:_-]*)");
    token
        .replace_all(text, |captures: &regex::Captures<'_>| {
            let name = &captures[3];
            if !captures[1].chars().all(space)
                || !name.chars().any(|c| c.is_ascii_alphabetic())
                || text[captures.get(0).map_or(text.len(), |capture| capture.end())..]
                    .chars()
                    .next()
                    .is_some_and(|c| !space(c))
                || regex(r"^[0-9][0-9_]*(?:[kKmMbBtT]|[eE][0-9]+)?$").is_match(name)
            {
                return captures[0].to_owned();
            }
            let found = skills.iter().find(|skill| skill.name == name);
            found.map_or_else(
                || captures[0].to_owned(),
                |skill| {
                    format!(
                        "{}{}",
                        &captures[1],
                        skill
                            .display_name
                            .as_ref()
                            .and_then(Option::as_ref)
                            .map(|name| name.trim_matches(space))
                            .filter(|name| !name.is_empty())
                            .unwrap_or(&skill.name),
                    )
                },
            )
        })
        .into_owned()
}
fn prose_files(text: &str, cwd: Option<&str>, suffixes: &BTreeMap<String, String>) -> String {
    regex(r#"(^|[\s(\["'])(/[^\s<>"'`]+)"#)
        .replace_all(text, |capture: &regex::Captures<'_>| {
            let raw = &capture[2];
            let mut parentheses = raw.matches('(').count() as i64 - raw.matches(')').count() as i64;
            let mut brackets = raw.matches('[').count() as i64 - raw.matches(']').count() as i64;
            let mut path = raw;
            while let Some(c) = path.chars().last() {
                if c == ')' && parentheses < 0 {
                    parentheses += 1;
                } else if c == ']' && brackets < 0 {
                    brackets += 1;
                } else if !".,;!?:".contains(c) {
                    break;
                }
                path = &path[..path.len() - c.len_utf8()];
            }
            let has_position = regex(r":[0-9]+(?::[0-9]+|-[0-9]+)?$").is_match(path);
            let root = [
                "/Users/",
                "/home/",
                "/tmp/",
                "/var/",
                "/etc/",
                "/opt/",
                "/mnt/",
                "/Volumes/",
                "/private/",
                "/root/",
                "/usr/",
                "/bin/",
                "/sbin/",
                "/lib/",
                "/lib64/",
                "/srv/",
                "/dev/",
                "/proc/",
                "/sys/",
                "/run/",
                "/boot/",
                "/media/",
                "/workspace/",
                "/workspaces/",
            ]
            .iter()
            .any(|root| path.starts_with(root));
            let basename = path.rsplit('/').next().unwrap_or_default();
            if (!root && !has_position)
                || (!has_position
                    && !extensionless(basename)
                    && !regex(r"\.[A-Za-z0-9_-]+$").is_match(basename))
                || (path.ends_with('/') && raw[path.len()..].starts_with("..."))
            {
                return capture[0].into();
            }
            file_target(path, cwd).map_or_else(
                || capture[0].into(),
                |target| {
                    format!(
                        "{}{}{}",
                        &capture[1],
                        label(&target, suffixes),
                        &raw[path.len()..]
                    )
                },
            )
        })
        .into_owned()
}
fn block_tag(tag: &str) -> bool {
    matches!(
        tag,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "br"
            | "dd"
            | "details"
            | "div"
            | "dl"
            | "dt"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hr"
            | "li"
            | "main"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "summary"
            | "table"
            | "tbody"
            | "td"
            | "th"
            | "thead"
            | "tr"
            | "ul"
    )
}
struct Render {
    segments: Vec<String>,
    text: String,
    hidden_tag: bool,
    raw_pre: usize,
}
impl Render {
    fn flush(&mut self) {
        if !self.text.trim_matches(space).is_empty() {
            self.segments.push(std::mem::take(&mut self.text));
        } else {
            self.text.clear();
        }
    }
    fn html_text(&mut self, text: &str) {
        if self.hidden_tag {
            return;
        }
        let text = regex(r"&(?:#[xX][0-9a-fA-F]+|#[0-9]+|[A-Za-z][A-Za-z0-9]+);").replace_all(
            text,
            |entity: &regex::Captures<'_>| {
                Parser::new(&entity[0])
                    .filter_map(|event| {
                        if let Event::Text(text) = event {
                            Some(text.into_string())
                        } else {
                            None
                        }
                    })
                    .collect::<String>()
            },
        );
        if self.raw_pre > 0 {
            self.text.push_str(&text);
        } else {
            self.text
                .push_str(&text.replace("\r\n", " ").replace('\n', " "));
        }
    }
    fn html(&mut self, text: &str, user: bool) {
        if user {
            self.text.push_str(text);
            return;
        }
        let tags = regex(
            r#"(?s)<!--.*?(?:-->|$)|</?([a-zA-Z][a-zA-Z0-9-]*)\b(?:[^"'<>]|"[^"]*"|'[^']*')*>"#,
        );
        let mut offset = 0;
        for captures in tags.captures_iter(text) {
            let Some(matched) = captures.get(0) else {
                continue;
            };
            self.html_text(&text[offset..matched.start()]);
            let tag = captures.get(1).map(|tag| tag.as_str().to_lowercase());
            if tag.as_deref() == Some("script") {
                self.hidden_tag = !matched.as_str().starts_with("</");
            } else if tag.as_deref().is_some_and(block_tag) && !self.hidden_tag {
                self.flush();
            }
            if tag.as_deref() == Some("pre") {
                if matched.as_str().starts_with("</") {
                    self.raw_pre = self.raw_pre.saturating_sub(1);
                } else {
                    self.raw_pre += 1;
                }
            }
            offset = matched.end();
        }
        self.html_text(&text[offset..]);
    }
}
pub fn markdown_segments(
    markdown: &str,
    user: bool,
    cwd: Option<&str>,
    skill_list: &[SearchThreadInputSkillsItem],
) -> Vec<String> {
    markdown_segments_breaks(markdown, user, cwd, skill_list, user)
}
fn markdown_segments_breaks(
    markdown: &str,
    user: bool,
    cwd: Option<&str>,
    skill_list: &[SearchThreadInputSkillsItem],
    breaks: bool,
) -> Vec<String> {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let markdown = github_alerts(markdown, options);
    directives::segments(&markdown, options)
        .into_iter()
        .flat_map(|segment| match segment {
            directives::Segment::Markdown(markdown) => {
                markdown_segments_core(&markdown, user, cwd, skill_list, breaks)
            }
            directives::Segment::Template(name, label) => vec![name, label.into()],
            directives::Segment::Literal(text) => vec![text],
        })
        .collect()
}
fn markdown_segments_core(
    markdown: &str,
    user: bool,
    cwd: Option<&str>,
    skill_list: &[SearchThreadInputSkillsItem],
    breaks: bool,
) -> Vec<String> {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let escaped;
    let markdown = if user {
        let protected: Vec<_> = Parser::new_ext(markdown, options)
            .into_offset_iter()
            .filter_map(|(event, range)| {
                matches!(event, Event::Code(_) | Event::Start(Tag::CodeBlock(_))).then_some(range)
            })
            .collect();
        let mut text = String::new();
        for (index, c) in markdown.char_indices() {
            if c == '~'
                && markdown.as_bytes().get(index.wrapping_sub(1)) != Some(&b'~')
                && markdown.as_bytes().get(index + 1) != Some(&b'~')
                && !protected.iter().any(|range| range.contains(&index))
            {
                text.push('\\');
            }
            text.push(c);
        }
        escaped = text;
        escaped.as_str()
    } else {
        markdown
    };
    let paths: Vec<_> = Parser::new_ext(markdown, options)
        .filter_map(|event| match event {
            Event::Start(Tag::Link { dest_url, .. }) => file_target(&dest_url, cwd),
            Event::Code(code) => code_target(&code, cwd),
            _ => None,
        })
        .collect();
    let suffixes = parents(&paths);
    let mut render = Render {
        segments: vec![],
        text: String::new(),
        hidden_tag: false,
        raw_pre: 0,
    };
    let mut in_pre = 0usize;
    let mut in_link = 0usize;
    let mut skill_blocks = 0usize;
    let mut skipped = 0;
    let mut captured: Option<(String, String, String)> = None;
    for event in Parser::new_ext(markdown, options) {
        if skipped > 0 {
            match event {
                Event::Start(_) => skipped += 1,
                Event::End(_) => skipped -= 1,
                _ => {}
            }
            continue;
        }
        match &event {
            Event::Start(Tag::Paragraph | Tag::Item) => skill_blocks += 1,
            Event::End(TagEnd::Paragraph | TagEnd::Item) => {
                skill_blocks = skill_blocks.saturating_sub(1)
            }
            _ => {}
        }
        match event {
            Event::Start(Tag::Image { .. }) => {
                skipped = 1;
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                if user
                    && regex(r"^t3-context://v1/[a-z][a-z0-9-]{0,39}/[a-zA-Z0-9_-]{1,128}$")
                        .is_match(&dest_url)
                {
                    render.flush();
                    skipped = 1;
                    continue;
                }
                if let Some((_, comment)) = citation(&dest_url) {
                    render.flush();
                    render.text.push_str(
                        &comment
                            .unwrap_or_default()
                            .replace("\r\n", " ")
                            .replace('\n', " "),
                    );
                    skipped = 1;
                    continue;
                }
                in_link += 1;
                if let Some(target) = file_target(&dest_url, cwd) {
                    captured = Some((
                        String::new(),
                        label(&target, &suffixes),
                        dest_url.into_string(),
                    ));
                }
            }
            Event::End(TagEnd::Link | TagEnd::Image) => {
                in_link = in_link.saturating_sub(1);
                if let Some((description, label, href)) = captured.take() {
                    if !description.is_empty() && !same_file_label(&description, &href) {
                        render.text.push_str(&description);
                        render.text.push(' ');
                    }
                    render.text.push_str(&label);
                }
            }
            Event::Start(Tag::CodeBlock(_)) => {
                render.flush();
                in_pre += 1;
            }
            Event::End(TagEnd::CodeBlock) => {
                render.flush();
                in_pre = in_pre.saturating_sub(1);
            }
            Event::Start(
                Tag::Paragraph
                | Tag::Heading { .. }
                | Tag::BlockQuote(_)
                | Tag::Item
                | Tag::List(_)
                | Tag::Table(_)
                | Tag::TableHead
                | Tag::TableRow
                | Tag::TableCell,
            ) => render.flush(),
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::BlockQuote(_)
                | TagEnd::Item
                | TagEnd::List(_)
                | TagEnd::Table
                | TagEnd::TableHead
                | TagEnd::TableRow
                | TagEnd::TableCell,
            ) => render.flush(),
            Event::Text(text) => {
                if render.hidden_tag {
                    continue;
                }
                let text = if in_pre == 0 {
                    text.replace('\n', " ")
                } else {
                    text.into_string()
                };
                let text = if in_pre == 0 && in_link == 0 {
                    prose_files(&text, cwd, &suffixes)
                } else {
                    text
                };
                if let Some((description, _, _)) = &mut captured {
                    description.push_str(&text);
                } else {
                    render
                        .text
                        .push_str(&if in_link > 0 || in_pre > 0 || skill_blocks == 0 {
                            text
                        } else {
                            skills(&text, skill_list)
                        });
                }
            }
            Event::Code(text) => {
                let shown = code_target(&text, cwd)
                    .map_or_else(|| text.into_string(), |target| label(&target, &suffixes));
                if let Some((description, _, _)) = &mut captured {
                    description.push_str(&shown);
                } else {
                    render.text.push_str(&shown);
                }
            }
            Event::SoftBreak => {
                if breaks {
                    render.flush();
                }
                render.text.push(' ');
            }
            Event::HardBreak | Event::Rule => render.flush(),
            Event::Html(text) | Event::InlineHtml(text) => render.html(&text, user),
            _ => {}
        }
    }
    render.flush();
    render.segments
}
fn github_alerts(source: &str, options: Options) -> String {
    let marker = regex(r"(?i)^( {0,3}> ?)(\[!(?:NOTE|TIP|IMPORTANT|WARNING|CAUTION)\])(?:\r?\n|$)");
    let mut replacements = vec![];
    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        if matches!(event, Event::Start(Tag::BlockQuote(_)))
            && let Some(capture) = marker.captures(&source[range.clone()])
            && let Some(full) = capture.get(0)
        {
            replacements.push((
                range.start..range.start + full.end(),
                format!("{}\n", &capture[1]),
            ));
        }
    }
    let mut rendered = source.to_owned();
    replacements.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    for (range, text) in replacements {
        rendered.replace_range(range, &text);
    }
    rendered
}
fn insight_markdown(source: &str) -> String {
    if !source.contains("★ Insight") {
        return source.into();
    }
    let protected: Vec<_> = Parser::new(source)
        .into_offset_iter()
        .filter_map(|(event, range)| {
            let code = matches!(
                &event,
                Event::Start(Tag::CodeBlock(_)) | Event::Html(_) | Event::InlineHtml(_)
            ) || (matches!(&event, Event::Code(_))
                && source[range.clone()].contains(['\r', '\n']));
            (code || matches!(event, Event::Start(Tag::List(_) | Tag::BlockQuote(_))))
                .then_some((range, code))
        })
        .collect();
    let opening = regex(r"^( {0,3})(`?)★ Insight[ \t]+─{3,}(`?)[ \t]*$");
    let closing = regex(r"^( {0,3})(`?)─{3,}(`?)[ \t]*$");
    let mut indent = None;
    let mut offset = 0;
    let mut rewritten = String::new();
    for line in source.split_inclusive('\n') {
        let value = line.trim_end_matches(['\r', '\n']);
        let ending = &line[value.len()..];
        let blocked = protected
            .iter()
            .any(|(range, code)| range.contains(&offset) && (indent.is_none() || *code));
        let opener = (!blocked && indent.is_none())
            .then(|| opening.captures(value))
            .flatten()
            .filter(|captures| captures[2] == captures[3]);
        let closer = (!blocked && indent.is_some())
            .then(|| closing.captures(value))
            .flatten()
            .filter(|captures| {
                captures[2] == captures[3] && indent.as_deref() == Some(&captures[1])
            });
        if let Some(opener) = opener {
            indent = Some(opener[1].to_owned());
            rewritten.push('>');
            rewritten.push_str(ending);
        } else if closer.is_some() {
            indent = None;
            rewritten.push_str(ending);
        } else {
            if indent.is_some() {
                rewritten.push_str("> ");
            }
            rewritten.push_str(line);
        }
        offset += line.len();
    }
    rewritten
}
pub fn rendered_segments(
    item: &Value,
    cwd: Option<&str>,
    skills: &[SearchThreadInputSkillsItem],
) -> Vec<String> {
    let kind = item["type"].as_str().unwrap_or("");
    let plan = kind == "proposed_plan";
    let user = kind == "user_message";
    let source = item[if plan { "markdown" } else { "text" }]
        .as_str()
        .unwrap_or("");
    let bootstrap = "[User attached one or more files without additional text. Respond using the conversation context and the attached files.]";
    if user && regex(r"(?i)^Ultrathink:\s*").replace(source, "") == bootstrap {
        return vec![];
    }
    let mut text = if user && item.get("context").is_none_or(Value::is_null) {
        legacy::upgrade(source)
    } else {
        source.into()
    };
    if !plan && !user && text.is_empty() && item["streaming"] != true {
        text = "(empty response)".into();
    }
    let mut title = None;
    if plan {
        let heading = regex(r"(?m)^\s{0,3}#{1,6}\s+(.+)$");
        title = Some(
            heading
                .captures(&text)
                .map_or("Proposed plan".to_owned(), |c| {
                    c[1].trim_matches(space).into()
                }),
        );
        let mut lines: Vec<_> = text
            .trim_end_matches(space)
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .collect();
        if lines.first().is_some_and(|line| heading.is_match(line)) {
            lines.remove(0);
        }
        while lines
            .first()
            .is_some_and(|line| line.trim_matches(space).is_empty())
        {
            lines.remove(0);
        }
        if lines.first().is_some_and(|line| {
            heading
                .captures(line)
                .is_some_and(|c| c[1].trim_matches(space).eq_ignore_ascii_case("summary"))
        }) {
            lines.remove(0);
            while lines
                .first()
                .is_some_and(|line| line.trim_matches(space).is_empty())
            {
                lines.remove(0);
            }
        }
        text = lines.join("\n");
    }
    let breaks = user || (!plan && regex(r"(?m)^★ Insight(?:\s|─)").is_match(&text));
    if !plan && !user {
        text = insight_markdown(&text);
    }
    let mut segments =
        markdown_segments_breaks(&text, user, cwd, if plan { &[] } else { skills }, breaks);
    if let Some(title) = title {
        segments.insert(0, title);
    }
    segments
}
impl NativeThreadFindText {
    pub fn segments(
        &self,
        item: &OrchestrationV2TurnItem,
        cwd: Option<&str>,
        skills: &[SearchThreadInputSkillsItem],
    ) -> Result<Vec<String>> {
        Ok(rendered_segments(&serde_json::to_value(item)?, cwd, skills))
    }
    pub fn occurrences(&self, text: &str, query: &str) -> u64 {
        occurrences(text, query)
    }
    pub fn embeds_result(&self, item: &OrchestrationV2TurnItem) -> bool {
        let Ok(value) = serde_json::to_value(item) else {
            return false;
        };
        value["type"] == "dynamic_tool"
            && ((value["status"] == "completed"
                && html_render_from_tool_item(&value).is_ok_and(|v| v.is_some()))
                || mcp_app_from_tool_item(&value).is_ok_and(|v| v.is_some()))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embeds_and_occurrences_match_executed_typescript() {
        let cases: Vec<Value> =
            serde_json::from_str(include_str!("../tests/fixtures/transport_find_embeds.json"))
                .unwrap();
        for case in cases {
            let item: OrchestrationV2TurnItem =
                serde_json::from_value(case["item"].clone()).unwrap();
            assert_eq!(
                NativeThreadFindText.embeds_result(&item),
                case["expected"].as_bool().unwrap(),
                "{}",
                case["item"]
            );
        }
        let cases: Vec<Value> = serde_json::from_str(include_str!(
            "../tests/fixtures/transport_find_occurrences.json"
        ))
        .unwrap();
        for case in cases {
            assert_eq!(
                occurrences(
                    case["text"].as_str().unwrap(),
                    case["query"].as_str().unwrap()
                ),
                case["expected"].as_u64().unwrap(),
                "{case}"
            );
        }
    }
    #[test]
    fn rendered_find_segments_match_typescript() {
        let cases: Vec<Value> =
            serde_json::from_str(include_str!("../tests/fixtures/transport_find_text.json"))
                .unwrap();
        let mut failures = vec![];
        for case in cases {
            let skills: Vec<SearchThreadInputSkillsItem> =
                serde_json::from_value(case["skills"].clone()).unwrap();
            let actual = rendered_segments(&case["item"], case["cwd"].as_str(), &skills);
            if serde_json::to_value(&actual).unwrap() != case["expected"] {
                failures.push(format!(
                    "{} cwd={}: actual={actual:?}, expected={}",
                    case["item"], case["cwd"], case["expected"]
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "{} mismatches:\n{}",
            failures.len(),
            failures
                .into_iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .take(40)
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}
