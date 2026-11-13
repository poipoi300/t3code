use crate::{
    CancellationToken,
    prompts::{attachment_id, attachment_name},
    text::{limit_title_message, utf16_len},
};
use async_trait::async_trait;
use std::{collections::BTreeMap, path::Path, time::Duration};
use t3_contracts::chat_attachment::ChatAttachment;

pub struct TitleMessage {
    pub role: TitleRole,
    pub text: String,
    pub attachments: Vec<ChatAttachment>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TitleRole {
    User,
    Assistant,
    System,
    Reasoning,
}
pub struct TitleContext {
    pub message: String,
    pub attachments: Vec<ChatAttachment>,
}
fn plain_citations(text: &str) -> String {
    let pattern = regex::Regex::new(r"\[Assistant quote\]\((t3-citation://v1/[^\s)]+)\)")
        .expect("citation pattern");
    pattern
        .replace_all(text, |captures: &regex::Captures<'_>| {
            let source = &captures[0];
            let Ok(url) = reqwest::Url::parse(&captures[1]) else {
                return source.to_owned();
            };
            if !url.username().is_empty()
                || url.password().is_some()
                || url.port().is_some()
                || url.fragment().is_some()
            {
                return source.into();
            }
            let parts = url
                .path()
                .trim_start_matches('/')
                .split('/')
                .collect::<Vec<_>>();
            if parts.len() != 3 {
                return source.into();
            }
            let pairs = url.query_pairs().collect::<Vec<_>>();
            let keys = ["text", "start", "end", "prefix", "suffix"];
            if pairs.len() != 5 + usize::from(pairs.iter().any(|(k, _)| k == "comment"))
                || keys
                    .iter()
                    .any(|k| pairs.iter().filter(|(key, _)| key == k).count() != 1)
            {
                return source.into();
            }
            let get = |key: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.as_ref())
            };
            if ["start", "end"].iter().any(|k| {
                get(k).is_none_or(|v| {
                    v.is_empty() || v.len() > 16 || !v.bytes().all(|b| b.is_ascii_digit())
                })
            }) {
                return source.into();
            }
            let Some(text) = get("text") else {
                return source.into();
            };
            if let Some(comment) = get("comment") {
                format!("{text}\nComment: {comment}")
            } else {
                text.into()
            }
        })
        .into_owned()
}
pub fn format_context(messages: &[TitleMessage]) -> TitleContext {
    const OMITTED: &str = "[Earlier content truncated]\n\n";
    struct Section<'a> {
        index: usize,
        source: &'a TitleMessage,
        prefix: &'static str,
        contents: String,
    }
    let sections=messages.iter().enumerate().filter_map(|(index,message)|{
        let prefix=match message.role{TitleRole::User=>"USER:\n",TitleRole::Assistant=>"ASSISTANT:\n",_=>return None};
        const BOOTSTRAP:&str="[User attached one or more files without additional text. Respond using the conversation context and the attached files.]";
        let stripped=regex::Regex::new(r"(?i)^Ultrathink:\s*").expect("bootstrap effort").replace(&message.text,"");
        let text=if stripped==BOOTSTRAP{String::new()}else{plain_citations(&message.text).trim().into()};
        if text.is_empty()&&message.attachments.is_empty(){return None;}
        let names=message.attachments.iter().map(attachment_name).collect::<Vec<_>>().join(", ");
        let contents=match (text.is_empty(),names.is_empty()){(_,true)=>text,(true,false)=>format!("[Attachments: {names}]"),(false,false)=>format!("{text}\n[Attachments: {names}]")};
        Some(Section{index,source:message,prefix,contents})
    }).collect::<Vec<_>>();
    let mut selected = BTreeMap::new();
    let mut remaining = 8000 - OMITTED.len();
    let add = |section: &Section<'_>,
               budget: usize,
               remaining: &mut usize,
               selected: &mut BTreeMap<usize, String>| {
        if selected.contains_key(&section.index) {
            return;
        }
        let limit = budget
            .min(*remaining)
            .saturating_sub(section.prefix.len() + 2);
        if limit <= 20 {
            return;
        }
        let contents = limit_title_message(&section.contents, limit);
        if contents.is_empty() {
            return;
        }
        let text = format!("{}{contents}", section.prefix);
        *remaining = remaining.saturating_sub(utf16_len(&text) + 2);
        selected.insert(section.index, text);
    };
    let first = sections.iter().find(|s| s.source.role == TitleRole::User);
    if let Some(first) = first {
        add(first, 2000, &mut remaining, &mut selected);
    }
    for section in sections
        .iter()
        .rev()
        .filter(|s| s.source.role == TitleRole::User)
    {
        add(
            section,
            2000.min(remaining.saturating_sub(2000)),
            &mut remaining,
            &mut selected,
        );
    }
    for section in sections
        .iter()
        .rev()
        .filter(|s| s.source.role == TitleRole::Assistant)
    {
        add(section, 2000, &mut remaining, &mut selected);
    }
    for role in [TitleRole::User, TitleRole::Assistant] {
        for section in sections.iter().rev().filter(|s| s.source.role == role) {
            if let Some(previous) = selected.get(&section.index) {
                let expanded = format!(
                    "{}{}",
                    section.prefix,
                    limit_title_message(
                        &section.contents,
                        utf16_len(previous) + remaining - section.prefix.len()
                    )
                );
                remaining = remaining.saturating_sub(utf16_len(&expanded) - utf16_len(previous));
                selected.insert(section.index, expanded);
            }
        }
    }
    let retained = sections
        .iter()
        .filter(|s| selected.contains_key(&s.index))
        .collect::<Vec<_>>();
    let truncated = retained
        .iter()
        .any(|s| selected.get(&s.index) != Some(&format!("{}{}", s.prefix, s.contents)));
    let mut message = if truncated || retained.len() < sections.len() {
        OMITTED.to_owned()
    } else {
        String::new()
    };
    message.push_str(&selected.into_values().collect::<Vec<_>>().join("\n\n"));
    let first_attachment = first.and_then(|s| s.source.attachments.first());
    let recent = retained
        .iter()
        .flat_map(|s| &s.source.attachments)
        .filter(|a| first_attachment.is_none_or(|first| attachment_id(a) != attachment_id(first)))
        .collect::<Vec<_>>();
    let count = if first_attachment.is_some() { 3 } else { 4 };
    let mut attachments = first_attachment.cloned().into_iter().collect::<Vec<_>>();
    attachments.extend(
        recent[recent.len().saturating_sub(count)..]
            .iter()
            .map(|a| (*a).clone()),
    );
    TitleContext {
        message,
        attachments,
    }
}

