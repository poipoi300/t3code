use crate::{Error, Result};
use async_trait::async_trait;
use std::{collections::HashMap, sync::Mutex};
use t3_contracts::{
    pull_request::PullRequestLinkedThreadsResult,
    pull_request_review_guide::PullRequestReviewGuide, thread_pull_request::ThreadPullRequestKey,
};

#[derive(Debug, Clone)]
pub struct ViewedScope {
    pub provider: String,
    pub host: String,
    pub repository: String,
    pub number: u64,
    pub viewer: String,
}
impl ViewedScope {
    pub fn key(&self) -> String {
        serde_json::json!([
            self.provider,
            self.host,
            self.repository,
            self.number,
            self.viewer
        ])
        .to_string()
    }
}
#[derive(Debug, Clone)]
pub struct ViewedMark {
    pub path: String,
    pub revision: Option<String>,
}

#[async_trait]
pub trait Records: Send + Sync {
    async fn linked_threads(
        &self,
        _key: &ThreadPullRequestKey,
    ) -> Result<PullRequestLinkedThreadsResult> {
        Err(Error::unsupported("linkedThreads"))
    }
    async fn viewed(&self, scope: &ViewedScope) -> Result<(Vec<ViewedMark>, bool)>;
    async fn set_viewed(
        &self,
        scope: &ViewedScope,
        marks: Vec<(ViewedMark, bool)>,
        viewed_at: String,
    ) -> Result<()>;
    async fn guide(&self, key: &ThreadPullRequestKey) -> Result<Option<PullRequestReviewGuide>>;
    async fn set_guide(
        &self,
        key: &ThreadPullRequestKey,
        guide: PullRequestReviewGuide,
    ) -> Result<()>;
}
#[derive(Default)]
pub struct MemoryRecords {
    viewed: Mutex<HashMap<String, Vec<ViewedMark>>>,
    guides: Mutex<HashMap<String, PullRequestReviewGuide>>,
}
pub fn key_string(key: &ThreadPullRequestKey) -> String {
    serde_json::json!([
        key.host.as_str().to_lowercase(),
        key.repository.as_str().to_lowercase(),
        key.number
    ])
    .to_string()
}
#[async_trait]
impl Records for MemoryRecords {
    async fn viewed(&self, scope: &ViewedScope) -> Result<(Vec<ViewedMark>, bool)> {
        Ok((
            self.viewed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&scope.key())
                .cloned()
                .unwrap_or_default(),
            false,
        ))
    }
    async fn set_viewed(
        &self,
        scope: &ViewedScope,
        marks: Vec<(ViewedMark, bool)>,
        _viewed_at: String,
    ) -> Result<()> {
        let mut held = self.viewed.lock().unwrap_or_else(|e| e.into_inner());
        let records = held.entry(scope.key()).or_default();
        for (mark, viewed) in marks {
            records.retain(|r| r.path != mark.path);
            if viewed {
                records.push(mark);
            }
        }
        Ok(())
    }
    async fn guide(&self, key: &ThreadPullRequestKey) -> Result<Option<PullRequestReviewGuide>> {
        Ok(self
            .guides
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key_string(key))
            .cloned())
    }
    async fn set_guide(
        &self,
        key: &ThreadPullRequestKey,
        guide: PullRequestReviewGuide,
    ) -> Result<()> {
        self.guides
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key_string(key), guide);
        Ok(())
    }
}
impl crate::PullRequestService {
    pub async fn linked_threads(
        &self,
        input: t3_contracts::pull_request::PullRequestRef,
    ) -> Result<PullRequestLinkedThreadsResult> {
        let context = self.context(&input).await?;
        self.records().linked_threads(&context.key()).await
    }
}
