use crate::{Error, Result, context::Context};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};
use t3_contracts::pull_request_review_guide::*;

pub fn iso_time(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms.min(i64::MAX as u64) as i64)
        .unwrap_or(chrono::DateTime::UNIX_EPOCH)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
#[derive(Debug, Clone, PartialEq)]
pub struct LineRange {
    pub start: u64,
    pub end: u64,
}
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileHunks {
    pub new: Vec<LineRange>,
    pub old: Vec<LineRange>,
}
#[derive(Debug, Clone)]
pub struct ReviewGuideChange {
    pub files: Vec<String>,
    pub hunks: BTreeMap<String, FileHunks>,
}
pub fn unquote_git_path(token: &str) -> String {
    if !token.starts_with('"') || !token.ends_with('"') {
        return token.into();
    }
    let bytes = token.as_bytes();
    let mut result = Vec::new();
    let mut i = 1;
    while i + 1 < bytes.len() {
        let b = bytes[i];
        i += 1;
        if b != b'\\' {
            result.push(b);
            continue;
        }
        if i + 1 > bytes.len() {
            break;
        }
        let b = bytes[i];
        i += 1;
        match b {
            b'n' => result.push(b'\n'),
            b't' => result.push(b'\t'),
            b'r' => result.push(b'\r'),
            b'a' => result.push(7),
            b'b' => result.push(8),
            b'f' => result.push(12),
            b'v' => result.push(11),
            b'0'..=b'7' => {
                let mut value = u16::from(b - b'0');
                for _ in 0..2 {
                    if i + 1 < bytes.len() && (b'0'..=b'7').contains(&bytes[i]) {
                        value = value * 8 + u16::from(bytes[i] - b'0');
                        i += 1;
                    } else {
                        break;
                    }
                }
                result.push(value as u8);
            }
            _ => result.push(b),
        }
    }
    String::from_utf8_lossy(&result).into_owned()
}
fn header_path(line: &str, prefix: &str) -> Option<String> {
    let token = line.get(4..)?.split('\t').next()?;
    unquote_git_path(token)
        .strip_prefix(prefix)
        .map(str::to_string)
}
pub fn parse_review_guide_change(name_only: &str, patch: &str) -> ReviewGuideChange {
    static HEADER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let header = HEADER.get_or_init(|| {
        regex::Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")
            .expect("constant hunk pattern")
    });
    let mut hunks = BTreeMap::<String, FileHunks>::new();
    let mut old_path = None;
    let mut current = None;
    let mut in_hunks = false;
    for line in patch.lines() {
        if line.starts_with("diff --git ") {
            old_path = None;
            current = None;
            in_hunks = false;
        } else if !in_hunks && line.starts_with("--- ") {
            old_path = header_path(line, "a/");
        } else if !in_hunks && line.starts_with("+++ ") {
            current = header_path(line, "b/").or_else(|| old_path.clone());
            if let Some(path) = &current {
                hunks.entry(path.clone()).or_default();
            }
        } else if let Some(path) = &current
            && let Some(caps) = header.captures(line)
        {
            in_hunks = true;
            let file = hunks.get_mut(path).expect("hunk file exists");
            for (start, count, out) in [(1, 2, &mut file.old), (3, 4, &mut file.new)] {
                let start = caps[start].parse::<u64>().unwrap_or(0);
                let count = caps
                    .get(count)
                    .map_or(1, |s| s.as_str().parse::<u64>().unwrap_or(0));
                if count > 0 {
                    out.push(LineRange {
                        start,
                        end: start.saturating_add(count - 1),
                    });
                }
            }
        }
    }
    ReviewGuideChange {
        files: name_only
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect(),
        hunks,
    }
}
pub fn normalize_review_guide(
    stops: Vec<PullRequestReviewGuideStop>,
    change: &ReviewGuideChange,
) -> Vec<PullRequestReviewGuideStop> {
    let changed: HashSet<_> = change.files.iter().map(String::as_str).collect();
    let mut stops = stops
        .into_iter()
        .filter(|s| !s.title.as_str().trim().is_empty())
        .map(|mut stop| {
            let mut merged: Vec<PullRequestReviewGuideExcerpt> = Vec::new();
            for mut excerpt in stop.excerpts {
                if !changed.contains(excerpt.path.as_str()) {
                    continue;
                }
                let Some(file) = change.hunks.get(excerpt.path.as_str()) else {
                    continue;
                };
                let (start, end) = (
                    excerpt.start.min(excerpt.end),
                    excerpt.start.max(excerpt.end),
                );
                let ranges = match excerpt.side {
                    PullRequestReviewGuideExcerptSide::New => &file.new,
                    PullRequestReviewGuideExcerptSide::Old => &file.old,
                };
                let overlapping = ranges
                    .iter()
                    .filter(|h| h.start <= end && start <= h.end)
                    .collect::<Vec<_>>();
                let (Some(first), Some(last)) = (overlapping.first(), overlapping.last()) else {
                    continue;
                };
                excerpt.start = start.max(first.start);
                excerpt.end = end.min(last.end);
                let touches = |other: &PullRequestReviewGuideExcerpt| {
                    other.path == excerpt.path
                        && other.side == excerpt.side
                        && other.start <= excerpt.end.saturating_add(1)
                        && excerpt.start <= other.end.saturating_add(1)
                };
                let group = merged
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| touches(e))
                    .map(|(i, _)| i)
                    .collect::<Vec<_>>();
                if let Some(first) = group.first() {
                    let mut combined = merged[*first].clone();
                    combined.start = combined.start.min(excerpt.start);
                    combined.end = combined.end.max(excerpt.end);
                    for i in &group {
                        combined.start = combined.start.min(merged[*i].start);
                        combined.end = combined.end.max(merged[*i].end);
                    }
                    combined.caption = group
                        .iter()
                        .find_map(|i| {
                            (!merged[*i].caption.is_empty()).then(|| merged[*i].caption.clone())
                        })
                        .unwrap_or(excerpt.caption);
                    merged[*first] = combined;
                    for i in group.into_iter().skip(1).rev() {
                        merged.remove(i);
                    }
                } else {
                    merged.push(excerpt);
                }
            }
            for excerpt in &mut merged {
                excerpt.end = excerpt.end.min(excerpt.start.saturating_add(24));
            }
            stop.excerpts = merged;
            stop.files.retain(|f| changed.contains(f.path.as_str()));
            for file in &mut stop.files {
                if file.note.as_ref().is_some_and(|n| n.trim().is_empty()) {
                    file.note = None;
                }
            }
            stop.notes.retain(|n| {
                !n.title.as_str().trim().is_empty()
                    && stop
                        .excerpts
                        .iter()
                        .any(|e| e.path == n.path && e.start <= n.line && n.line <= e.end)
            });
            stop.questions.retain(|q| !q.as_str().trim().is_empty());
            if stop
                .attention
                .as_ref()
                .is_some_and(|a| a.note.as_str().trim().is_empty())
            {
                stop.attention = None;
            }
            stop
        })
        .collect::<Vec<_>>();
    let mut placed: HashSet<String> = stops
        .iter()
        .flat_map(|s| s.excerpts.iter().map(|e| e.path.to_string()))
        .collect();
    for stop in &mut stops {
        stop.files.retain(|f| placed.insert(f.path.to_string()));
    }
    let files = change
        .files
        .iter()
        .filter(|p| !placed.contains(*p))
        .map(|p| serde_json::json!({"path":p}))
        .collect::<Vec<_>>();
    if !files.is_empty()
        && let Ok(stop) = crate::convert(
            serde_json::json!({"title":"Other files","summary":"Changed files the tour does not otherwise cover.","attention":null,"excerpts":[],"files":files,"notes":[],"questions":[]}),
        )
    {
        stops.push(stop);
    }
    stops
}