pub struct LinkedSubject {
    pub title: String,
    pub body: Option<String>,
}
#[async_trait]
pub trait TitleLinkResolver: Send + Sync {
    fn supports(&self, cwd: &Path, url: &reqwest::Url) -> bool;
    async fn resolve(&self, cwd: &Path, url: &reqwest::Url) -> Result<LinkedSubject, ()>;
}
pub async fn resolve_title_links(
    message: &str,
    cwd: &Path,
    resolver: &dyn TitleLinkResolver,
    cancellation: CancellationToken,
) -> Option<String> {
    let pattern = regex::Regex::new("https://[^\\s<>\"')\\]`]+").expect("source link pattern");
    let mut links = Vec::new();
    for m in pattern.find_iter(message) {
        let candidate = m
            .as_str()
            .trim_end_matches(['.', ',', ':', ';', '!', '?', '*']);
        let Ok(mut url) = reqwest::Url::parse(candidate) else {
            continue;
        };
        url.set_fragment(None);
        url.set_query(None);
        if links.contains(&url) || !resolver.supports(cwd, &url) {
            continue;
        }
        links.push(url);
        if links.len() == 2 {
            break;
        }
    }
    let work = async {
        futures_util::future::join_all(links.iter().map(|url| async move {
            match tokio::time::timeout(Duration::from_secs(3), resolver.resolve(cwd, url)).await {
                Ok(Ok(subject)) => {
                    #[derive(serde::Serialize)]
                    struct Subject<'a> {
                        title: &'a str,
                        body: &'a str,
                    }
                    let summary = Subject {
                        title: crate::text::head(&subject.title, 300),
                        body: crate::text::head(subject.body.as_deref().unwrap_or(""), 1200),
                    };
                    format!(
                        "{url}\n{}",
                        serde_json::to_string(&summary).expect("subject JSON")
                    )
                }
                _ => format!("{url}: unavailable"),
            }
        }))
        .await
    };
    let results =
        tokio::select! {biased;_=cancellation.cancelled()=>return None,results=work=>results};
    if results.is_empty() {
        None
    } else {
        Some(results.join("\n\n"))
    }
}
