//! Prompt stash with lossless context records and explicit durability outcomes.
//! The caller reads the latest stored entries before each operation and supplies an atomic writer.
//! Never clear composer content until `written && durable`; retain/quarantine invalid source bytes.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
pub const STORAGE_KEY: &str = "t3code:prompt-stash:v2";
pub const LEGACY_STORAGE_KEY: &str = "t3code:prompt-stash:v1";
pub const MAX_ENTRIES: usize = 20;
pub const MAX_ATTACHMENT_CHARS: usize = 2_700_000;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedImage {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: f64,
    pub data_url: String,
    #[serde(flatten)]
    pub metadata: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedFile {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: f64,
    pub attachment_id: String,
    pub environment_id: t3_contracts::base_schemas::EnvironmentId,
    #[serde(flatten)]
    pub metadata: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub created_at: String,
    pub prompt: String,
    pub attachments: Vec<SavedImage>,
    pub dropped_image_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<SavedFile>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreadable_image_names: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_image_count: Option<f64>,
    /// Composer records are a forward-compatible persisted format, including unknown variants.
    #[serde(flatten)]
    pub metadata: BTreeMap<String, Value>,
}
pub fn partition_attachments(attachments: Vec<SavedImage>) -> (Vec<SavedImage>, Vec<String>) {
    let mut used = 0;
    let mut kept = vec![];
    let mut dropped = vec![];
    for a in attachments {
        let len = a.data_url.encode_utf16().count();
        if used + len > MAX_ATTACHMENT_CHARS {
            dropped.push(a.name);
        } else {
            used += len;
            kept.push(a);
        }
    }
    (kept, dropped)
}
pub fn decode(raw: &str) -> Result<Vec<Entry>, serde_json::Error> {
    let v: Value = serde_json::from_str(raw)?;
    let mut entries: Vec<Entry> = serde_json::from_value(v["state"]["entries"].clone())?;
    for entry in &mut entries {
        if let Some(count) = entry.pending_image_count.filter(|n| *n != 0.0) {
            let count = if count.is_finite() && count > 0.0 {
                count.floor() as usize
            } else {
                0
            };
            let names = entry.unreadable_image_names.get_or_insert_default();
            names.extend((0..count).map(|i| format!("image {} (not saved before reload)", i + 1)));
            entry.pending_image_count = Some(0.0);
        }
    }
    Ok(entries)
}
pub fn encode(entries: &[Entry]) -> Result<String, serde_json::Error> {
    serde_json::to_string(&serde_json::json!({"version":2,"state":{"entries":entries}}))
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriteOutcome {
    pub written: bool,
    pub durable: bool,
}
pub struct StashOutcome {
    pub evicted: Option<Entry>,
    pub write: WriteOutcome,
}
#[derive(Debug, Clone, Default)]
pub struct Stash {
    pub entries: Vec<Entry>,
}
impl Stash {
    fn latest(&self, stored: Option<&[Entry]>) -> Vec<Entry> {
        stored.map_or_else(
            || self.entries.clone(),
            |entries| {
                entries
                    .iter()
                    .map(|e| {
                        self.entries
                            .iter()
                            .find(|m| m.id == e.id)
                            .unwrap_or(e)
                            .clone()
                    })
                    .collect()
            },
        )
    }
    pub async fn stash(
        &mut self,
        entry: Entry,
        stored: Option<&[Entry]>,
        persist: impl AsyncFnOnce(&[Entry]) -> WriteOutcome,
    ) -> StashOutcome {
        let mut next = self.latest(stored);
        next.insert(0, entry);
        let evicted = if next.len() > MAX_ENTRIES {
            next.pop()
        } else {
            None
        };
        let write = persist(&next).await;
        if !write.written {
            return StashOutcome {
                evicted: None,
                write: WriteOutcome::default(),
            };
        }
        self.entries = next;
        StashOutcome { evicted, write }
    }
    pub async fn take(
        &mut self,
        id: &str,
        stored: Option<&[Entry]>,
        persist: impl AsyncFnOnce(&[Entry]) -> WriteOutcome,
    ) -> (Option<Entry>, bool) {
        let mut next = self.latest(stored);
        let Some(index) = next.iter().position(|e| e.id == id) else {
            return (None, true);
        };
        let entry = next.remove(index);
        let write = persist(&next).await;
        self.entries = next;
        (Some(entry), write.written && write.durable)
    }
    pub async fn finalize_images(
        &mut self,
        id: &str,
        attachments: Vec<SavedImage>,
        dropped: Vec<String>,
        unreadable: Vec<String>,
        stored: Option<&[Entry]>,
        persist: impl AsyncFnOnce(&[Entry]) -> WriteOutcome,
    ) -> (bool, bool) {
        let mut next = self.latest(stored);
        let Some(entry) = next.iter_mut().find(|e| e.id == id) else {
            return (false, true);
        };
        entry.attachments = attachments;
        entry.dropped_image_names = dropped;
        entry.unreadable_image_names = Some(unreadable);
        entry.pending_image_count = Some(0.0);
        let write = persist(&next).await;
        self.entries = next;
        (true, write.written && write.durable)
    }
}