pub struct PreparedGuide {
    pub change: ReviewGuideChange,
    pub detail: t3_contracts::pull_request::PullRequestDetail,
    pub activity: t3_contracts::pull_request::PullRequestActivity,
    pub base_commit: String,
    pub cwd: String,
    pub cleanup_id: String,
}
#[async_trait::async_trait]
pub trait GuideGenerator: Send + Sync {
    async fn check_enabled(&self, context: &Context) -> Result<()>;
    /// Must use an isolated checkout, validated refs, a pinned head, and remove it in cleanup.
    async fn prepare(
        &self,
        context: &Context,
        detail: t3_contracts::pull_request::PullRequestDetail,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<PreparedGuide>;
    async fn generate(
        &self,
        input: &PreparedGuide,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<(
        Vec<PullRequestReviewGuideStop>,
        t3_contracts::model_selection::ModelSelection,
    )>;
    async fn cleanup(&self, input: PreparedGuide);
}
struct Run {
    id: u64,
    head: String,
    state: PullRequestReviewGuideRun,
}
struct GuideInner {
    pull_requests: crate::PullRequestService,
    generator: Arc<dyn GuideGenerator>,
    records: Arc<dyn crate::linked::Records>,
    clock: Arc<dyn crate::cache::Clock>,
    runs: tokio::sync::Mutex<BTreeMap<String, Run>>,
    counter: std::sync::atomic::AtomicU64,
    cancel: tokio_util::sync::CancellationToken,
    tasks: tokio_util::task::TaskTracker,
}
#[derive(Clone)]
pub struct PullRequestReviewGuideService(Arc<GuideInner>);
impl PullRequestReviewGuideService {
    pub fn new(
        pull_requests: crate::PullRequestService,
        generator: Arc<dyn GuideGenerator>,
        records: Arc<dyn crate::linked::Records>,
        clock: Arc<dyn crate::cache::Clock>,
    ) -> Self {
        let tasks = tokio_util::task::TaskTracker::new();
        tasks.close();
        Self(Arc::new(GuideInner {
            pull_requests,
            generator,
            records,
            clock,
            runs: Default::default(),
            counter: std::sync::atomic::AtomicU64::new(0),
            cancel: tokio_util::sync::CancellationToken::new(),
            tasks,
        }))
    }
    pub async fn get(
        &self,
        input: t3_contracts::pull_request::PullRequestRef,
    ) -> Result<PullRequestReviewGuideResult> {
        let _activity = self.0.tasks.token();
        tokio::select! {_ = self.0.cancel.cancelled()=>Err(Error::invalid("reviewGuide","Review guide service is shut down.")),result=self.get_inner(input)=>result}
    }
    async fn get_inner(
        &self,
        input: t3_contracts::pull_request::PullRequestRef,
    ) -> Result<PullRequestReviewGuideResult> {
        if self.0.cancel.is_cancelled() {
            return Err(Error::invalid(
                "reviewGuide",
                "Review guide service is shut down.",
            ));
        }
        let ctx = self.0.pull_requests.context(&input).await?;
        let guide = self.0.records.guide(&ctx.key()).await?;
        let detail = self.0.pull_requests.detail(input).await?;
        let run = self
            .0
            .runs
            .lock()
            .await
            .get(&crate::linked::key_string(&ctx.key()))
            .map(|r| r.state.clone());
        Ok(PullRequestReviewGuideResult {
            guide,
            run,
            current_head_sha: detail.head_sha.flatten(),
        })
    }
    pub async fn generate(
        &self,
        input: t3_contracts::pull_request::PullRequestRef,
    ) -> Result<(PullRequestReviewGuideResult, bool)> {
        let _activity = self.0.tasks.token();
        tokio::select! {_ = self.0.cancel.cancelled()=>Err(Error::invalid("generateReviewGuide","Review guide service is shut down.")),result=self.generate_inner(input)=>result}
    }
    async fn generate_inner(
        &self,
        mut input: t3_contracts::pull_request::PullRequestRef,
    ) -> Result<(PullRequestReviewGuideResult, bool)> {
        if self.0.cancel.is_cancelled() {
            return Err(Error::invalid(
                "generateReviewGuide",
                "Review guide service is shut down.",
            ));
        }
        let ctx = self.0.pull_requests.context(&input).await?;
        self.0.generator.check_enabled(&ctx).await?;
        let project = self.0.pull_requests.project(&input.project_id).await?;
        let own=self.0.pull_requests.context(&crate::convert(serde_json::json!({"projectId":project.project.id,"repository":project.project.repository,"host":project.project.host,"number":input.number}))?).await?;
        if own.key() != ctx.key() {
            return Err(Error::invalid(
                "generateReviewGuide",
                "Guides are written from this project's checkout; select a checkout of this pull request's repository.",
            ));
        }
        input.allow_stale = Some(Some(false));
        self.0
            .pull_requests
            .invalidate(
                crate::convert(serde_json::json!({"reference":input}))?,
                false,
            )
            .await?;
        let current = self.get(input.clone()).await?;
        let head = current
            .current_head_sha
            .as_ref()
            .ok_or_else(|| {
                Error::invalid(
                    "generateReviewGuide",
                    "The host did not report the pull request head.",
                )
            })?
            .to_string();
        if !(7..=64).contains(&head.len()) || !head.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::invalid(
                "generateReviewGuide",
                "The host reported a head that is not a commit hash.",
            ));
        }
        let key = crate::linked::key_string(&ctx.key());
        let mut runs = self.0.runs.lock().await;
        let latest = self.0.pull_requests.detail(input.clone()).await?;
        if latest
            .head_sha
            .as_ref()
            .and_then(Option::as_ref)
            .map(|s| s.as_str())
            != Some(head.as_str())
        {
            return Err(Error::invalid(
                "generateReviewGuide",
                "The pull request changed while the guide was starting. Try again.",
            ));
        }
        if let Some(existing) = runs
            .get(&key)
            .filter(|r| r.head == head && matches!(r.state, PullRequestReviewGuideRun::Running(_)))
        {
            return Ok((
                PullRequestReviewGuideResult {
                    run: Some(existing.state.clone()),
                    ..current
                },
                false,
            ));
        }
        let id = self
            .0
            .counter
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let state: PullRequestReviewGuideRun = crate::convert(
            serde_json::json!({"_tag":"Running","headSha":head,"startedAt":iso_time(self.0.clock.now_ms())}),
        )?;
        runs.insert(
            key.clone(),
            Run {
                id,
                head: head.clone(),
                state: state.clone(),
            },
        );
        drop(runs);
        let service = self.clone();
        self.0.tasks.spawn(async move {
            let result=service.run(&ctx,latest,&key,id,&head).await;
            let mut runs=service.0.runs.lock().await;
            if runs.get(&key).is_none_or(|r|r.id!=id){return;}
            match result {
                Ok(())=>{runs.remove(&key);},
                Err(_) if service.0.cancel.is_cancelled()=>{runs.remove(&key);},
                Err(error)=>{
                    let state=crate::convert(serde_json::json!({"_tag":"Failed","headSha":head,"failedAt":iso_time(service.0.clock.now_ms()),"detail":error.detail}));
                    if let Ok(state)=state{runs.insert(key,Run{id,head,state});}
                }
            }
        });
        Ok((
            PullRequestReviewGuideResult {
                run: Some(state),
                ..current
            },
            true,
        ))
    }
    async fn run(
        &self,
        ctx: &Context,
        detail: t3_contracts::pull_request::PullRequestDetail,
        key: &str,
        id: u64,
        head: &str,
    ) -> Result<()> {
        let prepared = self
            .0
            .generator
            .prepare(ctx, detail, self.0.cancel.child_token())
            .await?;
        let result = tokio::select! {_ = self.0.cancel.cancelled()=>Err(Error::invalid("generateReviewGuide","Review guide generation canceled.")),r=self.0.generator.generate(&prepared,self.0.cancel.child_token())=>r};
        let result=async {
            let (stops,model_selection)=result?;
            let stops=normalize_review_guide(stops,&prepared.change);
            let runs=self.0.runs.lock().await;
            if runs.get(key).is_some_and(|r|r.id==id)&&!self.0.cancel.is_cancelled(){
                let guide=crate::convert(serde_json::json!({"headSha":head,"generatedAt":iso_time(self.0.clock.now_ms()),"modelSelection":model_selection,"stops":stops}))?;
                self.0.records.set_guide(&ctx.key(),guide).await?;
            }
            Ok(())
        }.await;
        self.0.generator.cleanup(prepared).await;
        result
    }
    pub async fn drain(&self) {
        self.0.tasks.wait().await;
    }
    pub async fn shutdown(&self) {
        self.0.cancel.cancel();
        self.0.tasks.close();
        self.0.tasks.wait().await;
    }
}
