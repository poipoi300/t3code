//! Context edits share one reference representation and preserve UTF-16 insertion selections.
use super::{
    mobile_content::Insertion,
    web_drafts::{
        context_id, context_link, ensure_reference, kind_context_id, preview_label, review_label,
    },
    web_models::{draft, save},
    web_normalize,
};
use crate::{
    thread_views::document::{utf16_byte_offset, utf16_len},
    view_logic::context_references::remove_reference,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Terminal,
    Preview,
    Review,
    Thread,
    Environment,
}
impl Kind {
    pub(super) fn field(self) -> &'static str {
        match self {
            Self::Terminal => "terminalContexts",
            Self::Preview => "previewAnnotations",
            Self::Review => "reviewComments",
            Self::Thread => "threadContexts",
            Self::Environment => "environmentContexts",
        }
    }
    pub(super) fn wire(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Preview => "preview-annotation",
            Self::Review => "review-comment",
            Self::Thread => "thread",
            Self::Environment => "environment",
        }
    }
    pub(super) fn id(self, value: &Value) -> String {
        if matches!(self, Self::Thread | Self::Environment) {
            string(value, "contextId").into()
        } else {
            kind_context_id(self.wire(), string(value, "id"))
        }
    }
    pub(super) fn label(self, value: &Value) -> String {
        match self {
            Self::Terminal => {
                let start = &value["lineStart"];
                let end = &value["lineEnd"];
                format!(
                    "{} {}",
                    string(value, "terminalLabel"),
                    if start == end {
                        format!("line {start}")
                    } else {
                        format!("lines {start}-{end}")
                    }
                )
            }
            Self::Preview => preview_label(value),
            Self::Review => review_label(value),
            Self::Thread | Self::Environment => string(value, "label").into(),
        }
    }
}
fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}
fn array(value: &Value, key: &str) -> Vec<Value> {
    value[key].as_array().cloned().unwrap_or_default()
}
fn normalize(kind: Kind, value: &Value, thread: &str) -> Option<Value> {
    match kind {
        Kind::Terminal => {
            let terminal = value["terminalId"].as_str()?.trim();
            let label = value["terminalLabel"].as_str()?.trim();
            if terminal.is_empty() || label.is_empty() {
                return None;
            }
            let start = value["lineStart"].as_f64()?.floor().max(1.0);
            let end = value["lineEnd"].as_f64()?.floor().max(start);
            let mut next = value.clone();
            next["id"] = json!(context_id(string(value, "id")));
            next["threadId"] = json!(thread);
            next["terminalId"] = json!(terminal);
            next["terminalLabel"] = json!(label);
            next["lineStart"] =
                serde_json::from_str(&t3_contracts::wire::format_js_number(start)).ok()?;
            next["lineEnd"] =
                serde_json::from_str(&t3_contracts::wire::format_js_number(end)).ok()?;
            next["text"] = json!(
                string(value, "text")
                    .replace("\r\n", "\n")
                    .trim_matches('\n')
            );
            Some(next)
        }
        Kind::Review => web_normalize::review(value).then(|| value.clone()),
        Kind::Thread => {
            serde_json::from_value::<t3_contracts::composer_context::ThreadContextRecord>(
                value.clone(),
            )
            .ok()
            .map(|_| value.clone())
        }
        Kind::Environment => serde_json::from_value::<
            t3_contracts::composer_context::EnvironmentContextRecord,
        >(value.clone())
        .ok()
        .map(|_| value.clone()),
        Kind::Preview => {
            serde_json::from_value::<t3_contracts::ipc::PreviewAnnotationPayloadSchema>(
                value.clone(),
            )
            .ok()
            .map(|_| value.clone())
        }
    }
}
fn normalized(kind: Kind, values: &[Value], thread: &str) -> Vec<Value> {
    let mut ids = BTreeSet::new();
    let mut locations = BTreeSet::new();
    values
        .iter()
        .filter_map(|value| normalize(kind, value, thread))
        .filter(|value| {
            if kind != Kind::Terminal {
                return true;
            }
            let id = kind.id(value);
            let location = format!(
                "{}\0{}\0{}",
                string(value, "terminalId"),
                value["lineStart"],
                value["lineEnd"]
            );
            if ids.contains(&id) || locations.contains(&location) {
                return false;
            }
            ids.insert(id);
            locations.insert(location);
            true
        })
        .collect()
}
pub fn set(state: &mut Value, key: &str, kind: Kind, values: &[Value], thread: &str) {
    if key.is_empty() {
        return;
    }
    let mut next = draft(state, key);
    let values = normalized(kind, values, thread);
    let ids: BTreeSet<_> = values.iter().map(|value| kind.id(value)).collect();
    let mut prompt = string(&next, "prompt").to_owned();
    for prior in array(&next, kind.field()) {
        let id = kind.id(&prior);
        if !ids.contains(&id) {
            prompt = remove_reference(&prompt, &id)
        }
    }
    for value in &values {
        ensure_reference(
            &mut prompt,
            kind.wire(),
            &kind.id(value),
            &kind.label(value),
        );
    }
    next["prompt"] = json!(prompt);
    next[kind.field()] = json!(values);
    save(state, key, next);
}
pub struct Placement<'a> {
    pub append_reference: bool,
    pub insertion: Option<&'a Insertion>,
    pub allow_duplicate_reference: bool,
}
impl Default for Placement<'_> {
    fn default() -> Self {
        Self {
            append_reference: true,
            insertion: None,
            allow_duplicate_reference: false,
        }
    }
}
fn insert(prompt: &str, references: &[String], target: &Insertion) -> Option<(String, usize)> {
    if target.text != prompt {
        return None;
    }
    let start = target.start.min(utf16_len(prompt));
    let end = target.end.min(utf16_len(prompt)).max(start);
    let from = utf16_byte_offset(prompt, start)?;
    let to = utf16_byte_offset(prompt, end)?;
    let lead = prompt[..from]
        .chars()
        .next_back()
        .is_some_and(|ch| !matches!(ch, ' ' | '\t' | '\r' | '\n'));
    let text = format!("{}{} ", if lead { " " } else { "" }, references.join(" "));
    let to = to + usize::from(prompt.as_bytes().get(to) == Some(&b' '));
    Some((
        format!("{}{}{}", &prompt[..from], text, &prompt[to..]),
        start + utf16_len(&text),
    ))
}
pub fn add(
    state: &mut Value,
    key: &str,
    kind: Kind,
    values: &[Value],
    thread: &str,
    placement: Placement<'_>,
) -> Option<usize> {
    if key.is_empty() || values.is_empty() {
        return None;
    }
    let mut next = draft(state, key);
    let existing = array(&next, kind.field());
    let mut all = existing.clone();
    let incoming = normalized(kind, values, thread);
    let mut accepted = Vec::new();
    let replace = matches!(kind, Kind::Review | Kind::Preview);
    for value in incoming {
        let id = kind.id(&value);
        let present = all.iter().any(|prior| kind.id(prior) == id);
        if present && !replace {
            continue;
        }
        if kind == Kind::Terminal
            && all.iter().any(|prior| {
                prior["terminalId"] == value["terminalId"]
                    && prior["lineStart"] == value["lineStart"]
                    && prior["lineEnd"] == value["lineEnd"]
            })
        {
            continue;
        }
        if replace {
            all.retain(|prior| kind.id(prior) != id);
        }
        if placement.append_reference
            && (!present || (kind == Kind::Review && placement.allow_duplicate_reference))
        {
            accepted.push(value.clone());
        }
        all.push(value);
    }
    if all == existing && accepted.is_empty() {
        return None;
    }
    let mut prompt = string(&next, "prompt").to_owned();
    let mut caret = None;
    if !accepted.is_empty() {
        let references = accepted
            .iter()
            .map(|value| context_link(kind.wire(), &kind.id(value), &kind.label(value)))
            .collect::<Vec<_>>();
        if let Some(target) = placement.insertion
            && let Some((inserted, pos)) = insert(&prompt, &references, target)
        {
            prompt = inserted;
            caret = Some(pos);
        } else if matches!(kind, Kind::Review | Kind::Preview) {
            for reference in references {
                if prompt
                    .chars()
                    .next_back()
                    .is_some_and(|ch| !matches!(ch, ' ' | '\t' | '\r' | '\n'))
                {
                    prompt.push(' ')
                }
                prompt.push_str(&reference);
                prompt.push(' ');
            }
        } else {
            let references = if kind == Kind::Terminal {
                &all
            } else {
                &accepted
            };
            for value in references {
                ensure_reference(
                    &mut prompt,
                    kind.wire(),
                    &kind.id(value),
                    &kind.label(value),
                );
            }
        }
    }
    next["prompt"] = json!(prompt);
    next[kind.field()] = json!(all);
    save(state, key, next);
    caret
}
pub fn remove(state: &mut Value, key: &str, kind: Kind, id: &str) -> bool {
    let Some(prior) = state["draftsByThreadKey"].get(key) else {
        return false;
    };
    let mut next = prior.clone();
    let old = array(prior, kind.field());
    let values = old
        .iter()
        .filter(|value| {
            if matches!(kind, Kind::Thread | Kind::Environment) {
                string(value, "contextId") != id
            } else {
                string(value, "id") != id
            }
        })
        .cloned()
        .collect::<Vec<_>>();
    if values.len() == old.len() && kind != Kind::Terminal {
        return false;
    }
    let reference = if matches!(kind, Kind::Thread | Kind::Environment) {
        id.to_owned()
    } else {
        kind_context_id(kind.wire(), id)
    };
    next["prompt"] = json!(remove_reference(string(prior, "prompt"), &reference));
    next[kind.field()] = json!(values);
    if kind == Kind::Preview {
        for field in ["images", "persistedAttachments"] {
            next[field] = json!(
                array(&next, field)
                    .into_iter()
                    .filter(|value| string(value, "id") != id)
                    .collect::<Vec<_>>()
            );
        }
        next["nonPersistedImageIds"] = json!(
            array(&next, "nonPersistedImageIds")
                .into_iter()
                .filter(|value| value != id)
                .collect::<Vec<_>>()
        );
    }
    save(state, key, next);
    true
}
pub fn set_reply(
    state: &mut Value,
    key: &str,
    response: Option<&t3_contracts::composer_context::ResponseContextRecord>,
) {
    if key.is_empty() {
        return;
    }
    let mut next = draft(state, key);
    next["replyTo"] = json!(response);
    save(state, key, next);
}
