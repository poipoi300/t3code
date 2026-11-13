use super::super::{
    drafts::{DraftFormat, SavedDraftDocument, merge_recovery, storage_key},
    outbox::{QueuedMessage, encode_message},
};
use crate::{
    ClientError,
    platform::{Storage, StorageArea, StorageCondition, StorageTransaction, StorageWrite},
};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

fn invalid(message: &str) -> ClientError {
    ClientError::InvalidPayload(message.into())
}
pub(super) fn validate_document(raw: &Value) -> Result<(), ClientError> {
    if raw["schemaVersion"] != 1 || !raw["drafts"].is_object() {
        return Err(invalid("invalid mobile draft document"));
    }
    if raw.get("stickyModelSelection").is_some_and(|value| {
        serde_json::from_value::<t3_contracts::model_selection::ModelSelection>(value.clone())
            .is_err()
    }) || raw
        .get("cloudAccountId")
        .is_some_and(|value| !value.is_string())
    {
        return Err(invalid(
            "unreadable draft account or model selection retained for recovery",
        ));
    }
    if let Some(memory) = raw.get("modelOptionMemory") {
        let valid = memory.as_object().is_some_and(|instances| {
            instances.values().all(|models| {
                models.as_object().is_some_and(|models| {
                    models.values().all(|options| {
                        options.is_array()
                            && serde_json::from_value::<
                                Vec<t3_contracts::model::ProviderOptionSelection>,
                            >(options.clone())
                            .is_ok()
                    })
                })
            })
        });
        if !valid {
            return Err(invalid(
                "unreadable draft option memory retained for recovery",
            ));
        }
    }
    if let Some(archives) = raw.get("signedOutDrafts") {
        let archives = archives
            .as_object()
            .ok_or_else(|| invalid("unreadable cloud draft archives retained for recovery"))?;
        if archives
            .values()
            .any(|archive| !archive["drafts"].is_object() || !archive["queuedMessages"].is_array())
        {
            return Err(invalid(
                "unreadable cloud draft archive retained for recovery",
            ));
        }
    }
    Ok(())
}
fn record(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}
fn array(value: &Value, key: &str) -> Vec<Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}
fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}
fn empty(draft: &Value) -> bool {
    text(draft, "text").is_empty()
        && array(draft, "attachments").is_empty()
        && [
            "modelSelection",
            "runtimeMode",
            "interactionMode",
            "workspaceSelection",
        ]
        .iter()
        .all(|k| draft.get(*k).is_none())
        && draft["freshContext"] != true
}
pub fn draft_environment<'a>(
    key: &str,
    known: impl IntoIterator<Item = &'a str>,
    queued: &[Value],
    draft: &Value,
) -> Option<String> {
    if key.starts_with("pending-task:") {
        return queued
            .iter()
            .find(|m| key == format!("pending-task:{}", text(m, "messageId")))
            .map(|m| text(m, "environmentId").into());
    }
    if let Some(scope) = key.strip_prefix("new-task:") {
        if let Some(environment) = draft["project"]["environmentId"].as_str() {
            return Some(environment.into());
        }
        return scope
            .rsplit_once(':')
            .filter(|(e, p)| !e.is_empty() && !p.is_empty())
            .map(|(e, _)| e.into());
    }
    known
        .into_iter()
        .filter(|e| key.starts_with(&format!("{e}:")))
        .max_by_key(|e| e.len())
        .map(str::to_string)
}
pub fn restore_file_references(draft: &Value) -> Value {
    let mut result = draft.clone();
    let mut records = array(&draft["context"], "records");
    let mut prompt = text(draft, "text").to_string();
    let mut used: BTreeSet<String> = records
        .iter()
        .map(|r| text(r, "contextId").into())
        .collect();
    for attachment in array(draft, "attachments") {
        let mime = text(&attachment, "mimeType");
        if attachment["type"] == "image" || mime.starts_with("image/") || mime.starts_with("video/")
        {
            continue;
        }
        let id = text(&attachment, "id");
        let index = records
            .iter()
            .position(|r| r["kind"] == "file" && r["attachmentId"] == id);
        let context = if let Some(index) = index {
            records[index].clone()
        } else {
            let base: String = id
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                        c
                    } else {
                        '_'
                    }
                })
                .take(110)
                .collect();
            let base = if base.is_empty() { "file" } else { &base };
            let mut context_id = base.to_string();
            let mut suffix = 2;
            while used.contains(&context_id) {
                context_id = format!("{base}_{suffix}");
                suffix += 1;
            }
            used.insert(context_id.clone());
            let context = json!({"version":1,"kind":"file","contextId":context_id,"label":attachment["name"],"attachmentId":attachment["id"],"name":attachment["name"],"mimeType":attachment["mimeType"],"sizeBytes":attachment["sizeBytes"]});
            records.push(context.clone());
            context
        };
        let href = format!("t3-context://v1/file/{}", text(&context, "contextId"));
        if prompt.contains(&format!("]({href})")) {
            continue;
        }
        if prompt
            .chars()
            .next_back()
            .is_some_and(|c| !c.is_whitespace())
        {
            prompt.push(' ');
        }
        prompt.push_str(&format!(
            "[{}]({href}) ",
            text(&context, "label").replace(['[', ']', '\\', '\r', '\n'], " ")
        ));
    }
    result["text"] = prompt.into();
    if !records.is_empty() {
        result["context"] = json!({"version":1,"records":records});
    }
    result
}
pub fn migrate_document(
    raw: &Value,
    now: &str,
    mut fresh_id: impl FnMut() -> String,
) -> Result<Value, ClientError> {
    validate_document(raw)?;
    let mut result = raw.clone();
    let migrate = |source: &Value,
                   fresh_id: &mut dyn FnMut() -> String,
                   archived: bool|
     -> Result<Value, ClientError> {
        let mut drafts = Map::new();
        for (key, value) in record(source) {
            if !value.is_object() || !value["text"].is_string() || !value["attachments"].is_array()
            {
                return Err(invalid(
                    "unreadable user draft retained in original document",
                ));
            }
            let mut draft = restore_file_references(&value);
            let mut target = key.clone();
            if !archived
                && key.starts_with("new-task:")
                && draft.get("modelSelection").is_some()
                && text(&draft, "text").is_empty()
                && array(&draft, "attachments").is_empty()
                && ["runtimeMode", "interactionMode", "workspaceSelection"]
                    .iter()
                    .all(|k| draft.get(*k).is_none())
                && let Some(object) = draft.as_object_mut()
            {
                object.remove("modelSelection");
            }
            if draft.get("project").is_none()
                && let Some((environment, project)) = key
                    .strip_prefix("new-task:")
                    .and_then(|s| s.rsplit_once(':'))
                    .filter(|(e, p)| !e.is_empty() && !p.is_empty())
            {
                target = format!("new-task:{}", fresh_id());
                if source.get(&target).is_some() || drafts.contains_key(&target) {
                    return Err(invalid("new draft id collision"));
                }
                draft["project"] =
                    json!({"environmentId":environment,"projectId":project,"createdAt":now});
            }
            if !archived && key.contains("~queued-edit~") {
                continue;
            }
            if archived || !empty(&draft) || !array(&draft, "importedShareIds").is_empty() {
                drafts.insert(target, draft);
            }
        }
        Ok(drafts.into())
    };
    result["drafts"] = migrate(&raw["drafts"], &mut fresh_id, false)?;
    for (account, archive) in record(&raw["signedOutDrafts"]) {
        result["signedOutDrafts"][&account]["drafts"] =
            migrate(&archive["drafts"], &mut fresh_id, true)?;
    }
    Ok(result)
}
pub fn archive_cloud(
    raw: &Value,
    owner: Option<&str>,
    environments: &BTreeSet<String>,
    known: &[String],
    queued: &[QueuedMessage],
) -> Result<Value, ClientError> {
    validate_document(raw)?;
    let owner = owner.or_else(|| raw.get("cloudAccountId").and_then(Value::as_str));
    let Some(owner) = owner else {
        return Ok(raw.clone());
    };
    let mut result = raw.clone();
    let mut remaining = record(&raw["drafts"]);
    let mut saved = record(&raw["signedOutDrafts"][owner]["drafts"]);
    let queued_values: Vec<_> = queued
        .iter()
        .map(encode_message)
        .collect::<Result<_, _>>()?;
    for (key, draft) in record(&raw["drafts"]) {
        if draft_environment(
            &key,
            known.iter().map(String::as_str),
            &queued_values,
            &draft,
        )
        .is_some_and(|e| environments.contains(&e))
        {
            saved.insert(key.clone(), draft);
            remaining.remove(&key);
        }
    }
    let mut messages = array(&raw["signedOutDrafts"][owner], "queuedMessages");
    for message in queued_values {
        if !environments.contains(text(&message, "environmentId")) {
            continue;
        }
        if let Some(index) = messages
            .iter()
            .position(|m| m["messageId"] == message["messageId"])
        {
            messages[index] = message;
        } else {
            messages.push(message);
        }
    }
    result["drafts"] = remaining.into();
    result["cloudAccountId"] = owner.into();
    if !result["signedOutDrafts"].is_object() {
        result["signedOutDrafts"] = json!({});
    }
    result["signedOutDrafts"][owner] = json!({"drafts":saved,"queuedMessages":messages});
    Ok(result)
}
pub fn restore_cloud(raw: &Value, owner: &str) -> Result<(Value, Vec<QueuedMessage>), ClientError> {
    validate_document(raw)?;
    let mut result = raw.clone();
    let mut drafts = record(&raw["drafts"]);
    let archive = &raw["signedOutDrafts"][owner];
    let mut queued = vec![];
    for message in array(archive, "queuedMessages") {
        queued.push(super::super::outbox::decode_message(message)?);
    }
    for (key, draft) in record(&archive["drafts"]) {
        let restored = if let Some(existing) = drafts.get(&key) {
            merge_recovery(existing, &draft)?
        } else {
            draft
        };
        drafts.insert(key, restored);
    }
    result["drafts"] = drafts.into();
    result["cloudAccountId"] = owner.into();
    if let Some(archives) = result["signedOutDrafts"].as_object_mut() {
        archives.remove(owner);
    }
    Ok((result, queued))
}

