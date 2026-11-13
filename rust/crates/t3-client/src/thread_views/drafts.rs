//! Lossless saved-draft readers and record-level concurrent-write merging.
use crate::{
    ClientError,
    platform::{
        Storage, StorageArea, StorageCondition, StorageKey, StorageTransaction, StorageWrite,
    },
    types::{CommandId, ThreadKey},
};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
pub const WEB_STORAGE_KEY: &str = "t3code:composer-drafts:v1";
pub const WEB_VERSION: u64 = 9;
pub const WEB_DEBOUNCE_MS: u64 = 300;
pub const MOBILE_DEBOUNCE_MS: u64 = 200;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftFormat {
    Web,
    Mobile,
}
/// Original JSON is retained, including unknown fields and every historical model/context representation.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedDraftDocument {
    format: DraftFormat,
    raw: Value,
}
fn invalid(message: &str) -> ClientError {
    ClientError::InvalidPayload(message.into())
}
fn object(value: &Value) -> Result<&Map<String, Value>, ClientError> {
    value
        .as_object()
        .ok_or_else(|| invalid("draft document must be an object"))
}
impl SavedDraftDocument {
    pub fn decode_web(bytes: &[u8]) -> Result<Self, ClientError> {
        let raw = decode_record_payload(bytes, "composer-drafts.web", 9)?;
        let envelope = object(&raw)?;
        if envelope
            .get("version")
            .is_some_and(|version| !version.is_number())
            || !envelope.contains_key("state")
        {
            return Err(invalid("invalid web draft envelope"));
        }
        Ok(Self {
            format: DraftFormat::Web,
            raw,
        })
    }
    pub fn decode_mobile(bytes: &[u8]) -> Result<Self, ClientError> {
        let raw = decode_record_payload(bytes, "composer-drafts.mobile", 1)?;
        let envelope = object(&raw)?;
        if envelope.get("schemaVersion") != Some(&json!(1)) {
            return Err(invalid("unsupported mobile draft version"));
        }
        if !envelope.get("drafts").is_some_and(Value::is_object) {
            return Err(invalid("mobile draft map is missing"));
        }
        Ok(Self {
            format: DraftFormat::Mobile,
            raw,
        })
    }
    pub fn raw(&self) -> &Value {
        &self.raw
    }
    pub fn encode(&self) -> Result<Vec<u8>, ClientError> {
        Ok(serde_json::to_vec(&self.raw)?)
    }
    pub fn format(&self) -> DraftFormat {
        self.format
    }
    pub fn state(&self) -> &Value {
        match self.format {
            DraftFormat::Web => &self.raw["state"],
            DraftFormat::Mobile => &self.raw,
        }
    }
    pub fn drafts(&self) -> Option<&Map<String, Value>> {
        let state = self.state();
        match self.format {
            DraftFormat::Mobile => state.get("drafts")?.as_object(),
            DraftFormat::Web => state
                .get("draftsByThreadKey")
                .or_else(|| state.get("draftsByThreadId"))?
                .as_object(),
        }
    }
    pub fn sessions(&self) -> Option<&Map<String, Value>> {
        let state = self.state();
        state
            .get("draftThreadsByThreadKey")
            .or_else(|| state.get("draftThreadsByThreadId"))?
            .as_object()
    }
    pub fn requires_web_migration(&self) -> bool {
        self.format == DraftFormat::Web
            && (self.raw["version"] != WEB_VERSION
                || [
                    "draftsByThreadKey",
                    "draftThreadsByThreadKey",
                    "logicalProjectDraftThreadKeyByLogicalProjectKey",
                ]
                .iter()
                .any(|k| !self.state().get(k).is_some_and(Value::is_object)))
    }
    pub fn replace_current_web_state(&self, state: Value) -> Result<Self, ClientError> {
        if self.format != DraftFormat::Web {
            return Err(invalid("web state requires a web draft document"));
        }
        for key in [
            "draftsByThreadKey",
            "draftThreadsByThreadKey",
            "logicalProjectDraftThreadKeyByLogicalProjectKey",
        ] {
            if !state.get(key).is_some_and(Value::is_object) {
                return Err(invalid("draft maps are missing"));
            }
        }
        let mut raw = self.raw.clone();
        raw["state"] = state;
        raw["version"] = json!(WEB_VERSION);
        Ok(Self {
            format: self.format,
            raw,
        })
    }
    pub fn migrate_web(&self, now: &str) -> Result<Self, ClientError> {
        if self.format != DraftFormat::Web {
            return Err(invalid("web migration requires a web document"));
        }
        self.replace_current_web_state(super::composer::web_drafts::normalize_state(
            self.state(),
            now,
            self.raw["version"] != WEB_VERSION,
        ))
    }
}
fn decode_record_payload(bytes: &[u8], kind: &str, version: u32) -> Result<Value, ClientError> {
    let value: Value = serde_json::from_slice(bytes)?;
    if value.get("formatVersion").is_none() {
        return Ok(value);
    }
    let record: crate::persistence::Record<Value> = serde_json::from_value(value)?;
    if record.format_version != crate::persistence::FORMAT_VERSION
        || record.kind != kind
        || record.record_id != "current"
        || record.environment_id.is_some()
        || record.payload_version != version
    {
        return Err(invalid("draft record identity/version mismatch"));
    }
    Ok(record.payload)
}
pub fn storage_key(format: DraftFormat) -> StorageKey {
    StorageKey {
        namespace: match format {
            DraftFormat::Web => "composer-drafts.web",
            DraftFormat::Mobile => "composer-drafts.mobile",
        }
        .into(),
        environment: None,
        record: "current".into(),
    }
}
pub fn web_has_user_content(draft: &Value) -> bool {
    draft
        .get("prompt")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.trim().is_empty())
        || [
            "attachments",
            "files",
            "images",
            "persistedAttachments",
            "terminalContexts",
            "previewAnnotations",
            "reviewComments",
            "threadContexts",
            "environmentContexts",
        ]
        .iter()
        .any(|k| {
            draft
                .get(k)
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty())
        })
        || draft.get("replyTo").is_some_and(|v| !v.is_null())
}
pub(crate) fn merge_map(
    stored: Option<&Value>,
    local: Option<&Value>,
    baseline: Option<&Value>,
) -> Value {
    let mut merged = stored
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let local = local.and_then(Value::as_object);
    let baseline = baseline.and_then(Value::as_object);
    let keys: BTreeSet<_> = local
        .into_iter()
        .flat_map(|m| m.keys())
        .chain(baseline.into_iter().flat_map(|m| m.keys()))
        .collect();
    for key in keys {
        let next = local.and_then(|m| m.get(key));
        if next == baseline.and_then(|m| m.get(key)) {
            continue;
        }
        if let Some(value) = next {
            merged.insert(key.clone(), value.clone());
        } else {
            merged.remove(key);
        }
    }
    Value::Object(merged)
}
pub fn merge_web_states(
    stored: &Value,
    local: &Value,
    baseline: &Value,
) -> Result<Value, ClientError> {
    object(stored)?;
    object(local)?;
    object(baseline)?;
    let mut merged = object(stored)?.clone();
    for key in [
        "draftsByThreadKey",
        "draftThreadsByThreadKey",
        "logicalProjectDraftThreadKeyByLogicalProjectKey",
    ] {
        merged.insert(
            key.into(),
            merge_map(stored.get(key), local.get(key), baseline.get(key)),
        );
    }
    for key in [
        "stickyModelSelectionByProvider",
        "stickyOptionsByModelByProvider",
        "stickyActiveProvider",
    ] {
        let next = if local.get(key) == baseline.get(key) {
            stored.get(key)
        } else {
            local.get(key)
        };
        if let Some(value) = next {
            merged.insert(key.into(), value.clone());
        } else {
            merged.remove(key);
        }
    }
    Ok(Value::Object(merged))
}
pub struct WebDraftPersistence {
    baseline: SavedDraftDocument,
    pending: Option<(SavedDraftDocument, u64)>,
}
impl WebDraftPersistence {
    pub fn new(loaded: SavedDraftDocument) -> Result<Self, ClientError> {
        Self::new_at(loaded, "1970-01-01T00:00:00.000Z")
    }
    pub fn new_at(loaded: SavedDraftDocument, now: &str) -> Result<Self, ClientError> {
        if loaded.format() != DraftFormat::Web {
            return Err(invalid("web draft persistence requires a web document"));
        }
        Ok(Self {
            baseline: loaded.migrate_web(now)?,
            pending: None,
        })
    }
    pub fn schedule(&mut self, local: SavedDraftDocument, now: u64) -> Result<(), ClientError> {
        let local = local.migrate_web("1970-01-01T00:00:00.000Z")?;
        self.pending = Some((local, now.saturating_add(WEB_DEBOUNCE_MS)));
        Ok(())
    }
    pub fn deadline(&self) -> Option<u64> {
        self.pending.as_ref().map(|(_, at)| *at)
    }
    pub async fn flush(
        &mut self,
        storage: &impl Storage,
        key: StorageKey,
    ) -> Result<bool, ClientError> {
        let Some((local, _)) = &self.pending else {
            return Ok(false);
        };
        if key != storage_key(DraftFormat::Web) {
            return Err(invalid("web draft storage identity mismatch"));
        }
        let stored = storage.get(StorageArea::Ordinary, key.clone()).await?;
        let decoded = stored
            .as_deref()
            .map(SavedDraftDocument::decode_web)
            .transpose()?;
        let state = if let Some(stored) = decoded {
            let stored = stored.migrate_web("1970-01-01T00:00:00.000Z")?;
            merge_web_states(stored.state(), local.state(), self.baseline.state())?
        } else {
            local.state().clone()
        };
        let merged = local.replace_current_web_state(state)?;
        let record = crate::persistence::Record::new(
            "composer-drafts.web",
            None,
            "current",
            9,
            merged.raw().clone(),
        );
        let committed = storage
            .transaction(StorageTransaction {
                conditions: vec![StorageCondition {
                    area: StorageArea::Ordinary,
                    key: key.clone(),
                    expected: stored,
                }],
                ordinary: vec![StorageWrite {
                    key,
                    value: Some(serde_json::to_vec(&record)?),
                }],
                secure: vec![],
            })
            .await?;
        if !committed {
            return Err(invalid(
                "draft state changed concurrently; retry the pending write",
            ));
        }
        self.baseline = local.clone();
        self.pending = None;
        Ok(true)
    }
}
pub fn merge_text(existing: &str, incoming: &str) -> String {
    if incoming.is_empty() || existing == incoming || existing.ends_with(&format!("\n\n{incoming}"))
    {
        existing.into()
    } else if existing.is_empty() {
        incoming.into()
    } else {
        format!("{existing}\n\n{incoming}")
    }
}
pub fn merge_recovery(existing: &Value, incoming: &Value) -> Result<Value, ClientError> {
    let mut merged = object(incoming)?.clone();
    merged.extend(object(existing)?.clone());
    let text = merge_text(
        existing
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        incoming
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    merged.insert("text".into(), text.into());
    let mut attachments = existing
        .get("attachments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut ids: BTreeSet<_> = attachments
        .iter()
        .filter_map(|a| a.get("id").and_then(Value::as_str).map(str::to_string))
        .collect();
    for attachment in incoming
        .get("attachments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(id) = attachment.get("id").and_then(Value::as_str)
            && !ids.insert(id.into())
        {
            continue;
        }
        attachments.push(attachment.clone());
    }
    merged.insert("attachments".into(), attachments.into());
    let mut receipts = existing
        .get("importedShareIds")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for receipt in incoming
        .get("importedShareIds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if !receipts.contains(receipt) {
            receipts.push(receipt.clone());
        }
    }
    if !receipts.is_empty() {
        merged.insert("importedShareIds".into(), receipts.into());
    }
    let mut records = vec![];
    for context in [incoming.get("context"), existing.get("context")]
        .into_iter()
        .flatten()
    {
        for record in context
            .get("records")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(id) = record.get("contextId")
                && let Some(at) = records
                    .iter()
                    .position(|r: &Value| r.get("contextId") == Some(id))
            {
                records[at] = record.clone();
                continue;
            }
            records.push(record.clone());
        }
    }
    if !records.is_empty() {
        merged.insert("context".into(), json!({"version":1,"records":records}));
    }
    Ok(merged.into())
}

