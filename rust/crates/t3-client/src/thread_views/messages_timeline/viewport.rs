//! Pure scroll/minimap helpers exported by the web timeline logic.
use serde::{Deserialize, Serialize};

pub const TIMELINE_MINIMAP_MIN_ITEMS: usize = 2;
pub const TIMELINE_MINIMAP_TARGET_OFFSET: f64 = 24.;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkGroupScrollAnchor {
    pub entry_id: String,
    pub offset: f64,
}
pub fn resolve_work_group_scroll_index(
    ids: &[&str],
    anchor: Option<&WorkGroupScrollAnchor>,
) -> Option<(usize, f64)> {
    let anchor = anchor?;
    ids.iter()
        .position(|id| *id == anchor.entry_id)
        .map(|i| (i, -anchor.offset))
}
pub fn should_follow_work_group_append(
    previous: &[&str],
    entries: &[&str],
    distance_from_end: f64,
) -> bool {
    !previous.is_empty()
        && entries.len() > previous.len()
        && distance_from_end <= 1.
        && entries.starts_with(previous)
}
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TimelineEndState {
    pub is_at_end: Option<bool>,
    pub content_length: Option<f64>,
    pub scroll: Option<f64>,
    pub scroll_length: Option<f64>,
}
pub fn resolve_timeline_is_at_end(state: Option<&TimelineEndState>) -> Option<bool> {
    let state = state?;
    match (state.content_length, state.scroll, state.scroll_length) {
        (Some(c), Some(s), Some(l)) => Some(c - s - l <= 40.),
        _ => state.is_at_end,
    }
}
pub fn resolve_timeline_minimap_height_style(item_count: usize) -> String {
    format!(
        "min({}px, max(4rem, calc(100% - 14.75rem)))",
        item_count.saturating_sub(1).saturating_mul(8).max(1)
    )
}
pub fn resolve_timeline_minimap_top_percent(index: f64, item_count: usize) -> f64 {
    if item_count <= 1 {
        0.
    } else {
        index.max(0.).min((item_count - 1) as f64) / (item_count - 1) as f64 * 100.
    }
}
pub fn resolve_timeline_minimap_index_from_pointer(
    item_count: usize,
    rail_top: f64,
    rail_height: f64,
    pointer_y: f64,
) -> Option<usize> {
    if item_count == 0 || rail_height <= 0. {
        None
    } else if item_count == 1 {
        Some(0)
    } else {
        Some(
            (((pointer_y - rail_top) / rail_height).clamp(0., 1.) * (item_count - 1) as f64).round()
                as usize,
        )
    }
}
pub fn resolve_timeline_minimap_visible_range(
    start_buffered: Option<usize>,
    end_buffered: Option<usize>,
    scroll: f64,
    scroll_length: f64,
    position_at_index: impl Fn(usize) -> Option<f64>,
    size_at_index: impl Fn(usize) -> Option<f64>,
) -> (Option<usize>, Option<usize>) {
    let (Some(start), Some(end)) = (start_buffered, end_buffered) else {
        return (None, None);
    };
    let mut first = None;
    let mut last = None;
    for i in start..=end {
        if let Some(top) = position_at_index(i).filter(|n| n.is_finite())
            && top < scroll + scroll_length
            && top
                + size_at_index(i)
                    .filter(|n| n.is_finite())
                    .unwrap_or(1.)
                    .max(1.)
                > scroll
        {
            first.get_or_insert(i);
            last = Some(i);
        }
    }
    (first, last)
}
pub fn is_timeline_minimap_row_visible(
    row_index: usize,
    start: Option<usize>,
    end: Option<usize>,
) -> bool {
    start
        .zip(end)
        .is_some_and(|(a, b)| row_index >= a && row_index <= b)
}
pub fn resolve_timeline_minimap_current_index(
    start: Option<usize>,
    end: Option<usize>,
    indices: &[usize],
) -> Option<usize> {
    let (start, end) = start.zip(end)?;
    let mut preceding = None;
    for (i, row) in indices.iter().enumerate() {
        if *row >= start {
            return if *row <= end { Some(i) } else { preceding };
        }
        preceding = Some(i);
    }
    preceding
}
fn side_gutter(viewport: f64, content: f64) -> f64 {
    if !viewport.is_finite() || viewport <= 0. || !content.is_finite() {
        0.
    } else {
        ((viewport - viewport.min(content)) / 2.).max(0.)
    }
}
pub fn resolve_timeline_minimap_has_persistent_gutter(viewport: f64, content: f64) -> bool {
    side_gutter(viewport, content) >= 48.
}
pub fn resolve_timeline_minimap_hit_strip_width(viewport: f64, content: f64) -> f64 {
    (side_gutter(viewport, content).floor() - 12.).clamp(0., 40.)
}
pub fn resolve_timeline_minimap_navigation_interactive(collapsed_width: f64) -> bool {
    collapsed_width >= 14.
}