fn same_attachment_ids(left: &Value, right: &Value) -> bool {
    let left = array(left, "attachments");
    let right = array(right, "attachments");
    left.len() == right.len() && left.iter().zip(right).all(|(a, b)| a["id"] == b["id"])
}
/// A delivery may finish after sign-out captured its durable archive.
pub fn remove_delivered_cloud_message(
    raw: &Value,
    message: &QueuedMessage,
) -> Result<Value, ClientError> {
    validate_document(raw)?;
    let delivered = encode_message(message)?;
    let mut result = raw.clone();
    for (account, archive) in record(&raw["signedOutDrafts"]) {
        let messages = array(&archive, "queuedMessages");
        let Some(index) = messages.iter().position(|candidate| {
            candidate["environmentId"] == delivered["environmentId"]
                && candidate["messageId"] == delivered["messageId"]
        }) else {
            continue;
        };
        let archived = &messages[index];
        if [
            "commandId",
            "threadId",
            "text",
            "modelSelection",
            "runtimeMode",
            "interactionMode",
            "creation",
        ]
        .iter()
        .any(|key| archived[*key] != delivered[*key])
            || !same_attachment_ids(archived, &delivered)
        {
            continue;
        }
        let editor_key = format!("pending-task:{}", message.message_id.0);
        if let Some(editor) = archive["drafts"].get(&editor_key) {
            let workspace = &editor["workspaceSelection"];
            if editor["text"] != delivered["text"]
                || !same_attachment_ids(editor, &delivered)
                || ["modelSelection", "runtimeMode", "interactionMode"]
                    .iter()
                    .any(|key| {
                        editor
                            .get(*key)
                            .is_some_and(|value| value != &delivered[*key])
                    })
                || (!workspace.is_null()
                    && (["branch", "worktreePath", "pullRequestReference"]
                        .iter()
                        .any(|key| workspace[*key] != delivered["creation"][*key])
                        || workspace["mode"] != delivered["creation"]["workspaceMode"]
                        || (workspace["startFromOrigin"] == true)
                            != (delivered["creation"]["startFromOrigin"] == true)))
            {
                continue;
            }
        }
        let mut kept = messages;
        kept.remove(index);
        result["signedOutDrafts"][&account]["queuedMessages"] = kept.into();
        if let Some(drafts) = result["signedOutDrafts"][&account]["drafts"].as_object_mut() {
            drafts.remove(&editor_key);
        }
    }
    Ok(result)
}
/// Commits the restored archive and newly queued messages together, preserving an edited live queue row.
pub async fn commit_restore(
    storage: &impl Storage,
    expected: &[u8],
    owner: &str,
) -> Result<Value, ClientError> {
    let document = SavedDraftDocument::decode_mobile(expected)?;
    let (next, messages) = restore_cloud(document.raw(), owner)?;
    let key = storage_key(DraftFormat::Mobile);
    let mut transaction = StorageTransaction {
        conditions: vec![StorageCondition {
            area: StorageArea::Ordinary,
            key: key.clone(),
            expected: Some(expected.into()),
        }],
        ordinary: vec![],
        secure: vec![],
    };
    for message in messages {
        let write = super::super::outbox::message_storage_write(&message)?;
        let current = storage
            .get(StorageArea::Ordinary, write.key.clone())
            .await?;
        transaction.conditions.push(StorageCondition {
            area: StorageArea::Ordinary,
            key: write.key.clone(),
            expected: current.clone(),
        });
        if current.is_none() {
            transaction.ordinary.push(write);
        }
    }
    transaction.ordinary.push(StorageWrite {
        key,
        value: Some(serde_json::to_vec(&crate::persistence::Record::new(
            "composer-drafts.mobile",
            None,
            "current",
            1,
            next.clone(),
        ))?),
    });
    if !storage.transaction(transaction).await? {
        return Err(invalid("mobile drafts changed concurrently"));
    }
    Ok(next)
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttachmentReferences {
    pub files: BTreeSet<String>,
    pub uploads: BTreeSet<(String, String)>,
}
fn file_key(uri: &str) -> String {
    super::attachment_files::reference_key(uri)
}
impl AttachmentReferences {
    pub fn retain_owner(&mut self, owner: &Value) {
        for attachment in array(owner, "attachments") {
            if let Some(uri) = attachment["fileUri"].as_str() {
                self.files.insert(file_key(uri));
            }
            if let (Some(environment), Some(id)) = (
                attachment["uploadEnvironmentId"].as_str(),
                attachment["uploadedAttachmentId"].as_str(),
            ) {
                self.uploads.insert((environment.into(), id.into()));
            }
        }
    }
    pub fn from_document(raw: &Value, queued: &[Value], incoming_shares: &[Value]) -> Self {
        let mut result = Self::default();
        for owner in record(&raw["drafts"])
            .values()
            .chain(queued)
            .chain(incoming_shares)
        {
            result.retain_owner(owner);
        }
        for archive in record(&raw["signedOutDrafts"]).values() {
            for owner in record(&archive["drafts"])
                .values()
                .chain(array(archive, "queuedMessages").iter())
            {
                result.retain_owner(owner);
            }
        }
        result
    }
    pub fn unused(&self, candidates: &[Value], all_stores_readable: bool) -> Self {
        if !all_stores_readable {
            return Self::default();
        }
        let mut result = Self::default();
        result.retain_owner(&json!({"attachments":candidates}));
        result.files.retain(|key| !self.files.contains(key));
        result.uploads.retain(|key| !self.uploads.contains(key));
        result
    }

    pub fn unused_with_retention(
        &self,
        candidates: &[Value],
        all_stores_readable: bool,
        retention: &super::attachment_files::AttachmentRetention,
    ) -> Self {
        let mut result = self.unused(candidates, all_stores_readable);
        result.files.retain(|uri| !retention.is_retained(uri));
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_keeps_receipts_and_file_bytes_and_scopes_legacy_project() {
        let raw = json!({"schemaVersion":1,"drafts":{"new-task:host:local:p":{"text":"saved","attachments":[{"type":"file","id":"f","name":"a.txt","mimeType":"text/plain","sizeBytes":3,"fileUri":"file:///x"}]},"receipt":{"text":"","attachments":[],"importedShareIds":["s"]},"e:t~queued-edit~x":{"text":"temporary","attachments":[]}},"unknown":true});
        let result = migrate_document(&raw, "now", || "generated".into()).expect("migrate");
        assert_eq!(
            result["drafts"]["new-task:generated"]["project"]["environmentId"],
            "host:local"
        );
        assert!(
            result["drafts"]["new-task:generated"]["text"]
                .as_str()
                .expect("text")
                .contains("t3-context://v1/file/f")
        );
        assert!(result["drafts"].get("receipt").is_some());
        assert!(result["drafts"].get("e:t~queued-edit~x").is_none());
        assert_eq!(result["unknown"], true);
    }
    #[test]
    fn cloud_restore_merges_edits_and_does_not_apply_send_caps() {
        let raw = json!({"schemaVersion":1,"drafts":{"e:t":{"text":"edited","attachments":[{"id":"a"}]}},"signedOutDrafts":{"one":{"drafts":{"e:t":{"text":"old","attachments":[{"id":"b"}],"future":true}},"queuedMessages":[]}},"cloudAccountId":"one"});
        let (restored, queued) = restore_cloud(&raw, "one").expect("restore");
        assert!(queued.is_empty());
        assert_eq!(restored["drafts"]["e:t"]["text"], "edited\n\nold");
        assert_eq!(array(&restored["drafts"]["e:t"], "attachments").len(), 2);
        assert!(restored["signedOutDrafts"].get("one").is_none());
        let archived = archive_cloud(
            &restored,
            None,
            &BTreeSet::from(["e".into()]),
            &["e".into()],
            &[],
        )
        .expect("archive");
        assert!(archived["drafts"].as_object().expect("drafts").is_empty());
        assert_eq!(
            archived["signedOutDrafts"]["one"]["drafts"]["e:t"]["future"],
            true
        );
    }
    #[test]
    fn attachment_ownership_includes_archives_and_requires_complete_load() {
        let attachment =
            json!({"fileUri":"file:///owned","uploadEnvironmentId":"e","uploadedAttachmentId":"u"});
        let raw = json!({"drafts":{},"signedOutDrafts":{"a":{"drafts":{"d":{"attachments":[attachment.clone()]}},"queuedMessages":[]}}});
        let refs = AttachmentReferences::from_document(&raw, &[], &[]);
        assert!(refs.unused(&[attachment], true).files.is_empty());
        assert!(
            refs.unused(&[json!({"fileUri":"file:///unknown"})], false)
                .files
                .is_empty()
        );
        assert_eq!(
            refs.unused(&[json!({"fileUri":"file:///unused"})], true)
                .files
                .len(),
            1
        );
    }

    #[test]
    fn late_delivery_removes_only_matching_archive_and_untouched_editor() {
        let message=super::super::super::outbox::decode_message(json!({"schemaVersion":3,"environmentId":"e","threadId":"t","messageId":"m","commandId":"c","text":"sent","attachments":[],"createdAt":"now"})).expect("queued");
        let sent = encode_message(&message).expect("encode");
        let raw = json!({"schemaVersion":1,"drafts":{},"signedOutDrafts":{
            "same":{"drafts":{"pending-task:m":{"text":"sent","attachments":[]}},"queuedMessages":[sent]},
            "edited":{"drafts":{"pending-task:m":{"text":"new edit","attachments":[]}},"queuedMessages":[sent]},
            "changed":{"drafts":{},"queuedMessages":[{ "schemaVersion":3,"environmentId":"e","threadId":"t","messageId":"m","commandId":"c2","text":"sent","attachments":[],"createdAt":"now"}]}
        }});
        let next = remove_delivered_cloud_message(&raw, &message).expect("reconcile");
        assert!(array(&next["signedOutDrafts"]["same"], "queuedMessages").is_empty());
        assert!(record(&next["signedOutDrafts"]["same"]["drafts"]).is_empty());
        assert_eq!(
            next["signedOutDrafts"]["edited"],
            raw["signedOutDrafts"]["edited"]
        );
        assert_eq!(
            next["signedOutDrafts"]["changed"],
            raw["signedOutDrafts"]["changed"]
        );
    }
}
