use super::{regex, space};
use serde_json::{Value, json};
use std::collections::BTreeMap;

struct Entry {
    header: String,
    body: String,
}
fn entries(block: &str) -> Option<Vec<Entry>> {
    let mut entries: Vec<Entry> = vec![];
    for line in block.split('\n') {
        if let Some(header) = line
            .strip_prefix("- ")
            .and_then(|line| line.strip_suffix(':'))
            .filter(|line| !line.is_empty())
        {
            entries.push(Entry {
                header: header.into(),
                body: String::new(),
            });
        } else if let Some(entry) = entries.last_mut() {
            if let Some(line) = line.strip_prefix("  ") {
                if !entry.body.is_empty() {
                    entry.body.push('\n');
                }
                entry.body.push_str(line);
            } else if line.trim_matches(space).is_empty() {
                entry.body.push('\n');
            } else {
                return None;
            }
        } else if !line.trim_matches(space).is_empty() {
            return None;
        }
    }
    for entry in &mut entries {
        entry
            .body
            .truncate(entry.body.trim_end_matches(space).len());
    }
    Some(entries)
}
fn base(kind: &str, index: usize, label: &str) -> Value {
    json!({"version":1,"contextId":format!("legacy_{kind}_{index}"),"kind":kind,"label":label})
}
fn terminal(entry: &Entry, index: usize) -> Option<Value> {
    let header =
        regex(r"^(.*?) (?:line ([0-9]+)|lines ([0-9]+)-([0-9]+))$").captures(&entry.header)?;
    let label = header[1].trim_matches(space);
    if label.is_empty() {
        return None;
    }
    let start: u64 = header.get(2).or(header.get(3))?.as_str().parse().ok()?;
    let end: u64 = header.get(2).or(header.get(4))?.as_str().parse().ok()?;
    let mut record = base("terminal", index, &entry.header);
    record["terminalId"] = regex(r"\s+")
        .replace_all(&label.to_lowercase(), "-")
        .into_owned()
        .into();
    record["terminalLabel"] = label.into();
    record["lineStart"] = start.into();
    record["lineEnd"] = end.into();
    record["text"] = entry
        .body
        .split('\n')
        .map(|line| regex(r"^[0-9]+ \| ?").replace(line, "").into_owned())
        .collect::<Vec<_>>()
        .join("\n")
        .into();
    Some(record)
}
fn element(entry: &Entry, index: usize) -> Option<Value> {
    let header = regex(r"^<([^>]+)>").captures(&entry.header)?;
    let inner = &header[1];
    let mut fields = BTreeMap::new();
    let mut sections = BTreeMap::<String, Vec<String>>::new();
    let mut section: Option<String> = None;
    for line in entry.body.split('\n') {
        if let Some(key) = &section
            && (line.starts_with("  ") || line.is_empty())
        {
            sections
                .entry(key.clone())
                .or_default()
                .push(line.get(2..).unwrap_or_default().into());
            continue;
        }
        section = None;
        if let Some(field) = regex(r"^([a-z]+): (.*)$").captures(line) {
            fields.insert(field[1].to_owned(), field[2].to_owned());
        } else if let Some(key) = line
            .strip_suffix(':')
            .filter(|key| matches!(*key, "html" | "styles"))
        {
            section = Some(key.to_owned());
            sections.insert(key.into(), vec![]);
        }
    }
    let mut record = base("element", index, &format!("<{inner}>"));
    record["pageUrl"] = fields.get("url").cloned().unwrap_or_default().into();
    record["pageTitle"] = Value::Null;
    record["tagName"] = inner.to_lowercase().into();
    record["selector"] = fields.get("selector").cloned().into();
    record["htmlPreview"] = sections
        .get("html")
        .map(|lines| lines.join("\n"))
        .unwrap_or_default()
        .trim_end_matches(space)
        .into();
    record["styles"] = sections
        .get("styles")
        .map(|lines| lines.join("\n"))
        .unwrap_or_default()
        .trim_end_matches(space)
        .into();
    record["componentName"] = inner
        .chars()
        .any(|c| c.is_ascii_uppercase())
        .then_some(inner)
        .into();
    record["source"] = fields.get("source").and_then(|source| {
        let parsed=regex(r"^(.*?)(?::([0-9]+))?(?::([0-9]+))?$").captures(source)?;
        if parsed[1].is_empty(){return None;}
        Some(json!({"functionName":null,"fileName":&parsed[1],"lineNumber":parsed.get(2).and_then(|part|part.as_str().parse::<u64>().ok()),"columnNumber":parsed.get(3).and_then(|part|part.as_str().parse::<u64>().ok())}))
    }).unwrap_or(Value::Null);
    Some(record)
}
fn preview(body: &str, index: usize) -> Option<Value> {
    let lines: Vec<_> = body.split('\n').collect();
    let read = |prefix: &str| {
        let Some(start) = lines.iter().position(|line| line.starts_with(prefix)) else {
            return String::new();
        };
        let end=(start+1..lines.len()).find(|index| {
            let line=lines[*index];
            ["Preview annotation:","Id: ","Page: ","Comment: ","Targets: "].iter().any(|prefix|line.starts_with(prefix))
                || line=="Requested visual changes:" || line=="The attached screenshot is the annotated preview crop."
                || regex(r"^</?(?:terminal_context|element_context|preview_annotation|review_comment)\b").is_match(line)
        }).unwrap_or(lines.len());
        std::iter::once(lines[start].strip_prefix(prefix).unwrap_or_default())
            .chain(lines[start + 1..end].iter().copied())
            .collect::<Vec<_>>()
            .join("\n")
            .trim_matches(space)
            .into()
    };
    let page = read("Page: ");
    let page_url = regex(r"(?i)^https?://").is_match(&page);
    let mut elements = vec![];
    for capture in regex(r"(?s)<element_context>\n(.*?)\n</element_context>").captures_iter(body) {
        for entry in entries(&capture[1])? {
            let mut record = element(&entry, elements.len() + 1)?;
            for field in ["version", "contextId", "kind", "label"] {
                record.as_object_mut()?.remove(field);
            }
            elements.push(record);
        }
    }
    let mut record = base(
        "preview-annotation",
        index,
        if page.is_empty() {
            "Preview annotation"
        } else {
            &page
        },
    );
    record["annotationId"] = read("Id: ").into();
    record["pageUrl"] = if page_url {
        page.clone().into()
    } else {
        elements
            .first()
            .map(|element| element["pageUrl"].clone())
            .unwrap_or_else(|| "".into())
    };
    record["pageTitle"] = (!page_url && !page.is_empty()).then_some(page).into();
    record["comment"] = read("Comment: ").into();
    record["targetSummary"] = read("Targets: ").into();
    let changes: Vec<_> = lines
        .iter()
        .position(|line| *line == "Requested visual changes:")
        .into_iter()
        .flat_map(|index| {
            lines[index + 1..]
                .iter()
                .take_while(|line| line.starts_with("- "))
                .map(|line| &line[2..])
        })
        .collect();
    record["styleChanges"] = json!(changes);
    if !elements.is_empty() {
        record["elements"] = elements.into();
    }
    Some(record)
}
fn valid(record: &Value) -> bool {
    serde_json::from_value::<t3_contracts::composer_context::ComposerContextRecord>(record.clone())
        .is_ok()
}
fn reference(record: &Value) -> String {
    let raw_label = record["label"].as_str().unwrap_or_default();
    let label = regex(r"\s+")
        .replace_all(&raw_label.replace(['[', ']', '\\', '\r', '\n'], " "), " ")
        .trim_matches(space)
        .to_owned();
    let label: String = label
        .chars()
        .scan(0, |units, c| {
            *units += c.len_utf16();
            (*units <= t3_contracts::composer_context::COMPOSER_CONTEXT_LABEL_MAX_CHARS)
                .then_some(c)
        })
        .collect();
    let kind = record["kind"].as_str().unwrap_or_default();
    format!(
        "[{}](t3-context://v1/{}/{})",
        if label.is_empty() { kind } else { &label },
        kind,
        record["contextId"].as_str().unwrap_or_default()
    )
}
fn review(attributes: &str, body: &str, index: usize) -> Option<Value> {
    let fields: BTreeMap<_, _> = regex(r#"([a-zA-Z][a-zA-Z0-9_-]*)="([^"]*)""#)
        .captures_iter(attributes)
        .map(|capture| {
            (
                capture[1].to_owned(),
                capture[2]
                    .replace("&lt;", "<")
                    .replace("&gt;", ">")
                    .replace("&quot;", "\"")
                    .replace("&amp;", "&"),
            )
        })
        .collect();
    let path = fields.get("filePath")?.trim_matches(space);
    let section = fields.get("sectionId")?.trim_matches(space);
    let integer = |key| {
        fields
            .get(key)
            .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|value| value.parse::<u64>().ok())
    };
    let start = integer("startIndex")?;
    let end = integer("endIndex")?;
    if path.is_empty() || section.is_empty() {
        return None;
    }
    let range = fields
        .get("rangeLabel")
        .map(|range| range.trim_matches(space))
        .filter(|range| !range.is_empty())
        .unwrap_or("line");
    let mut record = base(
        "review-comment",
        index,
        &format!(
            "{} {range}",
            path.rsplit(['/', '\\']).next().unwrap_or(path)
        ),
    );
    record["sectionId"] = section.into();
    record["sectionTitle"] = fields
        .get("sectionTitle")
        .map(|title| title.trim_matches(space))
        .filter(|title| !title.is_empty())
        .unwrap_or("Review")
        .into();
    record["filePath"] = path.into();
    record["startIndex"] = start.min(end).into();
    record["endIndex"] = start.max(end).into();
    record["rangeLabel"] = range.into();
    let mut last = None;
    let mut cursor = 0;
    for opening in regex(r"(?m)^(`{3,})([^\s`]*)[^\n]*\n").captures_iter(body) {
        let matched = opening.get(0)?;
        if matched.start() < cursor {
            continue;
        }
        let closer = format!("\n{}", &opening[1]);
        if let Some(close) = body[matched.end()..].find(&closer) {
            last = Some((
                matched.start(),
                matched.end(),
                matched.end() + close,
                opening[2].to_owned(),
            ));
            cursor = matched.end() + close + closer.len();
        }
    }
    record["text"] = body[..last.as_ref().map_or(body.len(), |fence| fence.0)]
        .trim_matches(space)
        .into();
    record["diff"] = last
        .as_ref()
        .map_or("", |fence| &body[fence.1..fence.2])
        .into();
    record["fenceLanguage"] = last
        .map(|fence| fence.3)
        .filter(|language| !language.trim_matches(space).is_empty())
        .unwrap_or_else(|| "diff".into())
        .into();
    valid(&record).then_some(record)
}