pub(super) fn render_directives_for_copy(text: &str) -> String {
    let mut rendered = String::new();
    let mut at = 0;
    let mut code = None;
    let mut fence: Option<(u8, usize)> = None;
    let mut indented = false;
    while at < text.len() {
        let tail = &text[at..];
        let line_start = at == 0 || text.as_bytes()[at - 1] == b'\n';
        if line_start {
            let end = tail.find('\n').unwrap_or(tail.len());
            let line = &tail[..end];
            let trimmed = line.trim_start_matches(' ');
            let indent = line.len() - trimmed.len();
            let marker = trimmed.as_bytes().first().copied();
            let count = marker.map_or(0, |marker| {
                trimmed.bytes().take_while(|c| *c == marker).count()
            });
            if let Some((mark, n)) = fence {
                if indent <= 3
                    && marker == Some(mark)
                    && count >= n
                    && trimmed[count..].trim().is_empty()
                {
                    fence = None;
                }
                rendered.push_str(line);
                at += end;
                if at < text.len() {
                    rendered.push('\n');
                    at += 1;
                }
                continue;
            }
            if indent <= 3 && matches!(marker, Some(b'`' | b'~')) && count >= 3 {
                fence = marker.map(|m| (m, count));
                rendered.push_str(line);
                at += end;
                if at < text.len() {
                    rendered.push('\n');
                    at += 1;
                }
                continue;
            }
            indented = indent >= 4 || line.starts_with('\t');
        }
        if tail.starts_with('`') && !indented {
            let count = tail.bytes().take_while(|b| *b == b'`').count();
            if code == Some(count) {
                code = None;
            } else if code.is_none()
                && text[at + count..]
                    .match_indices(&"`".repeat(count))
                    .any(|(i, _)| {
                        let i = at + count + i;
                        (i == 0 || text.as_bytes()[i - 1] != b'`')
                            && text.as_bytes().get(i + count) != Some(&b'`')
                    })
            {
                code = Some(count);
            }
            rendered.push_str(&tail[..count]);
            at += count;
            continue;
        }
        let escaped =
            at > 0 && text[..at].bytes().rev().take_while(|b| *b == b'\\').count() % 2 == 1;
        let directive = [
            "::artifact-template",
            ":codex-file-citation",
            ":codex-followup",
        ]
        .into_iter()
        .find(|name| tail.starts_with(name));
        if code.is_none()
            && !indented
            && !escaped
            && (at == 0 || text.as_bytes()[at - 1] != b':')
            && let Some(name) = directive
            && let Some((end, replacement)) = copy_directive(tail, name)
            && (name != "::artifact-template"
                || (text[..at]
                    .rsplit('\n')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .is_empty()
                    && tail[end..]
                        .split('\n')
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .is_empty()))
            && (name != ":codex-file-citation" || !inside_link(text, at))
        {
            rendered.push_str(&replacement);
            at += end;
            continue;
        }
        let c = tail.chars().next().unwrap_or_default();
        rendered.push(c);
        at += c.len_utf8();
    }
    rendered
}
fn inside_link(text: &str, at: usize) -> bool {
    let mut starts = vec![];
    for (i, c) in text.char_indices() {
        if i > at && starts.is_empty() {
            break;
        }
        let escaped = text[..i].bytes().rev().take_while(|b| *b == b'\\').count() % 2 == 1;
        if escaped {
            continue;
        }
        if c == '[' {
            starts.push(i);
        } else if c == ']'
            && let Some(start) = starts.pop()
            && start < at
            && i > at
        {
            let rest = &text[i + 1..];
            if rest.starts_with('(') || rest.starts_with('[') {
                return true;
            }
            let label = &text[start + 1..i];
            if text
                .lines()
                .any(|line| line.trim_start().starts_with(&format!("[{label}]:")))
            {
                return true;
            }
        }
    }
    false
}
fn copy_directive(tail: &str, name: &str) -> Option<(usize, String)> {
    let mut cursor = name.len();
    let mut label = None;
    if tail[cursor..].starts_with('[') {
        let end = tail[cursor + 1..].find(']')? + cursor + 1;
        label = Some(&tail[cursor + 1..end]);
        cursor = end + 1;
    }
    if !tail[cursor..].starts_with('{') {
        return None;
    }
    cursor += 1;
    let mut attributes = std::collections::BTreeMap::new();
    let mut closed = false;
    while cursor < tail.len() {
        while tail
            .as_bytes()
            .get(cursor)
            .is_some_and(u8::is_ascii_whitespace)
        {
            cursor += 1;
        }
        if tail[cursor..].starts_with('}') {
            cursor += 1;
            closed = true;
            break;
        }
        let start = cursor;
        while tail
            .as_bytes()
            .get(cursor)
            .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            cursor += 1;
        }
        if cursor == start {
            return None;
        }
        let key = &tail[start..cursor];
        if tail.as_bytes().get(cursor) != Some(&b'=') {
            return None;
        }
        cursor += 1;
        let quote = *tail.as_bytes().get(cursor)?;
        let start = if matches!(quote, b'\'' | b'"') {
            cursor += 1;
            cursor
        } else {
            cursor
        };
        while let Some(c) = tail.as_bytes().get(cursor) {
            if matches!(quote, b'\'' | b'"') {
                if *c == quote {
                    break;
                }
            } else if c.is_ascii_whitespace() || *c == b'}' {
                break;
            }
            cursor += 1;
        }
        attributes.insert(key, &tail[start..cursor]);
        if matches!(quote, b'\'' | b'"') {
            cursor += 1;
        }
    }
    if !closed {
        return None;
    }
    let replacement = match name {
        ":codex-file-citation" => {
            let path = attributes.get("path")?.trim();
            if path.trim().is_empty() {
                return None;
            }
            let normalized = path.replace('\\', "/");
            let normalized = normalized.trim_end_matches('/');
            let label = normalized
                .rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or(if normalized.is_empty() {
                    "File"
                } else {
                    normalized
                });
            let mut href = path
                .replace('%', "%25")
                .replace('#', "%23")
                .replace('?', "%3F");
            if let Some(line) = attributes
                .get("line_range_start")
                .and_then(|s| s.trim().parse::<f64>().ok())
                .filter(|n| {
                    n.is_finite() && *n > 0. && n.fract() == 0. && *n <= 9_007_199_254_740_991.
                })
            {
                href.push_str(&format!("#L{line:.0}"));
            }
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
            format!(
                "[{}](<{}>)",
                label,
                href.replace('\\', "%5C")
                    .replace('<', "%3C")
                    .replace('>', "%3E")
                    .replace('\r', "%0D")
                    .replace('\n', "%0A")
            )
        }
        ":codex-followup" => {
            attributes.get("prompt").filter(|p| !p.trim().is_empty())?;
            label.filter(|l| !l.trim().is_empty())?.into()
        }
        "::artifact-template" => {
            attributes
                .get("skill_name")
                .filter(|s| s.starts_with("artifact-template-"))?;
            let directory = attributes.get("skill_directory")?;
            let b = directory.as_bytes();
            let drive = b.len() >= 3
                && b[0].is_ascii_alphabetic()
                && b[1] == b':'
                && matches!(b[2], b'/' | b'\\');
            let unc = directory
                .strip_prefix("\\\\")
                .map(|s| s.split('\\'))
                .or_else(|| directory.strip_prefix("//").map(|s| s.split('/')))
                .is_some_and(|mut parts| {
                    parts.next().is_some_and(|s| !s.is_empty())
                        && parts.next().is_some_and(|s| !s.is_empty())
                });
            if !(directory.starts_with('/') && !directory.starts_with("//")) && !drive && !unc {
                return None;
            }
            if attributes
                .get("gallery_kind")
                .is_some_and(|s| !matches!(*s, "imagegen" | "product-design"))
            {
                return None;
            }
            let display = attributes
                .get("display_name")
                .filter(|s| !s.trim().is_empty())?;
            let kind = match *attributes.get("artifact_kind")? {
                "document" => "Document template",
                "spreadsheet" => "Spreadsheet template",
                "presentation" => "Presentation template",
                "site" => "Site template",
                "google-docs" => "Google Doc template",
                "google-slides" => "Google Slides template",
                "google-sheets" => "Google Sheet template",
                "image" => "Image template",
                "email" => "Email template",
                "slack" => "Slack template",
                _ => return None,
            };
            format!("{} ({kind})", display.trim())
        }
        _ => return None,
    };
    Some((cursor, replacement))
}
