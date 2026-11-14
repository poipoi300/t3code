//! Pure mobile draft mutations; callers persist returned state before acknowledging imports.
use super::super::drafts::merge_text;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn array(value: &Value, key: &str) -> Vec<Value> {
    value[key].as_array().cloned().unwrap_or_default()
}
fn text(value: &Value) -> &str {
    value["text"].as_str().unwrap_or_default()
}
pub fn normalize(draft: &Value) -> Value {
    let mut draft = if draft.is_object() {
        draft.clone()
    } else {
        json!({})
    };
    if !draft["text"].is_string() {
        draft["text"] = json!("");
    }
    if !draft["attachments"].is_array() {
        draft["attachments"] = json!([]);
    }
    draft
}
pub fn is_empty(draft: &Value) -> bool {
    text(draft).is_empty()
        && array(draft, "attachments").is_empty()
        && [
            "modelSelection",
            "runtimeMode",
            "interactionMode",
            "workspaceSelection",
        ]
        .iter()
        .all(|key| draft.get(*key).is_none())
        && draft["freshContext"] != true
}
pub fn referenced_context(text: &str, context: &Value) -> Option<Value> {
    let mut references: BTreeSet<_> =
        crate::view_logic::context_references::collect_references(text)
            .into_iter()
            .map(|reference| reference.context_id.to_owned())
            .collect();
    for record in array(context, "records") {
        if record["kind"] == "preview-annotation"
            && references.contains(record["contextId"].as_str().unwrap_or_default())
            && let Some(screenshot) = record["screenshotContextId"]
                .as_str()
                .filter(|id| !id.is_empty())
        {
            references.insert(screenshot.to_owned());
        }
    }
    let records = array(context, "records")
        .into_iter()
        .filter(|record| references.contains(record["contextId"].as_str().unwrap_or_default()))
        .collect::<Vec<_>>();
    (!records.is_empty()).then(|| json!({"version":1,"records":records}))
}
fn set_context(draft: &mut Value, context: Option<Value>) {
    if let Some(context) = context {
        draft["context"] = context;
    } else if let Some(object) = draft.as_object_mut() {
        object.remove("context");
    }
}
#[derive(Debug, Clone)]
pub struct Insertion {
    pub text: String,
    pub start: usize,
    pub end: usize,
}
pub fn insertion_range(draft: &Value, target: Option<&Insertion>) -> (usize, usize) {
    let length = super::super::document::utf16_len(text(draft));
    let selection = target.filter(|target| target.text == text(draft));
    let start = selection.map_or(length, |target| target.start.min(length));
    (
        start,
        selection.map_or(start, |target| target.end.min(length).max(start)),
    )
}
pub fn insert_text(
    draft: &Value,
    value: &str,
    target: Option<&Insertion>,
) -> Option<(Value, usize)> {
    let (start, end) = insertion_range(draft, target);
    let original = text(draft);
    let before = super::super::document::utf16_byte_offset(original, start)?;
    let after = super::super::document::utf16_byte_offset(original, end)?;
    let next = set_text(
        draft,
        format!("{}{value}{}", &original[..before], &original[after..]),
    );
    Some((next, start + super::super::document::utf16_len(value)))
}
pub fn insert_context(
    draft: &Value,
    content: &Value,
    target: Option<&Insertion>,
) -> Option<(Value, usize)> {
    let original = normalize(draft);
    let (start, end) = insertion_range(&original, target);
    let before = super::super::document::utf16_byte_offset(text(&original), start)?;
    let after = super::super::document::utf16_byte_offset(text(&original), end)?;
    let (head, tail) = (&text(&original)[..before], &text(&original)[after..]);
    let retained = set_text(&original, format!("{head} {tail}"));
    let new_attachments = array(content, "attachments");
    if !new_attachments.is_empty()
        && array(&retained, "attachments").len() + new_attachments.len()
            > t3_contracts::chat_attachment::PROVIDER_SEND_TURN_MAX_ATTACHMENTS
    {
        return None;
    }
    let incoming = text(content);
    let lead = !head.is_empty()
        && !head.chars().next_back().is_some_and(char::is_whitespace)
        && !incoming.chars().next().is_some_and(char::is_whitespace);
    let trailing = !incoming
        .chars()
        .next_back()
        .is_some_and(char::is_whitespace)
        && (tail.is_empty() || !tail.chars().next().is_some_and(char::is_whitespace));
    let insertion = format!(
        "{}{incoming}{}",
        if lead { " " } else { "" },
        if trailing { " " } else { "" }
    );
    let value = format!("{head}{insertion}{tail}");
    let mut records = array(&original["context"], "records");
    for record in array(&content["context"], "records") {
        if let Some(index) = records
            .iter()
            .position(|old| old["contextId"] == record["contextId"])
        {
            records[index] = record;
        } else {
            records.push(record);
        }
    }
    let context = referenced_context(&value, &json!({"version":1,"records":records}));
    if context
        .as_ref()
        .map_or(0, |context| array(context, "records").len())
        > t3_contracts::composer_context::COMPOSER_CONTEXT_MAX_RECORDS
    {
        return None;
    }
    let mut next = original.clone();
    let mut attachments = array(&next, "attachments");
    attachments.extend(new_attachments);
    next["attachments"] = attachments.into();
    next["text"] = value.into();
    set_context(&mut next, context.clone());
    let previous: BTreeSet<_> = array(&original["context"], "records")
        .into_iter()
        .filter_map(|record| record["attachmentId"].as_str().map(str::to_owned))
        .collect();
    let retained: BTreeSet<_> = context
        .as_ref()
        .map(|context| array(context, "records"))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|record| record["attachmentId"].as_str().map(str::to_owned))
        .collect();
    next["attachments"] = array(&next, "attachments")
        .into_iter()
        .filter(|attachment| {
            attachment["type"] == "image"
                || !previous.contains(attachment["id"].as_str().unwrap_or_default())
                || retained.contains(attachment["id"].as_str().unwrap_or_default())
        })
        .collect::<Vec<_>>()
        .into();
    Some((next, start + super::super::document::utf16_len(&insertion)))
}
pub fn set_text(draft: &Value, value: String) -> Value {
    let mut next = normalize(draft);
    let context = referenced_context(&value, &next["context"]);
    let previous: BTreeSet<_> = array(&next["context"], "records")
        .into_iter()
        .filter_map(|record| record["attachmentId"].as_str().map(str::to_owned))
        .collect();
    let retained: BTreeSet<_> = context
        .as_ref()
        .map(|context| array(context, "records"))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|record| record["attachmentId"].as_str().map(str::to_owned))
        .collect();
    next["attachments"] = array(&next, "attachments")
        .into_iter()
        .filter(|attachment| {
            attachment["type"] == "image"
                || !previous.contains(attachment["id"].as_str().unwrap_or_default())
                || retained.contains(attachment["id"].as_str().unwrap_or_default())
        })
        .collect::<Vec<_>>()
        .into();
    next["text"] = value.into();
    set_context(&mut next, context);
    next
}
pub(super) fn put(current: &Value, key: &str, draft: Value) -> Value {
    let mut next = current.clone();
    if !next.is_object() {
        next = json!({});
    }
    if is_empty(&draft) && draft.get("project").is_none() {
        if let Some(fields) = next.as_object_mut() {
            fields.remove(key);
        }
    } else {
        next[key] = draft;
    }
    next
}
pub fn update_settings(
    current: &Value,
    key: &str,
    settings: &serde_json::Map<String, Value>,
) -> Value {
    let mut draft = normalize(&current[key]);
    if let Some(fields) = draft.as_object_mut() {
        for (key, value) in settings {
            if value.is_null() {
                fields.remove(key);
            } else {
                fields.insert(key.clone(), value.clone());
            }
        }
    }
    put(current, key, draft)
}
pub fn set_fresh_context(current: &Value, key: &str, pending: bool) -> Value {
    if (current[key]["freshContext"] == true) == pending {
        return current.clone();
    }
    update_settings(
        current,
        key,
        &serde_json::Map::from_iter([(
            "freshContext".into(),
            if pending { json!(true) } else { Value::Null },
        )]),
    )
}
pub fn remove_environment(current: &Value, environment: &str) -> Value {
    let prefix = format!("{environment}:");
    let mut next = current.clone();
    if let Some(drafts) = next.as_object_mut() {
        drafts.retain(|key, draft| {
            !key.starts_with(&prefix) && draft["project"]["environmentId"] != environment
        });
    }
    next
}
pub fn new_task(
    current: &Value,
    id: &str,
    environment: &str,
    project: &str,
    now: &str,
) -> (Value, String) {
    let key = format!("new-task:{id}");
    let mut next = current.clone();
    if !next.is_object() {
        next = json!({});
    }
    next[&key] = json!({"text":"","attachments":[],"project":{"environmentId":environment,"projectId":project,"createdAt":now}});
    (next, key)
}
pub fn new_task_keys(current: &Value, environment: &str, project: &str) -> Vec<String> {
    let mut entries = current
        .as_object()
        .into_iter()
        .flat_map(|drafts| drafts.iter())
        .filter(|(key, draft)| {
            key.starts_with("new-task:")
                && draft["project"]["environmentId"] == environment
                && draft["project"]["projectId"] == project
        })
        .collect::<Vec<_>>();
    entries.sort_by(|(_, a), (_, b)| {
        b["project"]["createdAt"]
            .as_str()
            .unwrap_or_default()
            .cmp(a["project"]["createdAt"].as_str().unwrap_or_default())
    });
    entries.into_iter().map(|(key, _)| key.clone()).collect()
}
pub fn clear_content(
    current: &Value,
    key: &str,
    clear_model: bool,
    clear_workspace: bool,
) -> Value {
    let Some(existing) = current.get(key) else {
        return current.clone();
    };
    let mut draft = normalize(existing);
    if let Some(fields) = draft.as_object_mut() {
        for field in ["importedShareIds", "context", "project"] {
            fields.remove(field);
        }
        if clear_model {
            fields.remove("modelSelection");
        }
        if clear_workspace {
            fields.remove("workspaceSelection");
        }
    }
    draft["text"] = json!("");
    draft["attachments"] = json!([]);
    put(current, key, draft)
}
pub fn merge_content(current: &Value, key: &str, content: &Value) -> Value {
    let existing = normalize(&current[key]);
    let share = content["sourceShareId"]
        .as_str()
        .filter(|id| !id.is_empty());
    let mut receipts = array(&existing, "importedShareIds");
    if share.is_some_and(|share| receipts.iter().any(|receipt| receipt == share)) {
        return current.clone();
    }
    let mut attachments = array(&existing, "attachments");
    let original_count = attachments.len();
    let mut ids: BTreeSet<_> = attachments
        .iter()
        .filter_map(|a| a["id"].as_str().map(str::to_owned))
        .collect();
    for attachment in array(content, "attachments") {
        if ids.insert(attachment["id"].as_str().unwrap_or_default().into()) {
            attachments.push(attachment);
        }
    }
    attachments.truncate(t3_contracts::chat_attachment::PROVIDER_SEND_TURN_MAX_ATTACHMENTS);
    let value = merge_text(text(&existing), text(content));
    let mut records = array(&existing["context"], "records");
    for incoming in array(&content["context"], "records") {
        if let Some(index) = records
            .iter()
            .position(|record| record["contextId"] == incoming["contextId"])
        {
            records[index] = incoming;
        } else {
            records.push(incoming);
        }
    }
    let context = referenced_context(&value, &json!({"version":1,"records":records}));
    if let Some(share) = share {
        receipts.push(json!(share));
    }
    let mut draft = existing;
    draft["text"] = value.into();
    draft["attachments"] = attachments.into();
    set_context(&mut draft, context);
    if !receipts.is_empty() {
        draft["importedShareIds"] = receipts.into();
    }
    if share.is_some() {
        let original_attachments = array(&draft, "attachments");
        let mut only_new = draft.clone();
        only_new["attachments"] = original_attachments
            .iter()
            .skip(original_count)
            .cloned()
            .collect::<Vec<_>>()
            .into();
        let restored = super::mobile_drafts::restore_file_references(&only_new);
        draft["text"] = restored["text"].clone();
        set_context(&mut draft, restored.get("context").cloned());
    }
    put(current, key, draft)
}
/// Reference fields are unchanged only when the caller's captured revision token still matches.
pub fn undo_merge(
    current: &Value,
    key: &str,
    snapshot: &Value,
    merged: &Value,
    unchanged_references: &BTreeSet<&str>,
) -> Value {
    let existing = normalize(&current[key]);
    let same = |field: &str| {
        existing.get(field) == merged.get(field)
            && (!matches!(
                field,
                "attachments"
                    | "context"
                    | "importedShareIds"
                    | "modelSelection"
                    | "workspaceSelection"
            ) || unchanged_references.contains(field))
    };
    if [
        "text",
        "attachments",
        "context",
        "importedShareIds",
        "modelSelection",
        "runtimeMode",
        "interactionMode",
        "freshContext",
        "workspaceSelection",
    ]
    .iter()
    .all(|field| same(field))
    {
        return put(current, key, normalize(snapshot));
    }
    let inserted = text(merged)
        .strip_prefix(text(snapshot))
        .unwrap_or_default();
    let value = if !inserted.is_empty() && text(&existing).starts_with(text(merged)) {
        format!(
            "{}{}",
            text(snapshot),
            &text(&existing)[text(merged).len()..]
        )
    } else if !inserted.is_empty() && text(&existing).ends_with(inserted) {
        text(&existing)[..text(&existing).len() - inserted.len()].into()
    } else {
        text(&existing).into()
    };
    let original_ids: BTreeSet<_> = array(snapshot, "attachments")
        .into_iter()
        .filter_map(|attachment| attachment["id"].as_str().map(str::to_owned))
        .collect();
    let inserted_ids: BTreeSet<_> = array(merged, "attachments")
        .into_iter()
        .filter_map(|attachment| {
            attachment["id"]
                .as_str()
                .filter(|id| !original_ids.contains(*id))
                .map(str::to_owned)
        })
        .collect();
    let mut draft = existing.clone();
    draft["text"] = value.into();
    draft["attachments"] = array(&existing, "attachments")
        .into_iter()
        .filter(|attachment| !inserted_ids.contains(attachment["id"].as_str().unwrap_or_default()))
        .collect::<Vec<_>>()
        .into();
    let context = referenced_context(text(&draft), &existing["context"]);
    set_context(&mut draft, context);
    for field in [
        "modelSelection",
        "runtimeMode",
        "interactionMode",
        "freshContext",
        "workspaceSelection",
    ] {
        if same(field) {
            if let Some(value) = snapshot.get(field) {
                draft[field] = value.clone();
            } else if let Some(fields) = draft.as_object_mut() {
                fields.remove(field);
            }
        }
    }
    put(current, key, draft)
}
pub fn retarget(draft: &Value, environment: &str, project: &str, now: &str) -> Value {
    retarget_with_options(draft, environment, project, now, false)
}
pub fn retarget_with_options(
    draft: &Value,
    environment: &str,
    project: &str,
    now: &str,
    keep_pull_request_workspace: bool,
) -> Value {
    let mut next = normalize(draft);
    if next["project"]["environmentId"] == environment && next["project"]["projectId"] == project {
        return next;
    }
    let created = next["project"]["createdAt"]
        .as_str()
        .unwrap_or(now)
        .to_owned();
    if keep_pull_request_workspace
        && next["workspaceSelection"]
            .get("pullRequestReference")
            .is_some()
    {
        next["workspaceSelection"]["worktreePath"] = Value::Null;
    } else if let Some(fields) = next.as_object_mut() {
        fields.remove("workspaceSelection");
    }
    let attachments = array(&next, "attachments")
        .into_iter()
        .map(|mut attachment| {
            if attachment
                .get("uploadEnvironmentId")
                .is_some_and(|id| id != environment)
                && let Some(fields) = attachment.as_object_mut()
            {
                fields.remove("uploadEnvironmentId");
                fields.remove("uploadedAttachmentId");
            }
            attachment
        })
        .collect::<Vec<_>>();
    next["attachments"] = attachments.into();
    next["project"] = json!({"environmentId":environment,"projectId":project,"createdAt":created});
    next
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file(id: &str) -> Value {
        json!({"type":"file","id":id,"name":format!("{id}.txt"),"mimeType":"text/plain","sizeBytes":3,"fileUri":format!("file:///{id}")})
    }
    #[test]
    fn asynchronous_context_insertion_uses_captured_selection_until_typing_changes_the_text() {
        let record = json!({"version":1,"kind":"file","contextId":"f","attachmentId":"f","label":"f.txt","name":"f.txt","mimeType":"text/plain","sizeBytes":3});
        let content = json!({"text":"[f.txt](t3-context://v1/file/f)","context":{"version":1,"records":[record]},"attachments":[file("f")]});
        let draft = json!({"text":"😀 replace tail","attachments":[]});
        let insertion = Insertion {
            text: text(&draft).into(),
            start: 3,
            end: 10,
        };
        let (inserted, cursor) =
            insert_context(&draft, &content, Some(&insertion)).expect("insert");
        assert_eq!(text(&inserted), "😀 [f.txt](t3-context://v1/file/f) tail");
        assert_eq!(
            cursor,
            super::super::super::document::utf16_len("😀 [f.txt](t3-context://v1/file/f)")
        );
        assert_eq!(array(&inserted, "attachments").len(), 1);
        let edited = json!({"text":"new typing","attachments":[]});
        let (appended, _) =
            insert_context(&edited, &content, Some(&insertion)).expect("append after edit");
        assert_eq!(
            text(&appended),
            "new typing [f.txt](t3-context://v1/file/f) "
        );
        let target = Insertion {
            text: text(&inserted).into(),
            start: 3,
            end: cursor,
        };
        let (replaced, _) =
            insert_text(&inserted, "gone", Some(&target)).expect("replace selected file");
        assert!(array(&replaced, "attachments").is_empty());
        assert!(replaced.get("context").is_none());
    }
    #[test]
    fn shared_files_receive_visible_references_and_receipts_survive_edits_and_retries() {
        let current = json!({"key":{"text":"typed","attachments":[],"future":true}});
        let content =
            json!({"text":"shared","attachments":[file("f"),file("f")],"sourceShareId":"share"});
        let merged = merge_content(&current, "key", &content);
        assert_eq!(array(&merged["key"], "attachments").len(), 1);
        assert!(text(&merged["key"]).contains("t3-context://v1/file/f"));
        assert_eq!(merged["key"]["future"], true);
        let mut edited = merged.clone();
        edited["key"]["text"] = json!("rewritten");
        assert_eq!(merge_content(&edited, "key", &content), edited);
        let removed = set_text(&merged["key"], "file reference removed".into());
        assert!(array(&removed, "attachments").is_empty());
        assert!(removed.get("context").is_none());
        assert_eq!(removed["importedShareIds"], json!(["share"]));
    }
    #[test]
    fn ordinary_recovery_does_not_append_reference_text_and_shared_merge_caps_only_new_data() {
        let current = json!({"key":{"text":"","attachments":(0..100).map(|id|file(&id.to_string())).collect::<Vec<_>>()}});
        let content = json!({"text":"recover","attachments":[file("extra")]});
        let merged = merge_content(&current, "key", &content);
        assert_eq!(text(&merged["key"]), "recover");
        assert_eq!(array(&merged["key"], "attachments").len(), 100);
        assert_eq!(merge_content(&merged, "key", &content), merged);
    }
    #[test]
    fn undo_removes_only_imported_content_after_concurrent_typing_and_preserves_newer_same_value_model_pick()
     {
        let snapshot = json!({"text":"before","attachments":[],"modelSelection":{"instanceId":"codex","model":"before"}});
        let mut merged = merge_content(
            &json!({"key":snapshot}),
            "key",
            &json!({"text":"imported","attachments":[file("new")]}),
        )["key"]
            .clone();
        merged["modelSelection"] = json!({"instanceId":"codex","model":"after"});
        let mut edited = merged.clone();
        edited["text"] = json!(format!("{}\n\nfollow-up", text(&merged)));
        let references = BTreeSet::from([
            "attachments",
            "context",
            "importedShareIds",
            "workspaceSelection",
        ]);
        let undone = undo_merge(
            &json!({"key":edited}),
            "key",
            &snapshot,
            &merged,
            &references,
        );
        assert_eq!(text(&undone["key"]), "before\n\nfollow-up");
        assert!(array(&undone["key"], "attachments").is_empty());
        assert_eq!(undone["key"]["modelSelection"]["model"], "after");
        let references = BTreeSet::from([
            "attachments",
            "context",
            "importedShareIds",
            "workspaceSelection",
            "modelSelection",
        ]);
        assert_eq!(
            undo_merge(
                &json!({"key":merged}),
                "key",
                &snapshot,
                &merged,
                &references
            ),
            json!({"key":snapshot})
        );
    }
    #[test]
    fn retarget_preserves_local_bytes_and_original_time_but_releases_cross_environment_upload_stamps()
     {
        let mut attachment = file("f");
        attachment["uploadEnvironmentId"] = json!("old");
        attachment["uploadedAttachmentId"] = json!("uploaded");
        let original = json!({"text":"draft","attachments":[attachment],"project":{"environmentId":"old","projectId":"p","createdAt":"original"},"workspaceSelection":{"mode":"worktree"},"modelSelection":{"instanceId":"codex","model":"model"}});
        assert_eq!(retarget(&original, "old", "p", "later"), original);
        let next = retarget(&original, "new", "p2", "later");
        assert_eq!(next["attachments"][0]["fileUri"], "file:///f");
        assert!(next["attachments"][0].get("uploadedAttachmentId").is_none());
        assert_eq!(next["project"]["createdAt"], "original");
        assert!(next.get("workspaceSelection").is_none());
        assert_eq!(next["modelSelection"], original["modelSelection"]);
    }
    #[test]
    fn clear_content_preserves_runtime_and_fresh_context_and_optionally_releases_model_and_workspace()
     {
        let current = json!({"key":{"text":"content","attachments":[file("f")],"project":{"projectId":"p"},"importedShareIds":["share"],"runtimeMode":"auto","freshContext":true,"modelSelection":{"model":"model"},"workspaceSelection":{"mode":"local"}}});
        let next = clear_content(&current, "key", true, true);
        assert_eq!(
            next["key"],
            json!({"text":"","attachments":[],"runtimeMode":"auto","freshContext":true})
        );
        assert!(
            clear_content(
                &json!({"key":{"text":"gone","attachments":[]}}),
                "key",
                false,
                false
            )
            .get("key")
            .is_none()
        );
    }
}
