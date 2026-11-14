//! ComposerContextRecords and the lossless, synchronous part of useComposerContextImport.
use super::{
    answer_context,
    web_context::{self, Kind, Placement},
};
use crate::{
    ClientError,
    platform::Platform,
    types::ThreadKey,
    view_logic::context_references::{collect_references, kind_scoped_id},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use t3_contracts::composer_context::ComposerContextRecord;

fn array<'a>(raw: &'a Value, field: &str) -> &'a [Value] {
    raw[field].as_array().map_or(&[], Vec::as_slice)
}
fn string<'a>(raw: &'a Value, field: &str) -> &'a str {
    raw[field].as_str().unwrap_or_default()
}
pub fn draft_chip_records(
    draft: &Value,
) -> Result<BTreeMap<String, ComposerContextRecord>, ClientError> {
    let mut records = answer_context::records(Some(draft), Some(draft))?;
    for raw in array(draft, "terminalContexts") {
        let mut record = raw.clone();
        record["version"] = json!(1);
        record["kind"] = json!("terminal");
        record["contextId"] = json!(kind_scoped_id("terminal", string(raw, "id")));
        record["label"] = json!(answer_context::label(
            &Kind::Terminal.label(raw),
            "terminal"
        ));
        record["text"] = json!(answer_context::truncate(
            string(raw, "text").replace("\r\n", "\n").trim_matches('\n'),
            t3_contracts::composer_context::COMPOSER_CONTEXT_TERMINAL_TEXT_MAX_CHARS,
            "\n… truncated …"
        ));
        records.push(serde_json::from_value(record)?);
    }
    for raw in array(draft, "environmentContexts")
        .iter()
        .chain(draft.get("replyTo").filter(|raw| !raw.is_null()))
    {
        records.push(serde_json::from_value(raw.clone())?);
    }
    let images = array(draft, "images");
    records
        .into_iter()
        .map(|record| {
            let mut raw = serde_json::to_value(record)?;
            if raw["kind"] == "preview-annotation"
                && images
                    .iter()
                    .any(|image| image["id"] == raw["annotationId"])
            {
                raw["screenshotContextId"] =
                    json!(kind_scoped_id("image", string(&raw, "annotationId")));
            }
            Ok((
                string(&raw, "contextId").to_owned(),
                serde_json::from_value(raw)?,
            ))
        })
        .collect()
}
fn payload(mut raw: Value) -> Value {
    if let Some(raw) = raw.as_object_mut() {
        raw.remove("contextId");
        raw.remove("label");
    }
    raw
}
fn producer(kind: &str, id: &str) -> String {
    id.strip_prefix(&format!("{kind}_")).unwrap_or(id).into()
}
pub fn import_records(
    platform: &dyn Platform,
    document: &mut Value,
    key: &str,
    thread: Option<&ThreadKey>,
    records: &[ComposerContextRecord],
    linked: &[crate::types::EnvironmentId],
) -> Result<BTreeMap<String, String>, ClientError> {
    let existing = draft_chip_records(&document["draftsByThreadKey"][key])?;
    let mut rewritten = BTreeMap::new();
    let now = crate::time::iso_from_epoch_ms(platform.now_epoch_ms());
    let mut ordered = records.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|record| !matches!(record, ComposerContextRecord::PreviewAnnotation(_)));
    for record in ordered {
        let raw = serde_json::to_value(record)?;
        let kind = string(&raw, "kind");
        let old = string(&raw, "contextId");
        let prior = existing.get(old).map(serde_json::to_value).transpose()?;
        if prior
            .as_ref()
            .is_some_and(|prior| payload(prior.clone()) == payload(raw.clone()))
        {
            continue;
        }
        let conflicts = prior.is_some();
        let id = if conflicts {
            crate::operations::allocate_command_id(platform, None)?.0
        } else {
            producer(kind, old)
        };
        let placement = Placement {
            append_reference: false,
            ..Default::default()
        };
        let destination_thread = thread
            .map(|thread| thread.thread.0.as_str())
            .unwrap_or_default();
        let new_id = match kind {
            "terminal" => {
                if thread.is_none() {
                    continue;
                }
                let mut value = raw.clone();
                value["id"] = json!(id);
                value["threadId"] = json!(destination_thread);
                value["createdAt"] = json!(now);
                web_context::add(
                    document,
                    key,
                    Kind::Terminal,
                    &[value],
                    destination_thread,
                    placement,
                );
                kind_scoped_id(kind, &id)
            }
            "review-comment" => {
                let mut value = raw.clone();
                value["id"] = json!(id);
                web_context::add(
                    document,
                    key,
                    Kind::Review,
                    &[value],
                    destination_thread,
                    placement,
                );
                kind_scoped_id(kind, &id)
            }
            "preview-annotation" => {
                let annotation_id = if conflicts {
                    id
                } else {
                    raw["annotationId"]
                        .as_str()
                        .filter(|id| !id.is_empty())
                        .unwrap_or(&id)
                        .into()
                };
                let value = preview_from_record(&raw, &annotation_id, &now);
                web_context::add(
                    document,
                    key,
                    Kind::Preview,
                    &[value],
                    destination_thread,
                    placement,
                );
                kind_scoped_id(kind, &annotation_id)
            }
            "thread" => {
                if let Some(thread) = thread
                    && raw["environmentId"] != thread.environment.0
                    && !linked.iter().any(|id| raw["environmentId"] == id.0)
                {
                    continue;
                }
                web_context::add(
                    document,
                    key,
                    Kind::Thread,
                    std::slice::from_ref(&raw),
                    destination_thread,
                    placement,
                );
                old.into()
            }
            "environment" => {
                web_context::add(
                    document,
                    key,
                    Kind::Environment,
                    std::slice::from_ref(&raw),
                    destination_thread,
                    placement,
                );
                old.into()
            }
            "response" => {
                if let Some(thread) = thread
                    && document["draftsByThreadKey"][key]["replyTo"].is_null()
                    && raw["citation"]["threadId"] == thread.thread.0
                    && raw["citation"]["environmentId"] == thread.environment.0
                {
                    web_context::set_reply(
                        document,
                        key,
                        Some(&serde_json::from_value(raw.clone())?),
                    );
                }
                continue;
            }
            _ => continue,
        };
        rewritten.insert(old.into(), new_id);
    }
    Ok(rewritten)
}
fn preview_from_record(raw: &Value, id: &str, now: &str) -> Value {
    let context = string(raw, "contextId");
    let rect = json!({"x":0,"y":0,"width":0,"height":0});
    let elements=array(raw,"elements").iter().enumerate().map(|(index,element)|{let mut element=element.clone();element["stack"]=json!([]);element["pickedAt"]=json!(now);json!({"id":array(raw,"elementIds").get(index).cloned().unwrap_or_else(||json!(format!("{context}-element-{}",index+1))),"rect":rect,"element":element})}).collect::<Vec<_>>();
    let regions = (0..raw["regionCount"].as_u64().unwrap_or(0))
        .map(|index| json!({"id":format!("{context}-region-{}",index+1),"rect":rect}))
        .collect::<Vec<_>>();
    let strokes=(0..raw["strokeCount"].as_u64().unwrap_or(0)).map(|index|json!({"id":format!("{context}-stroke-{}",index+1),"color":"","width":0,"points":[],"bounds":rect})).collect::<Vec<_>>();
    let styles=raw.get("styleChangeDetails").filter(|details|!details.is_null()).cloned().unwrap_or_else(||json!(array(raw,"styleChanges").iter().filter_map(|change|{let (property,rest)=change.as_str()?.split_once(": ")?;let (previous,value)=rest.split_once(" → ")?;Some(json!({"targetId":array(raw,"elementIds").first().cloned().unwrap_or_else(||json!(format!("{context}-element-1"))),"selector":null,"property":property,"previousValue":if previous=="(unset)"{""}else{previous},"value":value}))}).collect::<Vec<_>>()));
    json!({"id":id,"pageUrl":raw["pageUrl"],"pageTitle":raw["pageTitle"],"comment":raw["comment"],"elements":elements,"regions":regions,"strokes":strokes,"styleChanges":styles,"screenshot":null,"createdAt":now})
}
pub fn rewrite_references(prompt: &str, rewritten: &BTreeMap<String, String>) -> String {
    let mut next = String::new();
    let mut cursor = 0;
    for reference in collect_references(prompt) {
        let start = reference.start;
        let end = reference.end;
        next.push_str(&prompt[cursor..start]);
        let source = &prompt[start..end];
        if let Some(id) = rewritten.get(reference.context_id) {
            let href = source
                .find("](t3-context://v1/")
                .expect("parsed context reference")
                + 2;
            next.push_str(&source[..href]);
            next.push_str(&format!(
                "t3-context://v1/{}/{id})",
                if reference.kind == "element" {
                    "preview-annotation"
                } else {
                    reference.kind
                }
            ));
        } else {
            next.push_str(source);
        }
        cursor = end;
    }
    next.push_str(&prompt[cursor..]);
    next
}
