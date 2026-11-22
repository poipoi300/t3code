use crate::error::Result;
use async_trait::async_trait;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};
use t3_contracts::{base_schemas::*, orchestration_v2::*};
pub use t3_shared::find_text::NativeThreadFindText;

/// Local projection shim: the store owns fork visibility and the single read transaction.
#[async_trait]
pub trait ThreadFindSnapshotReader: Send + Sync {
    async fn snapshot(&self, thread_id: ThreadId) -> Result<FindSnapshot>;
}
pub struct FindSnapshot {
    pub cwd: Option<String>,
    pub sequence: u64,
    pub payloads: Vec<FindPayload>,
    pub tool_payloads: Vec<String>,
}
pub struct FindPayload {
    pub position: u64,
    pub entry_id: String,
    pub run_id: Option<RunId>,
    pub item_type: String,
    pub payload: String,
}
/// Local shared-helper shim; search text must be what the client actually renders.
pub trait ThreadFindText: Send + Sync {
    fn segments(
        &self,
        item: &OrchestrationV2TurnItem,
        cwd: Option<&str>,
        skills: &[SearchThreadInputSkillsItem],
    ) -> Result<Vec<String>>;
    fn occurrences(&self, text: &str, query: &str) -> u64;
    fn embeds_result(&self, item: &OrchestrationV2TurnItem) -> bool;
}
#[derive(Clone)]
pub struct ThreadFind {
    reader: Arc<dyn ThreadFindSnapshotReader>,
    text: Arc<dyn ThreadFindText>,
    caches: Arc<FindCaches>,
}
impl ThreadFind {
    pub fn new(reader: Arc<dyn ThreadFindSnapshotReader>, text: Arc<dyn ThreadFindText>) -> Self {
        Self {
            reader,
            text,
            caches: Arc::new(FindCaches::default()),
        }
    }
    pub async fn search_thread_stream(
        &self,
        input: OrchestrationV2SearchThreadInput,
    ) -> Result<ThreadFindSubscription> {
        let lease = self.caches.acquire();
        let snapshot = self.reader.snapshot(input.thread_id.clone()).await?;
        let cache_key = serde_json::to_string(&(
            &input.thread_id,
            &snapshot.cwd,
            &input.query,
            &input.skills.as_deref().unwrap_or(&[]),
        ))?;
        let cached = self.caches.completed(&cache_key, snapshot.sequence);
        if let Some(documents) = cached {
            let frame = result_for(&documents, &input, snapshot.sequence);
            return Ok(ThreadFindSubscription {
                input,
                sequence: snapshot.sequence,
                cwd: snapshot.cwd,
                rows: vec![],
                documents: BTreeMap::new(),
                embedded: Default::default(),
                text: self.text.clone(),
                first: false,
                ended: false,
                cached_result: Some(frame),
                cache_key,
                lease: Some(lease),
            });
        }
        let mut folded = BTreeMap::new();
        for row in snapshot
            .payloads
            .iter()
            .filter(|row| row.item_type == "user_input_request")
        {
            let decoded: OrchestrationV2TurnItem = crate::stores::event_store::decode_json(
                crate::stores::event_store::parse_json(&row.payload)?,
            )?;
            let item = serde_json::to_value(decoded)?;
            if let Some(request) = item["questionAnswer"]["requestId"].as_str() {
                folded.insert(format!("async-answer:{request}"), row.run_id.clone());
            }
        }
        let mut embedded = std::collections::BTreeSet::new();
        for payload in &snapshot.tool_payloads {
            let item: OrchestrationV2TurnItem = crate::stores::event_store::decode_json(
                crate::stores::event_store::parse_json(payload)?,
            )?;
            if self.text.embeds_result(&item) {
                let value = serde_json::to_value(&item)?;
                embedded.insert(value["runId"].as_str().map(|id| RunId(id.into())));
            }
        }
        let rows = snapshot
            .payloads
            .into_iter()
            .filter(|row| {
                row.item_type != "user_input_request"
                    && !(row.item_type == "user_message"
                        && folded.get(&row.entry_id) == Some(&row.run_id))
            })
            .collect();
        Ok(ThreadFindSubscription {
            input,
            sequence: snapshot.sequence,
            cwd: snapshot.cwd,
            rows,
            documents: BTreeMap::new(),
            embedded,
            text: self.text.clone(),
            first: true,
            ended: false,
            cached_result: None,
            cache_key,
            lease: Some(lease),
        })
    }
    pub async fn search_thread(
        &self,
        input: OrchestrationV2SearchThreadInput,
    ) -> Result<OrchestrationV2SearchThreadResult> {
        let mut stream = self.search_thread_stream(input).await?;
        let mut result = None;
        while let Some(frame) = stream.pull().await? {
            result = Some(frame);
        }
        result.ok_or(crate::error::Error::Unsupported("empty thread find stream"))
    }
}
#[derive(Clone)]
struct Document {
    entry_id: String,
    run_id: Option<RunId>,
    count: u64,
}
#[derive(Default)]
struct CacheState {
    users: usize,
    generation: u64,
    text: VecDeque<(String, Vec<String>)>,
    large_text: VecDeque<(String, Vec<String>)>,
    completed: VecDeque<(String, u64, Vec<Document>)>,
}
#[derive(Default)]
struct FindCaches {
    state: Mutex<CacheState>,
    idle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    #[cfg(test)]
    expired: tokio::sync::Notify,
}
impl FindCaches {
    fn acquire(self: &Arc<Self>) -> CacheLease {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.users += 1;
        state.generation = state.generation.wrapping_add(1);
        drop(state);
        if let Some(task) = self.idle.lock().unwrap_or_else(|e| e.into_inner()).take() {
            task.abort();
        }
        CacheLease(self.clone())
    }
    fn completed(&self, key: &str, sequence: u64) -> Option<Vec<Document>> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let index = state
            .completed
            .iter()
            .position(|(candidate, revision, _)| candidate == key && *revision == sequence)?;
        let entry = state.completed.remove(index)?;
        let documents = entry.2.clone();
        state.completed.push_back(entry);
        Some(documents)
    }
    fn finish(&self, key: String, sequence: u64, documents: Vec<Document>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state
            .completed
            .retain(|(candidate, _, _)| *candidate != key);
        state.completed.push_back((key, sequence, documents));
        if state.completed.len() > 8 {
            state.completed.pop_front();
        }
    }
    fn parsed(&self, key: &str, large: bool) -> Option<Vec<String>> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let cache = if large {
            &mut state.large_text
        } else {
            &mut state.text
        };
        let index = cache.iter().position(|(candidate, _)| candidate == key)?;
        let entry = cache.remove(index)?;
        let segments = entry.1.clone();
        cache.push_back(entry);
        Some(segments)
    }
    fn insert_parsed(&self, key: String, large: bool, segments: Vec<String>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let cache = if large {
            &mut state.large_text
        } else {
            &mut state.text
        };
        cache.retain(|(candidate, _)| *candidate != key);
        cache.push_back((key, segments));
        if cache.len() > if large { 32 } else { 8192 } {
            cache.pop_front();
        }
    }
}
impl Drop for FindCaches {
    fn drop(&mut self) {
        if let Some(task) = self
            .idle
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            task.abort();
        }
    }
}
struct CacheLease(Arc<FindCaches>);
impl Drop for CacheLease {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.users -= 1;
        if state.users > 0 {
            return;
        }
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        drop(state);
        let weak = Arc::downgrade(&self.0);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
            let task = handle.spawn(async move {
                tokio::time::sleep_until(deadline).await;
                if let Some(caches) = weak.upgrade() {
                    let mut state = caches.state.lock().unwrap_or_else(|e| e.into_inner());
                    if state.users == 0 && state.generation == generation {
                        state.text.clear();
                        state.large_text.clear();
                        state.completed.clear();
                        #[cfg(test)]
                        caches.expired.notify_one();
                    }
                }
            });
            if let Some(previous) = self
                .0
                .idle
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .replace(task)
            {
                previous.abort();
            }
        } else {
            let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.users == 0 && state.generation == generation {
                state.text.clear();
                state.large_text.clear();
                state.completed.clear();
            }
        }
    }
}
pub struct ThreadFindSubscription {
    input: OrchestrationV2SearchThreadInput,
    sequence: u64,
    cwd: Option<String>,
    rows: Vec<FindPayload>,
    documents: BTreeMap<usize, Document>,
    embedded: std::collections::BTreeSet<Option<RunId>>,
    text: Arc<dyn ThreadFindText>,
    first: bool,
    ended: bool,
    cached_result: Option<OrchestrationV2SearchThreadResult>,
    cache_key: String,
    lease: Option<CacheLease>,
}
impl ThreadFindSubscription {
    async fn count(&mut self, index: usize) -> Result<Document> {
        if let Some(document) = self.documents.get(&index) {
            return Ok(document.clone());
        }
        let row = &self.rows[index];
        let item: OrchestrationV2TurnItem = crate::stores::event_store::decode_json(
            crate::stores::event_store::parse_json(&row.payload)?,
        )?;
        let empty_hidden = matches!(&item,OrchestrationV2TurnItem::AssistantMessage(item) if item.text.is_empty())
            && self.embedded.contains(&row.run_id);
        let count = if empty_hidden {
            0
        } else {
            let value = serde_json::to_value(&item)?;
            let text = if row.item_type == "proposed_plan" {
                value["markdown"].as_str().unwrap_or("")
            } else {
                value["text"].as_str().unwrap_or("")
            };
            let large = text.encode_utf16().count() > 32768;
            let key = serde_json::to_string(&(
                text,
                &row.item_type,
                value["streaming"] == true,
                !value["context"].is_null(),
                &self.cwd,
                self.input.skills.as_deref().unwrap_or(&[]),
            ))?;
            let caches = &self.lease.as_ref().ok_or(crate::error::Error::Cancelled)?.0;
            let segments = if let Some(segments) = caches.parsed(&key, large) {
                segments
            } else {
                let segments = self.text.segments(
                    &item,
                    self.cwd.as_deref(),
                    self.input.skills.as_deref().unwrap_or(&[]),
                )?;
                caches.insert_parsed(key, large, segments.clone());
                segments
            };
            segments
                .iter()
                .map(|segment| self.text.occurrences(segment, &self.input.query.0))
                .sum()
        };
        let document = Document {
            entry_id: row.entry_id.clone(),
            run_id: row.run_id.clone(),
            count,
        };
        self.documents.insert(index, document.clone());
        tokio::task::yield_now().await;
        Ok(document)
    }
    pub async fn pull(&mut self) -> Result<Option<OrchestrationV2SearchThreadResult>> {
        if self.ended {
            return Ok(None);
        }
        if let Some(frame) = self.cached_result.take() {
            self.ended = true;
            self.lease.take();
            return Ok(Some(frame));
        }
        if self.first && self.input.index.is_none() && self.input.offset.unwrap_or(0) == 0 {
            self.first = false;
            let anchor = self
                .input
                .start
                .as_ref()
                .and_then(|start| {
                    self.rows
                        .iter()
                        .position(|row| row.entry_id == start.entry_id.0)
                })
                .unwrap_or(0);
            for step in 0..=self.rows.len() {
                if self.rows.is_empty() {
                    break;
                }
                let index = (anchor + step) % self.rows.len();
                let document = self.count(index).await?;
                let occurrence = if step == 0 {
                    self.input
                        .start
                        .as_ref()
                        .filter(|start| start.entry_id.0 == document.entry_id)
                        .map_or(0, |start| start.occurrence.min(document.count))
                } else {
                    0
                };
                if occurrence >= document.count {
                    continue;
                }
                return Ok(Some(OrchestrationV2SearchThreadResult {
                    complete: Some(false),
                    snapshot_sequence: self.sequence,
                    total_matches: 1,
                    active_index: 0,
                    r#match: Some(OrchestrationV2ThreadFindMatch {
                        entry_id: TrimmedNonEmptyString(document.entry_id),
                        run_id: document.run_id,
                        occurrence,
                    }),
                    navigation: None,
                }));
            }
        }
        for index in 0..self.rows.len() {
            self.count(index).await?;
        }
        self.ended = true;
        let documents: Vec<_> = self.documents.values().cloned().collect();
        if let Some(lease) = self.lease.take() {
            lease
                .0
                .finish(self.cache_key.clone(), self.sequence, documents.clone());
        }
        Ok(Some(result_for(&documents, &self.input, self.sequence)))
    }
}
fn result_for(
    documents: &[Document],
    input: &OrchestrationV2SearchThreadInput,
    sequence: u64,
) -> OrchestrationV2SearchThreadResult {
    let total: u64 = documents.iter().map(|doc| doc.count).sum();
    let mut requested = input.index.unwrap_or(0);
    if input.index.is_none()
        && let Some(start) = &input.start
        && let Some(index) = documents
            .iter()
            .position(|doc| doc.entry_id == start.entry_id.0)
    {
        requested = documents[..index].iter().map(|doc| doc.count).sum::<u64>()
            + start.occurrence.min(documents[index].count);
        if requested >= total {
            requested = 0;
        }
    }
    let active = if input.index.is_none() && total > 0 {
        ((i128::from(requested) + i128::from(input.offset.unwrap_or(0)))
            .rem_euclid(i128::from(total))) as u64
    } else {
        requested.min(total.saturating_sub(1))
    };
    let mut occurrence = active;
    let mut selected = None;
    let mut navigation = vec![];
    let mut start_index = 0;
    for doc in documents {
        if doc.count > 0 {
            navigation.push(SearchThreadResultNavigationItem {
                entry_id: TrimmedNonEmptyString(doc.entry_id.clone()),
                run_id: doc.run_id.clone(),
                count: doc.count,
                start_index,
            });
        }
        start_index += doc.count;
        if selected.is_none() {
            if occurrence < doc.count {
                selected = Some(OrchestrationV2ThreadFindMatch {
                    entry_id: TrimmedNonEmptyString(doc.entry_id.clone()),
                    run_id: doc.run_id.clone(),
                    occurrence,
                });
            } else {
                occurrence -= doc.count;
            }
        }
    }
    if navigation.len() > 17 {
        let selected_index = selected
            .as_ref()
            .and_then(|selected| {
                navigation
                    .iter()
                    .position(|entry| entry.entry_id == selected.entry_id)
            })
            .unwrap_or(0);
        navigation = (0..17)
            .map(|i| {
                navigation[(selected_index + i + navigation.len() - 8) % navigation.len()].clone()
            })
            .collect();
    }
    OrchestrationV2SearchThreadResult {
        complete: None,
        snapshot_sequence: sequence,
        total_matches: total,
        active_index: active,
        r#match: selected,
        navigation: Some(navigation),
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn parsed_text_and_scans_expire_after_one_idle_minute_but_not_while_borrowed() {
        let caches = Arc::new(FindCaches::default());
        let lease = caches.acquire();
        caches.insert_parsed("small".into(), false, vec!["rendered".into()]);
        caches.insert_parsed("large".into(), true, vec!["rendered-large".into()]);
        caches.finish(
            "scan".into(),
            9,
            vec![Document {
                entry_id: "message".into(),
                run_id: None,
                count: 2,
            }],
        );
        tokio::time::advance(std::time::Duration::from_secs(120)).await;
        assert!(caches.completed("scan", 9).is_some());
        drop(lease);
        tokio::time::advance(std::time::Duration::from_secs(59)).await;
        assert!(caches.parsed("small", false).is_some());
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        caches.expired.notified().await;
        assert!(caches.parsed("small", false).is_none());
        assert!(caches.parsed("large", true).is_none());
        assert!(caches.completed("scan", 9).is_none());
    }
}
