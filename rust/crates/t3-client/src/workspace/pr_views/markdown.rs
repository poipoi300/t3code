use std::{collections::BTreeMap, rc::Rc};

/// Parser-neutral syntax tree; code nodes have no text children.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarkdownNode {
    pub kind: String,
    pub value: Option<String>,
    pub url: Option<String>,
    pub autolink_kind: Option<String>,
    pub children: Vec<MarkdownNode>,
    pub source_range: Option<std::ops::Range<usize>>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceAutolink {
    pub index: usize,
    pub text: String,
    pub label: String,
    pub url: String,
    pub kind: String,
}
pub fn remark_autolinks(
    tree: &mut MarkdownNode,
    autolinks: &impl Fn(&str) -> Vec<ReferenceAutolink>,
) {
    let children = std::mem::take(&mut tree.children);
    for mut child in children {
        if matches!(child.kind.as_str(), "link" | "linkReference") {
            tree.children.push(child);
            continue;
        }
        let Some(value) = child.value.as_deref().filter(|_| child.kind == "text") else {
            remark_autolinks(&mut child, autolinks);
            tree.children.push(child);
            continue;
        };
        let links = autolinks(value);
        if links.is_empty() {
            tree.children.push(child);
            continue;
        }
        let mut start = 0;
        for link in links {
            let end = link.index.saturating_add(link.text.len());
            if link.index < start
                || !value.is_char_boundary(link.index)
                || !value.is_char_boundary(end)
            {
                continue;
            }
            if start < link.index {
                tree.children.push(text_node(&value[start..link.index]));
            }
            tree.children.push(MarkdownNode {
                kind: "link".into(),
                url: Some(link.url),
                autolink_kind: Some(link.kind),
                children: vec![text_node(&link.label)],
                ..Default::default()
            });
            start = end;
        }
        if start < value.len() {
            tree.children.push(text_node(&value[start..]));
        }
    }
}
fn text_node(text: &str) -> MarkdownNode {
    MarkdownNode {
        kind: "text".into(),
        value: Some(text.into()),
        ..Default::default()
    }
}
pub fn remark_media_urls(tree: &mut MarkdownNode, resolve: &impl Fn(&str) -> String) {
    if matches!(tree.kind.as_str(), "image" | "definition")
        && let Some(url) = &tree.url
    {
        tree.url = Some(resolve(url));
    }
    for child in &mut tree.children {
        remark_media_urls(child, resolve);
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownPlugins {
    pub provider: String,
    pub repository_url: String,
    pub autolinks: bool,
}
#[derive(Default)]
pub struct MarkdownPluginCache {
    entries: BTreeMap<(String, String), Rc<MarkdownPlugins>>,
}
impl MarkdownPluginCache {
    pub fn plugins(
        &mut self,
        provider: &str,
        repository_url: Option<&str>,
        autolinks: bool,
    ) -> Option<Rc<MarkdownPlugins>> {
        let repository = repository_url?;
        Some(
            self.entries
                .entry((provider.into(), repository.into()))
                .or_insert_with(|| {
                    Rc::new(MarkdownPlugins {
                        provider: provider.into(),
                        repository_url: repository.into(),
                        autolinks,
                    })
                })
                .clone(),
        )
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HtmlNode {
    Text(String),
    Element {
        tag: String,
        alt: Option<String>,
        children: Vec<HtmlNode>,
    },
    Root(Vec<HtmlNode>),
    Ignored,
}
fn preview_text(node: &HtmlNode, output: &mut String) {
    match node {
        HtmlNode::Text(text) => output.push_str(text),
        HtmlNode::Element { tag, alt, children } => {
            if tag == "img" {
                if let Some(alt) = alt {
                    output.push_str(alt);
                }
                return;
            }
            let block = [
                "blockquote",
                "br",
                "dd",
                "details",
                "div",
                "dl",
                "dt",
                "h1",
                "h2",
                "h3",
                "h4",
                "h5",
                "h6",
                "hr",
                "li",
                "ol",
                "p",
                "pre",
                "summary",
                "table",
                "td",
                "th",
                "tr",
                "ul",
            ]
            .contains(&tag.as_str());
            if block {
                output.push(' ');
            }
            for child in children {
                preview_text(child, output);
            }
            if block {
                output.push(' ');
            }
        }
        HtmlNode::Root(children) => {
            for child in children {
                preview_text(child, output);
            }
        }
        HtmlNode::Ignored => {}
    }
}
pub fn comment_preview(tree: &HtmlNode) -> String {
    let mut output = String::new();
    preview_text(tree, &mut output);
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}
pub trait PullRequestMarkdownParser {
    fn parse(&self, body: &str) -> MarkdownNode;
    fn preview_tree(&self, body: &str) -> HtmlNode;
}
pub fn pull_request_comment_preview(body: &str, parser: &impl PullRequestMarkdownParser) -> String {
    comment_preview(&parser.preview_tree(body))
}
pub fn readable_body(raw: &str, parser: &impl PullRequestMarkdownParser) -> Option<String> {
    super::detail::visible_body(raw)?;
    let body = raw.replace("\r\n", "\n").replace('\r', "\n");
    fn cuts(node: &MarkdownNode, result: &mut Vec<std::ops::Range<usize>>) {
        if node.kind == "html"
            && let Some(range) = &node.source_range
        {
            result.push(range.clone());
        }
        for child in &node.children {
            cuts(child, result);
        }
    }
    let mut ranges = Vec::new();
    cuts(&parser.parse(&body), &mut ranges);
    let mut output = String::new();
    let mut from = 0;
    for range in ranges {
        if range.start < from
            || range.end < range.start
            || !body.is_char_boundary(range.start)
            || !body.is_char_boundary(range.end)
        {
            continue;
        }
        output.push_str(&body[from..range.start]);
        let mut html = String::new();
        let mut tail = &body[range.clone()];
        while let Some(start) = tail.find("<!--") {
            html.push_str(&tail[..start]);
            let Some(end) = tail[start + 4..].find("-->") else {
                tail = "";
                break;
            };
            tail = &tail[start + 4 + end + 3..];
        }
        html.push_str(tail);
        let prefix = output.rsplit('\n').next().unwrap_or_default();
        let suffix = &body[range.end..];
        let suffix_trimmed = suffix.trim_start_matches([' ', '\t']);
        let own_line = prefix.chars().all(|ch| matches!(ch, ' ' | '\t' | '>'))
            && (suffix_trimmed.is_empty() || suffix_trimmed.starts_with('\n'));
        if own_line && html.trim().is_empty() {
            output.truncate(output.trim_end_matches([' ', '\t', '>']).len());
            from = range.end
                + suffix_trimmed
                    .strip_prefix('\n')
                    .map_or(0, |_| suffix.len() - suffix_trimmed.len() + 1);
        } else {
            output.push_str(&html);
            from = range.end;
        }
    }
    output.push_str(&body[from..]);
    let output = output.trim();
    (!output.is_empty()).then(|| output.to_owned())
}
pub fn markdown_excerpt(tree: &MarkdownNode) -> Option<String> {
    fn visit(node: &MarkdownNode, words: &mut Vec<String>) {
        if matches!(node.kind.as_str(), "text" | "inlineCode")
            && let Some(value) = &node.value
        {
            words.push(value.clone());
        }
        if node.kind != "code" {
            for child in &node.children {
                visit(child, words);
            }
        }
    }
    let mut words = Vec::new();
    visit(tree, &mut words);
    let mut text = words.join(" ");
    for marker in [
        "[!note]",
        "[!tip]",
        "[!important]",
        "[!warning]",
        "[!caution]",
    ] {
        while let Some(index) = text.to_ascii_lowercase().find(marker) {
            text.replace_range(index..index + marker.len(), "");
        }
    }
    text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return None;
    }
    Some(bound_utf16(&text, 280, "…"))
}
pub fn bound_utf16(value: &str, max: usize, suffix: &str) -> String {
    if value.encode_utf16().count() <= max {
        return value.into();
    }
    let room = max.saturating_sub(suffix.encode_utf16().count());
    let mut used = 0;
    let prefix: String = value
        .chars()
        .take_while(|ch| {
            used += ch.len_utf16();
            used <= room
        })
        .collect();
    format!("{prefix}{suffix}")
}
