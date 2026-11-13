#![allow(dead_code)]
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};
use t3_contracts::pull_request::*;
use t3_pull_request::{
    Error, PullRequestService, Result,
    cache::{Clock, MemoryStore, PullRequestReadCache},
    context::{Binding, Context, Project, ProjectSource},
    host::*,
};

pub fn decode<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).expect("fixture decodes")
}
pub fn reference() -> PullRequestRef {
    decode(
        json!({"projectId":"p","host":"github.com","repository":"owner/repo","number":42,"allowStale":false}),
    )
}
pub fn summary(state: &str) -> PullRequestSummary {
    decode(
        json!({"provider":"github","projectId":"p","repository":"owner/repo","number":42,"title":"PR title","url":"https://github.com/owner/repo/pull/42","state":state,"headBranch":"topic","baseBranch":"main","closedAt":null,"mergedAt":if state=="merged"{Some("2026-01-01T00:00:00.000Z")}else{None},"updatedAt":"2026-01-01T00:00:00.000Z","isDraft":false}),
    )
}
pub fn capabilities() -> PullRequestCapabilities {
    decode(
        json!({"diff":true,"comment":true,"actions":["merge","ready","draft","close","reopen","update-branch","delete-branch"],"mergeMethods":["merge","squash"],"updateMethods":["merge","rebase"],"search":true,"deleteBranch":true,"reactions":true,"viewedFiles":"environment","review":{"inlineComment":true,"reply":true,"resolve":true,"verdicts":["comment","approve","request-changes"]},"reviewers":{"request":true,"listCandidates":true},"edit":{"changeRequest":true,"comment":true,"maintainerEdits":true},"labels":true}),
    )
}
pub fn permissions() -> PullRequestViewerPermissions {
    decode(
        json!({"actions":["merge","close","update-branch","delete-branch"],"updateMethods":["merge","rebase"],"comment":true,"resolve":true,"verdicts":["comment","approve","request-changes"],"requestReviewers":true,"labels":true}),
    )
}
pub fn project() -> Project {
    decode(
        json!({"id":"p","title":"Project","workspace_root":"C:/sandbox","host":"github.com","repository":"owner/repo","canonical_repository_key":"github.com/owner/repo","remove_agent_credits_on_merge":false}),
    )
}
pub fn row(number: u64, updated: &str) -> PullRequestListEntry {
    decode(
        json!({"provider":"github","host":"github.com","projectId":"p","projectTitle":"Project","repository":"owner/repo","number":number,"title":"PR title","url":format!("https://github.com/owner/repo/pull/{number}"),"author":{"login":"alice","name":null,"avatarUrl":null},"headBranch":"topic","baseBranch":"main","state":"open","isDraft":false,"mergeability":"mergeable","additions":1,"deletions":0,"createdAt":updated,"updatedAt":updated,"viewerReviewRequested":false,"labels":[]}),
    )
}
#[derive(Default)]
pub struct TestClock(pub AtomicU64);
impl Clock for TestClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }
}
pub struct Source(pub Mutex<Vec<Binding>>);
#[async_trait]
impl ProjectSource for Source {
    async fn projects(&self) -> Result<Vec<Binding>> {
        Ok(self.0.lock().expect("projects").clone())
    }
}
pub struct FakeHost {
    pub caps: PullRequestCapabilities,
    pub perms: Mutex<PullRequestViewerPermissions>,
    pub reads: AtomicUsize,
    pub writes: Mutex<Vec<Value>>,
    pub summaries: Mutex<VecDeque<Result<PullRequestSummary>>>,
    pub pages: Mutex<VecDeque<ListPage>>,
    pub queries: Mutex<Vec<ListQuery>>,
    pub revisions: Mutex<BTreeMap<String, String>>,
    pub paths: Mutex<VecDeque<DiffPaths>>,
    pub contents: AtomicUsize,
    pub diff_reads: AtomicUsize,
    pub action: Mutex<Option<PullRequestSummary>>,
    pub verified: AtomicUsize,
    pub head: Mutex<String>,
    pub options: Mutex<PullRequestFilterOptions>,
}
impl Default for FakeHost {
    fn default() -> Self {
        Self {
            caps: capabilities(),
            perms: Mutex::new(permissions()),
            reads: AtomicUsize::new(0),
            writes: Default::default(),
            summaries: Default::default(),
            pages: Default::default(),
            queries: Default::default(),
            revisions: Default::default(),
            paths: Default::default(),
            contents: AtomicUsize::new(0),
            diff_reads: AtomicUsize::new(0),
            action: Default::default(),
            verified: AtomicUsize::new(0),
            head: Mutex::new("abc1234".into()),
            options: Mutex::new(PullRequestFilterOptions {
                labels: Vec::new(),
                authors: Vec::new(),
                references: Vec::new(),
                truncated: false,
            }),
        }
    }
}
#[async_trait]
impl Host for FakeHost {
    fn kind(&self) -> &str {
        "github"
    }
    fn capabilities(&self) -> &PullRequestCapabilities {
        &self.caps
    }
    fn reads_public_host(&self, host: &str) -> bool {
        host == "github.com"
    }
    async fn viewer(&self, _: &Project) -> Result<String> {
        Ok("alice".into())
    }
    async fn filter_options(
        &self,
        _: &Context,
        _: &PullRequestFilterOptionsInput,
    ) -> Result<PullRequestFilterOptions> {
        Ok(self.options.lock().expect("options").clone())
    }
    async fn verify(&self, _: &Project) -> Result<VerifiedHost> {
        self.verified.fetch_add(1, Ordering::AcqRel);
        Ok(VerifiedHost {
            account_id: "account-a".into(),
            viewer: "alice".into(),
            credential_fingerprint: "credential-a".into(),
            host: Arc::new(FakeHost::default()),
        })
    }
    async fn summary(&self, _: &Context) -> Result<PullRequestSummary> {
        self.reads.fetch_add(1, Ordering::AcqRel);
        self.summaries
            .lock()
            .expect("summaries")
            .pop_front()
            .unwrap_or_else(|| Ok(summary("open")))
    }
    async fn detail(&self, ctx: &Context) -> Result<PullRequestDetail> {
        let mut value = json!(summary("open"));
        value.as_object_mut().expect("detail").extend(json!({"capabilities":self.caps,"viewerPermissions":permissions(),"projectTitle":ctx.project.title,"workspaceRoot":ctx.project.workspace_root,"body":"Description","author":null,"isDraft":false,"mergeability":"mergeable","additions":1,"deletions":0,"changedFiles":1,"createdAt":"2026-01-01T00:00:00.000Z","reviewers":[],"labels":[],"checks":[],"mergeCapabilities":{"merge":true,"squash":true,"rebase":false},"headSha":self.head.lock().expect("head").clone()}).as_object().expect("fields").clone());
        Ok(decode(value))
    }
    async fn permissions(&self, _: &Context, _: bool) -> Result<PullRequestViewerPermissions> {
        Ok(self.perms.lock().expect("perms").clone())
    }
    async fn comment(&self, _: &Context, input: &PullRequestCommentInput) -> Result<()> {
        self.writes.lock().expect("writes").push(json!(input));
        Ok(())
    }
    async fn update(&self, _: &Context, input: &PullRequestUpdateInput) -> Result<()> {
        self.writes.lock().expect("writes").push(json!(input));
        Ok(())
    }
    async fn update_comment(
        &self,
        _: &Context,
        input: &PullRequestCommentUpdateInput,
    ) -> Result<()> {
        self.writes.lock().expect("writes").push(json!(input));
        Ok(())
    }
    async fn submit_review(&self, _: &Context, input: &PullRequestSubmitReviewInput) -> Result<()> {
        self.writes.lock().expect("writes").push(json!(input));
        Ok(())
    }
    async fn set_labels(&self, _: &Context, input: &PullRequestLabelChangeInput) -> Result<()> {
        self.writes.lock().expect("writes").push(json!(input));
        Ok(())
    }
    async fn set_reaction(&self, _: &Context, input: &PullRequestReactionInput) -> Result<()> {
        self.writes.lock().expect("writes").push(json!(input));
        Ok(())
    }
    async fn run_action(
        &self,
        _: &Context,
        input: &PullRequestActionInput,
    ) -> Result<Option<PullRequestSummary>> {
        self.writes.lock().expect("writes").push(json!(input));
        Ok(self.action.lock().expect("action").clone())
    }
    async fn list(&self, _: &Context, query: ListQuery) -> Result<ListPage> {
        self.queries.lock().expect("queries").push(query);
        self.pages
            .lock()
            .expect("pages")
            .pop_front()
            .ok_or_else(|| Error::invalid("list", "No scripted page."))
    }
    async fn file_revisions(&self, _: &Context, _: &[String]) -> Result<FileRevisions> {
        Ok(FileRevisions {
            revisions: self.revisions.lock().expect("revisions").clone(),
            complete: true,
        })
    }
    async fn diff(&self, _: &Context, _: &PullRequestDiffInput) -> Result<PullRequestDiffResult> {
        let read = self.diff_reads.fetch_add(1, Ordering::AcqRel) + 1;
        Ok(decode(
            json!({"patch":format!("patch {read}"),"truncated":false,"nextCursor":null}),
        ))
    }
    async fn diff_paths(&self, _: &Context, _: &PullRequestDiffInput) -> Result<DiffPaths> {
        self.paths
            .lock()
            .expect("paths")
            .pop_front()
            .ok_or_else(|| Error::invalid("paths", "No scripted paths."))
    }
    async fn diff_file_contents(
        &self,
        _: &Context,
        _: &PullRequestDiffFileContentsInput,
    ) -> Result<PullRequestDiffFileContentsResult> {
        self.contents.fetch_add(1, Ordering::AcqRel);
        Ok(PullRequestDiffFileContentsResult {
            old_contents: "old".into(),
            new_contents: "new".into(),
        })
    }
}
pub fn setup() -> (
    PullRequestService,
    Arc<FakeHost>,
    Arc<TestClock>,
    Arc<Source>,
) {
    let host = Arc::new(FakeHost::default());
    let source = Arc::new(Source(Mutex::new(vec![Binding {
        project: project(),
        host: host.clone(),
    }])));
    let clock = Arc::new(TestClock::default());
    let cache = Arc::new(PullRequestReadCache::new(
        Arc::new(MemoryStore::default()),
        clock.clone(),
    ));
    (
        PullRequestService::new(source.clone(), clock.clone(), cache),
        host,
        clock,
        source,
    )
}
pub fn input(extra: Value) -> Value {
    let mut value = json!(reference());
    value
        .as_object_mut()
        .expect("ref")
        .extend(extra.as_object().expect("fields").clone());
    value
}
