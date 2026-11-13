use super::*;
use crate::linked::{ViewedMark, ViewedScope};
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct RevisionState {
    asked: Vec<String>,
    revisions: BTreeMap<String, String>,
}
struct RevisionRefresh {
    service: PullRequestService,
    key: String,
}
impl Drop for RevisionRefresh {
    fn drop(&mut self) {
        self.service
            .0
            .memory
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .revision_refreshing
            .remove(&self.key);
    }
}

impl PullRequestService {
    async fn refresh_file_revisions(
        &self,
        ctx: &Context,
        paths: &[String],
        key: String,
    ) -> Result<FileRevisions> {
        let epoch = self.0.epoch.load(Ordering::Acquire);
        let now = self.0.clock.now_ms();
        let answer = self
            .protect(ctx, false, ctx.host.file_revisions(ctx, paths))
            .await?;
        let previous = self
            .held(&key)
            .filter(|h| now <= h.at.saturating_add(600_000));
        let mut state = previous
            .as_ref()
            .and_then(|h| crate::convert::<RevisionState>(&h.value).ok())
            .unwrap_or_default();
        let mut learned = paths.to_vec();
        if answer.complete {
            learned.extend(answer.revisions.keys().cloned());
        }
        for path in learned {
            state.asked.retain(|p| *p != path);
            state.asked.push(path.clone());
            if let Some(revision) = answer.revisions.get(&path) {
                state.revisions.insert(path, revision.clone());
            }
        }
        if state.asked.len() > 1_000 {
            let discarded = state
                .asked
                .drain(..state.asked.len() - 1_000)
                .collect::<Vec<_>>();
            for path in discarded {
                state.revisions.remove(&path);
            }
        }
        let at = if state
            .revisions
            .keys()
            .all(|p| answer.revisions.contains_key(p))
        {
            now
        } else {
            previous.as_ref().map_or(now, |h| h.at)
        };
        let revisions = state.revisions.clone();
        self.store(key, "revisions", json!(state), at, 60_000, epoch);
        Ok(FileRevisions {
            revisions,
            complete: answer.complete,
        })
    }
    async fn file_revisions_of(
        &self,
        ctx: &Context,
        paths: &[String],
        fresh: bool,
    ) -> Result<FileRevisions> {
        let key = self.cache_key(ctx, "revisions", "");
        let now = self.0.clock.now_ms();
        if let Some(held) = self
            .held(&key)
            .filter(|h| now <= h.at.saturating_add(600_000))
        {
            let state: RevisionState = crate::convert(held.value)?;
            if paths.iter().all(|p| state.asked.contains(p)) {
                if now <= held.at.saturating_add(60_000) {
                    return Ok(FileRevisions {
                        revisions: state.revisions,
                        complete: false,
                    });
                }
                if !fresh {
                    let start = self
                        .0
                        .memory
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .revision_refreshing
                        .insert(key.clone());
                    if start {
                        let service = self.clone();
                        let ctx = ctx.clone();
                        let paths = paths.to_vec();
                        let cancel = self.0.cancel.clone();
                        self.0.tasks.spawn(async move{let _guard=RevisionRefresh{service:service.clone(),key:key.clone()};tokio::select!{_ = cancel.cancelled()=>{},_ = service.refresh_file_revisions(&ctx,&paths,key)=>{}}});
                    }
                    return Ok(FileRevisions {
                        revisions: state.revisions,
                        complete: false,
                    });
                }
            }
        }
        self.refresh_file_revisions(ctx, paths, key).await
    }
    pub(crate) fn records(&self) -> &Arc<dyn crate::linked::Records> {
        &self.0.records
    }
    async fn viewed_scope(&self, ctx: &Context) -> Result<ViewedScope> {
        let viewer = self.viewer_cached(ctx).await?;
        Ok(ViewedScope {
            provider: ctx.host.kind().into(),
            host: ctx.project.host.to_lowercase(),
            repository: ctx
                .project
                .canonical_repository_key
                .clone()
                .unwrap_or_else(|| format!("{}/{}", ctx.project.host, ctx.project.repository))
                .to_lowercase(),
            number: ctx.reference.number,
            viewer,
        })
    }
    pub async fn files_viewed(
        &self,
        input: PullRequestRef,
    ) -> Result<PullRequestFilesViewedResult> {
        let ctx = self.context(&input).await?;
        match ctx
            .host
            .capabilities()
            .viewed_files
            .as_ref()
            .and_then(Option::as_ref)
        {
            Some(PullRequestViewedFilesStore::Host) => {
                let next = ctx.clone();
                self.read(
                    ctx,
                    ReadPolicy {
                        kind: "viewed",
                        ttl: 15_000,
                        bypass: false,
                        allow_stale: false,
                        persist: false,
                    },
                    String::new(),
                    async move { next.host.files_viewed(&next).await },
                )
                .await
            }
            Some(PullRequestViewedFilesStore::Environment) => {
                let service = self.clone();
                let next = ctx.clone();
                self.read(
                    ctx,
                    ReadPolicy {
                        kind: "viewed",
                        ttl: 15_000,
                        bypass: false,
                        allow_stale: false,
                        persist: false,
                    },
                    String::new(),
                    async move { service.environment_files_viewed(&next).await },
                )
                .await
            }
            _ => Err(Error::unsupported("filesViewed")),
        }
    }
    async fn environment_files_viewed(
        &self,
        ctx: &Context,
    ) -> Result<PullRequestFilesViewedResult> {
        let scope = self.viewed_scope(ctx).await?;
        let (marks, truncated) = self.0.records.viewed(&scope).await?;
        let paths = marks.iter().map(|m| m.path.clone()).collect::<Vec<_>>();
        let revisions = if paths.is_empty() {
            None
        } else {
            self.file_revisions_of(ctx, &paths, false).await.ok()
        };
        let files = marks
            .into_iter()
            .map(|mark| {
                let changed = mark.revision.as_ref().is_some_and(|baseline| {
                    revisions
                        .as_ref()
                        .and_then(|r| r.revisions.get(&mark.path))
                        .is_some_and(|current| current != baseline)
                });
                PullRequestFileViewed {
                    path: mark.path,
                    state: if changed {
                        PullRequestFileViewedState::Dismissed
                    } else {
                        PullRequestFileViewedState::Viewed
                    },
                }
            })
            .collect();
        Ok(PullRequestFilesViewedResult { files, truncated })
    }
    pub async fn set_files_viewed(&self, input: PullRequestSetFilesViewedInput) -> Result<()> {
        let ctx = self.context(&crate::convert(&input)?).await?;
        let key = format!("viewed-write:{}", ctx.scope());
        let gate = {
            let mut mem = self.0.memory.lock().unwrap_or_else(|e| e.into_inner());
            mem.flights.retain(|_, v| v.strong_count() > 0);
            mem.flights
                .get(&key)
                .and_then(std::sync::Weak::upgrade)
                .unwrap_or_else(|| {
                    let gate = Arc::new(AsyncMutex::new(()));
                    mem.flights.insert(key, Arc::downgrade(&gate));
                    gate
                })
        };
        let _gate = gate.lock().await;
        match ctx
            .host
            .capabilities()
            .viewed_files
            .as_ref()
            .and_then(Option::as_ref)
        {
            Some(PullRequestViewedFilesStore::Host) => {
                self.protect(&ctx, true, ctx.host.set_files_viewed(&ctx, &input))
                    .await?;
            }
            Some(PullRequestViewedFilesStore::Environment) => {
                let scope = self.viewed_scope(&ctx).await?;
                let paths = input
                    .files
                    .iter()
                    .filter(|f| f.viewed && f.revision.is_none())
                    .map(|f| f.path.clone())
                    .collect::<Vec<_>>();
                let revisions = if paths.is_empty() {
                    None
                } else {
                    self.file_revisions_of(&ctx, &paths, true).await.ok()
                };
                let marks = input
                    .files
                    .into_iter()
                    .map(|f| {
                        let revision = f.revision.flatten().map(|r| r.to_string()).or_else(|| {
                            revisions
                                .as_ref()
                                .and_then(|r| r.revisions.get(&f.path))
                                .cloned()
                        });
                        (
                            ViewedMark {
                                path: f.path,
                                revision,
                            },
                            f.viewed,
                        )
                    })
                    .collect();
                self.0
                    .records
                    .set_viewed(&scope, marks, crate::guide::iso_time(self.0.clock.now_ms()))
                    .await?;
            }
            _ => return Err(Error::unsupported("setFilesViewed")),
        }
        self.bump_scope(&format!("viewed:{}", ctx.scope()));
        Ok(())
    }
    pub async fn change_breakdown(
        &self,
        input: PullRequestRef,
    ) -> Result<PullRequestChangeBreakdown> {
        let ctx = self.context(&input).await?;
        let stats = match self.protect(&ctx, false, ctx.host.file_stats(&ctx)).await {
            Ok(s) => s,
            Err(e) if e.reason == FailureReason::Unsupported => return Ok(None),
            Err(e) => return Err(e),
        };
        if stats.truncated {
            return Ok(None);
        }
        let mut counts = BTreeMap::<&str, (u64, u64)>::from_iter(
            ["code", "tests", "mocks", "generated", "docs"].map(|kind| (kind, (0, 0))),
        );
        for (path, added, deleted) in stats.files {
            let count = counts
                .get_mut(classify_file(&path))
                .expect("known file category");
            count.0 = count.0.saturating_add(added);
            count.1 = count.1.saturating_add(deleted);
        }
        crate::convert(
            counts
                .into_iter()
                .map(|(kind, (added, deleted))| {
                    (kind, json!({"additions":added,"deletions":deleted}))
                })
                .collect::<BTreeMap<_, _>>(),
        )
        .map(Some)
    }
}
pub fn classify_file(path: &str) -> &'static str {
    let path = path.replace('\\', "/");
    let mut segments = path.split('/').collect::<Vec<_>>();
    let name = segments.pop().unwrap_or("");
    let lower = name.to_lowercase();
    let dirs = segments
        .iter()
        .map(|s| s.to_lowercase())
        .collect::<Vec<_>>();
    let in_dir = |options: &[&str]| dirs.iter().any(|s| options.contains(&s.as_str()));
    static PATTERNS: std::sync::OnceLock<regex::RegexSet> = std::sync::OnceLock::new();
    let kinds=PATTERNS.get_or_init(||regex::RegexSet::new([
        r"(\.generated\.|\.gen\.|\.g\.dart$|\.pb\.go$|_pb2(_grpc)?\.py$|\.pb\.(cc|h)$|\.min\.(js|css)$|\.(js|css)\.map$)",
        r"(\.snap$|\.(mocks?|fixtures?|stubs?|fakes?)\.[^.]+$|^(mocks?|fixtures?|stubs?|fakes?)([-_.A-Z]|$))",
        r"(?i)(\.(test|spec|e2e|eval)\.[^.]+$|_(test|spec)\.[^.]+$|^test_.+\.py$|^conftest\.py$|^(vitest|jest)\.setup\.[^.]+$)",
        r"(Tests?|Spec)\.(java|kt|kts|scala|groovy|cs|php|swift|m)$",
        r"(?i)\.(md|mdx|markdown|rst|adoc|txt)$",
        r"(?i)^(requirements.*|constraints.*|CMakeLists|robots|llms)\.txt$",
    ]).expect("constant changed-file patterns")).matches(name);
    if [
        "bun.lock",
        "bun.lockb",
        "cargo.lock",
        "composer.lock",
        "deno.lock",
        "flake.lock",
        "gemfile.lock",
        "go.sum",
        "gradle.lockfile",
        "mix.lock",
        "npm-shrinkwrap.json",
        "package-lock.json",
        "packages.lock.json",
        "pipfile.lock",
        "pnpm-lock.yaml",
        "podfile.lock",
        "poetry.lock",
        "pubspec.lock",
        "uv.lock",
        "yarn.lock",
    ]
    .contains(&lower.as_str())
        || in_dir(&["__generated__", "_generated", "generated"])
        || kinds.matched(0)
    {
        return "generated";
    }
    if in_dir(&[
        "__fixtures__",
        "__mocks__",
        "__snapshots__",
        "fakes",
        "fixture",
        "fixtures",
        "mock",
        "mocks",
        "stubs",
        "testdata",
    ]) || kinds.matched(1)
    {
        return "mocks";
    }
    if in_dir(&[
        "__tests__",
        "__test__",
        "cypress",
        "e2e",
        "evals",
        "spec",
        "specs",
        "test",
        "tests",
    ]) || kinds.matched(2)
        || kinds.matched(3)
    {
        return "tests";
    }
    if kinds.matched(4) && !kinds.matched(5) {
        return "docs";
    }
    "code"
}

#[cfg(test)]
mod tests {
    #[test]
    fn changed_file_kinds_match_source_cases_and_precedence() {
        for (path, kind) in [
            ("src/contest.py", "code"),
            ("test/fixtures/pr.json", "mocks"),
            ("pnpm-lock.yaml", "generated"),
            ("convex/_generated/api.d.ts", "generated"),
            ("src/agent.eval.ts", "tests"),
            ("evals/title-generation.ts", "tests"),
            ("README.md", "docs"),
            ("tests/README.md", "tests"),
            ("src/FooTest.java", "tests"),
            ("src/Latest.java", "code"),
            ("requirements.txt", "code"),
            ("src/model_pb2.py", "generated"),
        ] {
            assert_eq!(super::classify_file(path), kind, "{path}");
        }
    }
}