#[derive(Debug, Clone, PartialEq)]
pub struct SendSnapshot {
    pub command: CommandId,
    pub thread: ThreadKey,
    pub draft: Rc<Value>,
}
#[derive(Default)]
pub struct OptimisticSends {
    pub drafts: BTreeMap<ThreadKey, Rc<Value>>,
    pub pending: BTreeMap<CommandId, SendSnapshot>,
    pub failed: BTreeMap<CommandId, SendSnapshot>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureRecovery {
    Restored,
    KeptConcurrentDraft,
    Missing,
}
impl OptimisticSends {
    pub fn begin(
        &mut self,
        command: CommandId,
        thread: ThreadKey,
        cleared: Value,
    ) -> Result<SendSnapshot, ClientError> {
        if self.pending.contains_key(&command) || self.failed.contains_key(&command) {
            return Err(invalid("duplicate send command"));
        }
        let draft = self
            .drafts
            .get(&thread)
            .cloned()
            .ok_or_else(|| invalid("send draft is missing"))?;
        let snapshot = SendSnapshot {
            command: command.clone(),
            thread: thread.clone(),
            draft,
        };
        self.pending.insert(command, snapshot.clone());
        self.drafts.insert(thread, Rc::new(cleared));
        Ok(snapshot)
    }
    pub fn accepted(&mut self, command: &CommandId) -> Option<SendSnapshot> {
        self.pending.remove(command)
    }
    pub fn rejected(&mut self, command: &CommandId) -> FailureRecovery {
        let Some(snapshot) = self.pending.remove(command) else {
            return FailureRecovery::Missing;
        };
        let concurrent = self
            .drafts
            .get(&snapshot.thread)
            .is_some_and(|d| web_has_user_content(d));
        if concurrent {
            self.failed.insert(command.clone(), snapshot);
            FailureRecovery::KeptConcurrentDraft
        } else {
            self.drafts.insert(snapshot.thread.clone(), snapshot.draft);
            FailureRecovery::Restored
        }
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{
        testing::FakePlatform,
        types::{EnvironmentId, ThreadId},
    };
    use futures::executor::block_on;
    fn current() -> Value {
        json!({"version":9,"state":{"draftsByThreadKey":{},"draftThreadsByThreadKey":{},"logicalProjectDraftThreadKeyByLogicalProjectKey":{}}})
    }
    fn doc(value: &Value) -> SavedDraftDocument {
        SavedDraftDocument::decode_web(&serde_json::to_vec(value).unwrap()).unwrap()
    }
    #[test]
    fn historical_envelopes_are_lossless() {
        for version in 0..=9 {
            let value = json!({"version":version,"state":{"draftsByThreadId":{"t":{"prompt":"saved 😀","attachments":[{"dataUrl":"data:image/png,user-bytes"}],"modelOptions":{"codex":{"effort":"high"}}}},"draftThreadsByThreadId":{},"projectDraftThreadIdByProjectId":{},"future":true}});
            let loaded = doc(&value);
            assert_eq!(
                serde_json::from_slice::<Value>(&loaded.encode().unwrap()).unwrap(),
                value
            );
            assert_eq!(loaded.drafts().unwrap()["t"]["prompt"], "saved 😀");
            assert!(loaded.requires_web_migration());
        }
    }
    #[test]
    fn mobile_keeps_context_over_send_cap_and_cloud_outbox() {
        let value = json!({"schemaVersion":1,"drafts":{"t":{"text":"saved","attachments":[],"context":{"version":1,"records":(0..300).map(|i|json!({"kind":"future","contextId":i})).collect::<Vec<_>>()}}},"signedOutDrafts":{"account":{"drafts":{},"queuedMessages":[{"schemaVersion":4,"attachments":[{"fileUri":"saved"}]}]}}});
        let d = SavedDraftDocument::decode_mobile(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(d.raw(), &value);
        assert_eq!(
            d.drafts().unwrap()["t"]["context"]["records"]
                .as_array()
                .unwrap()
                .len(),
            300
        );
    }
    #[test]
    fn cross_tab_record_merge_deletion_and_sticky_whole_field() {
        let mut baseline = current()["state"].clone();
        baseline["draftsByThreadKey"] = json!({"a":{"prompt":"a"},"b":{"prompt":"b"}});
        baseline["stickyActiveProvider"] = json!("codex");
        let mut local = baseline.clone();
        local["draftsByThreadKey"]
            .as_object_mut()
            .unwrap()
            .remove("a");
        local["draftsByThreadKey"]["b"] = json!({"prompt":"local"});
        local
            .as_object_mut()
            .unwrap()
            .remove("stickyActiveProvider");
        let mut stored = baseline.clone();
        stored["draftsByThreadKey"]["a"] = json!({"prompt":"other tab"});
        stored["draftsByThreadKey"]["c"] = json!({"prompt":"new"});
        let merged = merge_web_states(&stored, &local, &baseline).unwrap();
        assert!(merged["draftsByThreadKey"].get("a").is_none());
        assert_eq!(merged["draftsByThreadKey"]["b"]["prompt"], "local");
        assert_eq!(merged["draftsByThreadKey"]["c"]["prompt"], "new");
        assert!(merged.get("stickyActiveProvider").is_none());
    }
    #[test]
    fn flush_reads_latest_stored_and_updates_only_after_commit() {
        block_on(async {
            let p = FakePlatform::default();
            let key = storage_key(DraftFormat::Web);
            let initial = doc(&current());
            let mut local = current();
            local["state"]["draftsByThreadKey"]["a"] = json!({"prompt":"mine"});
            let mut stored = current();
            stored["state"]["draftsByThreadKey"]["b"] = json!({"prompt":"theirs"});
            p.write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: key.clone(),
                    value: Some(doc(&stored).encode().unwrap()),
                }],
            )
            .await
            .unwrap();
            let mut persistence = WebDraftPersistence::new(initial).unwrap();
            persistence.schedule(doc(&local), 10).unwrap();
            assert_eq!(persistence.deadline(), Some(310));
            assert!(persistence.flush(&p, key.clone()).await.unwrap());
            let result = SavedDraftDocument::decode_web(
                &p.get(StorageArea::Ordinary, key).await.unwrap().unwrap(),
            )
            .unwrap();
            assert_eq!(result.drafts().unwrap().len(), 2);
            assert!(persistence.deadline().is_none());
        });
    }
    #[test]
    fn legacy_write_preserves_content_and_upgrades_version() {
        let original = json!({"version":1,"state":{"draftsByThreadId":{"t":{"prompt":"keep"}}}});
        let loaded = doc(&original);
        let mut persistence = WebDraftPersistence::new(loaded).unwrap();
        assert_eq!(
            persistence.baseline.drafts().unwrap()["t"]["prompt"],
            "keep"
        );
        assert!(!persistence.baseline.requires_web_migration());
        persistence
            .schedule(persistence.baseline.clone(), 0)
            .unwrap();
    }
    #[test]
    fn recovery_does_not_drop_overflow_and_keeps_edits() {
        let existing = json!({"text":"new","attachments":[{"id":"a"}],"runtimeMode":"full-access","context":{"version":1,"records":[{"contextId":"ctx","label":"new"}]}});
        let incoming = json!({"text":"failed","attachments":(0..100).map(|i|json!({"id":format!("b{i}")})).collect::<Vec<_>>(),"runtimeMode":"auto","context":{"version":1,"records":[{"contextId":"ctx","label":"old"}]}});
        let merged = merge_recovery(&existing, &incoming).unwrap();
        assert_eq!(merged["text"], "new\n\nfailed");
        assert_eq!(merged["attachments"].as_array().unwrap().len(), 101);
        assert_eq!(merged["runtimeMode"], "full-access");
        assert_eq!(merged["context"]["records"][0]["label"], "new");
        assert_eq!(merge_text("new\n\nfailed", "failed"), "new\n\nfailed");
    }
    #[test]
    fn web_optimistic_failures_preserve_current_and_failed_drafts() {
        let key = ThreadKey {
            environment: EnvironmentId("e".into()),
            thread: ThreadId("t".into()),
        };
        let command = CommandId("c".into());
        let mut sends = OptimisticSends::default();
        sends.drafts.insert(
            key.clone(),
            Rc::new(json!({"prompt":"sent","attachments":[{"dataUrl":"bytes"}]})),
        );
        sends
            .begin(
                command.clone(),
                key.clone(),
                json!({"prompt":"","attachments":[]}),
            )
            .unwrap();
        sends.drafts.insert(
            key.clone(),
            Rc::new(json!({"prompt":"typed during send","attachments":[]})),
        );
        assert_eq!(
            sends.rejected(&command),
            FailureRecovery::KeptConcurrentDraft
        );
        assert_eq!(sends.drafts[&key]["prompt"], "typed during send");
        assert_eq!(sends.failed[&command].draft["prompt"], "sent");
        let next = CommandId("d".into());
        sends
            .begin(
                next.clone(),
                key.clone(),
                json!({"prompt":"","attachments":[]}),
            )
            .unwrap();
        assert_eq!(sends.rejected(&next), FailureRecovery::Restored);
        assert_eq!(sends.drafts[&key]["prompt"], "typed during send");
    }
    #[test]
    fn model_only_is_not_content() {
        assert!(!web_has_user_content(
            &json!({"prompt":"  ","modelSelectionByProvider":{"codex":{"model":"x"}}})
        ));
        assert!(web_has_user_content(
            &json!({"prompt":"","replyTo":{"contextId":"r"}})
        ));
    }
}
