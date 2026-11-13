//! Mobile attachment mutations return cleanup candidates after ownership transfers.
use super::{
    mobile_content::{self, Insertion},
    web_drafts::{context_label, context_link},
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn attachments(draft: &Value) -> Vec<Value> {
    draft["attachments"].as_array().cloned().unwrap_or_default()
}
fn records(draft: &Value) -> Vec<Value> {
    draft["context"]["records"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}
pub fn context_record(attachment: &Value) -> Value {
    let kind = if attachment["type"] == "image"
        || super::attachment_files::media_kind(attachment) == Some("image")
    {
        "image"
    } else {
        "file"
    };
    json!({"version":1,"kind":kind,"contextId":attachment["id"],
        "label":context_label(attachment["name"].as_str().unwrap_or_default(), attachment["type"].as_str().unwrap_or("file")),
        "attachmentId":attachment["id"],"name":attachment["name"],"mimeType":attachment["mimeType"],"sizeBytes":attachment["sizeBytes"]})
}
#[derive(Default)]
pub struct AppendOptions<'a> {
    pub allow_overflow: bool,
    pub append_reference: bool,
    pub insertion: Option<&'a Insertion>,
    pub max_attachments: Option<usize>,
}
pub struct Mutation {
    pub draft: Value,
    pub cleanup: Vec<Value>,
    pub rejected: usize,
    pub cursor: Option<usize>,
}
pub fn append(draft: &Value, incoming: &[Value], options: AppendOptions<'_>) -> Mutation {
    let existing = mobile_content::normalize(draft);
    let retained = if options.append_reference {
        mobile_content::insert_text(&existing, " ", options.insertion)
            .map(|(draft, _)| draft)
            .unwrap_or_else(|| existing.clone())
    } else {
        existing.clone()
    };
    let remaining = if options.allow_overflow {
        incoming.len()
    } else {
        options
            .max_attachments
            .unwrap_or(t3_contracts::chat_attachment::PROVIDER_SEND_TURN_MAX_ATTACHMENTS)
            .min(t3_contracts::chat_attachment::PROVIDER_SEND_TURN_MAX_ATTACHMENTS)
            .saturating_sub(attachments(&retained).len())
    };
    let capacity = if options.append_reference {
        t3_contracts::composer_context::COMPOSER_CONTEXT_MAX_RECORDS
            .saturating_sub(records(&retained).len())
    } else {
        incoming.len()
    };
    let count = incoming.len().min(remaining).min(capacity);
    let mut result = Mutation {
        draft: existing.clone(),
        cleanup: incoming[count..].to_vec(),
        rejected: incoming.len() - count,
        cursor: None,
    };
    if count == 0 {
        return result;
    }
    let mut all = attachments(&existing);
    all.extend_from_slice(&incoming[..count]);
    result.draft["attachments"] = all.into();
    if options.append_reference {
        let context: Vec<_> = incoming[..count].iter().map(context_record).collect();
        let prompt = context
            .iter()
            .map(|r| {
                context_link(
                    r["kind"].as_str().unwrap_or("file"),
                    r["contextId"].as_str().unwrap_or_default(),
                    r["label"].as_str().unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        let content = json!({"text":prompt,"context":{"version":1,"records":context}});
        let Some((next, cursor)) =
            mobile_content::insert_context(&result.draft, &content, options.insertion)
        else {
            return Mutation {
                draft: existing,
                cleanup: incoming.to_vec(),
                rejected: incoming.len(),
                cursor: None,
            };
        };
        result.draft = next;
        result.cursor = Some(cursor);
        let retained = attachments(&result.draft);
        result.cleanup.extend(
            attachments(&existing)
                .into_iter()
                .filter(|old| !retained.contains(old)),
        );
    }
    result
}
pub fn replace(draft: &Value, incoming: &[Value]) -> Mutation {
    let mut next = mobile_content::normalize(draft);
    let retained: BTreeSet<_> = incoming.iter().filter_map(|a| a["id"].as_str()).collect();
    let dropped: BTreeSet<_> = records(&next)
        .into_iter()
        .filter(|r| {
            r.get("attachmentId")
                .is_some_and(|id| !retained.contains(id.as_str().unwrap_or_default()))
        })
        .filter_map(|r| r["contextId"].as_str().map(str::to_owned))
        .collect();
    let original = next["text"].as_str().unwrap_or_default();
    let mut prompt = String::new();
    let mut end = 0;
    for reference in crate::view_logic::context_references::collect_references(original) {
        if dropped.contains(reference.context_id) {
            prompt.push_str(&original[end..reference.start]);
            end = reference.end;
        }
    }
    prompt.push_str(&original[end..]);
    next["text"] = prompt.clone().into();
    if let Some(context) = mobile_content::referenced_context(&prompt, &next["context"]) {
        next["context"] = context;
    } else if let Some(fields) = next.as_object_mut() {
        fields.remove("context");
    }
    let cleanup = attachments(&next)
        .into_iter()
        .filter(|a| !retained.contains(a["id"].as_str().unwrap_or_default()))
        .collect();
    next["attachments"] = incoming.to_vec().into();
    Mutation {
        draft: next,
        cleanup,
        rejected: 0,
        cursor: None,
    }
}
pub fn remove(draft: &Value, id: &str) -> Mutation {
    replace(
        draft,
        &attachments(draft)
            .into_iter()
            .filter(|a| a["id"] != id)
            .collect::<Vec<_>>(),
    )
}
pub fn set_upload(draft: &Value, attachment: &Value) -> (Value, Option<Value>) {
    let mut next = draft.clone();
    let Some(previous) = attachments(draft)
        .into_iter()
        .find(|a| a["id"] == attachment["id"])
    else {
        return (next, None);
    };
    if let Some(all) = next["attachments"].as_array_mut() {
        for candidate in all.iter_mut().filter(|a| a["id"] == attachment["id"]) {
            for field in ["uploadEnvironmentId", "uploadedAttachmentId"] {
                if let Some(value) = attachment.get(field) {
                    candidate[field] = value.clone();
                } else if let Some(fields) = candidate.as_object_mut() {
                    fields.remove(field);
                }
            }
        }
    }
    (next, Some(previous))
}