pub(super) fn upgrade(source: &str) -> String {
    if !regex(r"<(?:terminal_context|element_context|preview_annotation|review_comment)\b|\u{fffc}")
        .is_match(source)
    {
        return source.into();
    }
    let mut token = "\u{e000}".to_owned();
    while source.contains(&token) {
        token.push_str(&token.clone());
    }
    let mut protected = vec![];
    for tag in ["terminal_context", "element_context", "preview_annotation"] {
        for (start, _) in source.match_indices(&format!("<{tag}>\n")) {
            if start > 0 && source.as_bytes()[start - 1] != b'\n' {
                continue;
            }
            if let Some(end) = source[start..].find(&format!("\n</{tag}>")) {
                protected.push(start..start + end + tag.len() + 4);
            }
        }
    }
    let mut reviews = vec![];
    let mut rest = String::new();
    let mut consumed = 0;
    for opening in regex(r"<review_comment\b([^>]*)>").captures_iter(source) {
        let Some(matched) = opening.get(0) else {
            continue;
        };
        if matched.start() < consumed
            || protected
                .iter()
                .any(|range| range.contains(&matched.start()))
        {
            continue;
        }
        let mut fence = 0;
        let mut boundary_end = None;
        for boundary in regex(r"(?m)^(`{3,})([^\n]*)$|</review_comment>")
            .captures_iter(&source[matched.end()..])
        {
            let Some(found) = boundary.get(0) else {
                continue;
            };
            if let Some(ticks) = boundary.get(1) {
                if fence == 0 {
                    fence = ticks.len();
                } else if ticks.len() >= fence && boundary[2].trim_matches(space).is_empty() {
                    fence = 0;
                }
            } else if fence == 0 {
                boundary_end = Some((matched.end() + found.start(), matched.end() + found.end()));
                break;
            }
        }
        let Some((body_end, end)) = boundary_end else {
            break;
        };
        if let Some(record) = review(
            &opening[1],
            &source[matched.end()..body_end],
            reviews.len() + 1,
        ) {
            rest.push_str(&source[consumed..matched.start()]);
            rest.push_str(&format!("{token}{}{token}", reviews.len()));
            consumed = end;
            reviews.push(record);
        }
    }
    rest.push_str(&source[consumed..]);
    let trailing_reviews = rest.clone();
    loop {
        let trimmed = rest.trim_end_matches(space);
        let Some(tail) = trimmed.strip_suffix(&token) else {
            break;
        };
        let Some(at) = tail.rfind(&token) else {
            break;
        };
        let index = &tail[at + token.len()..];
        if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
            break;
        }
        rest.truncate(at);
    }
    if !["terminal_context", "element_context", "preview_annotation"]
        .iter()
        .any(|tag| {
            rest.trim_end_matches(space)
                .ends_with(&format!("\n</{tag}>"))
        })
    {
        rest = trailing_reviews;
    }
    let mut records = vec![];
    let mut terminals = vec![];
    loop {
        let trimmed = rest.trim_end_matches(space);
        let mut peeled = None;
        for tag in ["preview_annotation", "element_context", "terminal_context"] {
            let ending = format!("\n</{tag}>");
            if !trimmed.ends_with(&ending) {
                continue;
            }
            let opening = format!("<{tag}>\n");
            let Some(start) = trimmed.rfind(&opening) else {
                continue;
            };
            let body = &trimmed[start + opening.len()..trimmed.len() - ending.len()];
            let parsed = if tag == "preview_annotation" {
                preview(body, records.len() + 1).map(|record| vec![record])
            } else {
                entries(body).and_then(|entries| {
                    if entries.is_empty() {
                        None
                    } else {
                        entries
                            .iter()
                            .enumerate()
                            .map(|(index, entry)| {
                                if tag == "terminal_context" {
                                    terminal(entry, index + 1)
                                } else {
                                    element(entry, index + 1)
                                }
                            })
                            .collect::<Option<Vec<_>>>()
                    }
                })
            };
            let Some(parsed) = parsed.filter(|records| records.iter().all(valid)) else {
                return source.into();
            };
            if tag == "terminal_context" {
                terminals.splice(0..0, parsed.clone());
            }
            records.extend(parsed);
            peeled = Some(trimmed[..start].trim_end_matches('\n').to_owned());
            break;
        }
        let Some(peeled) = peeled else {
            break;
        };
        rest = peeled;
    }
    if records.len() + reviews.len() > 200 {
        return source.into();
    }
    for (index, review) in reviews.iter().enumerate() {
        rest = rest.replace(&format!("{token}{index}{token}"), &reference(review));
    }
    let mut placed = 0;
    rest = rest
        .chars()
        .map(|c| {
            if c == '\u{fffc}' {
                let replacement = terminals.get(placed).map(reference).unwrap_or_default();
                placed += 1;
                replacement
            } else {
                c.to_string()
            }
        })
        .collect();
    for terminal in terminals.iter().skip(placed) {
        let label = terminal["terminalLabel"].as_str().unwrap_or_default();
        let label = regex(r"\s+")
            .replace_all(&label.trim_matches(space).to_lowercase(), "-")
            .into_owned();
        let start = terminal["lineStart"].as_u64().unwrap_or(0);
        let end = terminal["lineEnd"].as_u64().unwrap_or(0);
        let label = format!(
            "@{label}:{start}{}",
            if start == end {
                String::new()
            } else {
                format!("-{end}")
            }
        );
        if let Some(at) = rest.match_indices(&label).find_map(|(at, _)| {
            let before = &rest[..at];
            let after = &rest[at + label.len()..];
            (!regex(r"[\p{L}\p{N}\p{M}_@.\-]$").is_match(before)
                && !regex(r"^(?:[\p{L}\p{N}\p{M}_-]|[.@]+[\p{L}\p{N}\p{M}_-])").is_match(after))
            .then_some(at)
        }) {
            rest.replace_range(at..at + label.len(), &reference(terminal));
        }
    }
    rest.trim_end_matches(space).into()
}
