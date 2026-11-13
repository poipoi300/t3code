mod support;
use serde_json::json;
use support::decode;
use t3_contracts::pull_request_review_guide::*;
use t3_pull_request::guide::*;
fn stop(extra: serde_json::Value) -> PullRequestReviewGuideStop {
    let mut value = json!({"title":"Stop","summary":"","attention":null,"excerpts":[],"files":[],"notes":[],"questions":[]});
    value
        .as_object_mut()
        .expect("stop")
        .extend(extra.as_object().expect("extra").clone());
    decode(value)
}
fn excerpt(path: &str, start: u64, end: u64) -> serde_json::Value {
    json!({"path":path,"side":"new","start":start,"end":end,"caption":""})
}

#[test]
fn parser_keeps_header_like_source_lines_inside_current_hunks_and_deletions() {
    let patch = "diff --git a/notes.md b/notes.md\n--- a/notes.md\n+++ b/notes.md\n@@ -3,1 +3,2 @@\n--- a/old-heading\n+++ b/other.ts\n+tail\n@@ -9,0 +10,1 @@\n+later\ndiff --git a/gone b/gone\n--- a/gone\n+++ /dev/null\n@@ -1,20 +0,0 @@\n-removed\n";
    let change = parse_review_guide_change("notes.md\0gone\0", patch);
    assert_eq!(change.hunks.len(), 2);
    assert_eq!(
        change.hunks["notes.md"].new,
        [
            LineRange { start: 3, end: 4 },
            LineRange { start: 10, end: 10 }
        ]
    );
    assert_eq!(change.hunks["gone"].old, [LineRange { start: 1, end: 20 }]);
    assert!(change.hunks["gone"].new.is_empty());
}

#[test]
fn parser_unquotes_git_paths_and_preserves_tabs_and_utf8_octals() {
    let patch = "diff --git a/x b/x\n--- \"a/na\\303\\257ve\\tfile\"\n+++ \"b/na\\303\\257ve\\tfile\"\n@@ -1 +1 @@\n-a\n+b\n";
    let change = parse_review_guide_change("naïve\tfile\0", patch);
    assert!(change.hunks.contains_key("naïve\tfile"));
    assert_eq!(unquote_git_path("\"a\\\\b\\\"c\""), "a\\b\"c");
}

#[test]
fn normalization_clamps_merges_bridged_ranges_caps_and_discards_outside_notes() {
    let change = parse_review_guide_change(
        "a.rs\0other.md\0unshown\0",
        "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +10,100 @@\n-a\n+b\n",
    );
    let guide = normalize_review_guide(
        vec![stop(
            json!({"excerpts":[excerpt("a.rs",1,12),excerpt("a.rs",20,60),excerpt("a.rs",12,20),excerpt("outside",1,5)],"files":[{"path":"outside"},{"path":"other.md","note":" "}],"notes":[{"path":"a.rs","line":11,"title":"inside","body":""},{"path":"a.rs","line":90,"title":"outside","body":""}]}),
        )],
        &change,
    );
    assert_eq!(guide.len(), 2);
    assert_eq!(guide[0].excerpts.len(), 1);
    assert_eq!(guide[0].excerpts[0].start, 10);
    assert_eq!(guide[0].excerpts[0].end, 34);
    assert_eq!(guide[0].notes.len(), 1);
    assert_eq!(guide[0].files.len(), 1);
    assert_eq!(guide[0].files[0].note, None);
    assert_eq!(guide[1].files[0].path.as_str(), "unshown");
}

#[test]
fn different_stops_may_show_distinct_excerpts_but_files_are_listed_once() {
    let change = parse_review_guide_change(
        "a\0b\0c\0",
        "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1,30 @@\n-a\n+b\n",
    );
    let guide = normalize_review_guide(
        vec![
            stop(json!({"excerpts":[excerpt("a",1,2)],"files":[{"path":"b"}]})),
            stop(json!({"excerpts":[excerpt("a",20,22)],"files":[{"path":"a"},{"path":"b"}]})),
        ],
        &change,
    );
    assert_eq!(guide[0].files.len(), 1);
    assert!(guide[1].files.is_empty());
    assert_eq!(guide[1].excerpts[0].start, 20);
    assert_eq!(guide[2].files[0].path.as_str(), "c");
}

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use t3_pull_request::{Error, Result, context::Context, linked::MemoryRecords};
use tokio::sync::{Notify, Semaphore};
struct Generator {
    started: Notify,
    released: Semaphore,
    calls: AtomicUsize,
    cleaned: AtomicUsize,
    enabled: AtomicBool,
}
impl Generator {
    fn new() -> Self {
        Self {
            started: Notify::new(),
            released: Semaphore::new(0),
            calls: AtomicUsize::new(0),
            cleaned: AtomicUsize::new(0),
            enabled: AtomicBool::new(true),
        }
    }
}
#[async_trait::async_trait]
impl GuideGenerator for Generator {
    async fn check_enabled(&self, _: &Context) -> Result<()> {
        if self.enabled.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(Error::invalid(
                "generateReviewGuide",
                "Review guides are off.",
            ))
        }
    }
    async fn prepare(
        &self,
        ctx: &Context,
        detail: t3_contracts::pull_request::PullRequestDetail,
        _: tokio_util::sync::CancellationToken,
    ) -> Result<PreparedGuide> {
        Ok(PreparedGuide {
            change: parse_review_guide_change(
                "a\0",
                "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-a\n+b\n",
            ),
            detail,
            activity: decode(
                json!({"comments":[],"commentCount":0,"commentsTruncated":false,"reviewThreads":[],"commits":[]}),
            ),
            base_commit: "def1234".into(),
            cwd: ctx.project.workspace_root.clone(),
            cleanup_id: "run".into(),
        })
    }
    async fn generate(
        &self,
        _: &PreparedGuide,
        _: tokio_util::sync::CancellationToken,
    ) -> Result<(
        Vec<PullRequestReviewGuideStop>,
        t3_contracts::model_selection::ModelSelection,
    )> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        self.started.notify_one();
        let permit = self.released.acquire().await.expect("release");
        permit.forget();
        Ok((
            vec![stop(json!({"excerpts":[excerpt("a",1,1)]}))],
            decode(json!({"instanceId":"codex","model":"test"})),
        ))
    }
    async fn cleanup(&self, _: PreparedGuide) {
        self.cleaned.fetch_add(1, Ordering::AcqRel);
    }
}
#[tokio::test]
async fn concurrent_generation_claims_share_one_head_and_shutdown_cleans_canceled_work() {
    let (pull_requests, _, clock, _) = support::setup();
    let generator = Arc::new(Generator::new());
    let records = Arc::new(MemoryRecords::default());
    let guides = PullRequestReviewGuideService::new(
        pull_requests.clone(),
        generator.clone(),
        records,
        clock,
    );
    assert!(
        guides
            .generate(support::reference())
            .await
            .expect("start")
            .1
    );
    generator.started.notified().await;
    assert!(
        !guides
            .generate(support::reference())
            .await
            .expect("duplicate")
            .1
    );
    assert_eq!(generator.calls.load(Ordering::Acquire), 1);
    guides.shutdown().await;
    assert_eq!(generator.cleaned.load(Ordering::Acquire), 1);
    assert!(guides.generate(support::reference()).await.is_err());
    pull_requests.shutdown().await;
}

#[tokio::test]
async fn generated_guide_persists_normalized_stops_and_disabled_guides_refuse_start() {
    let (pull_requests, _, clock, _) = support::setup();
    let generator = Arc::new(Generator::new());
    let guides = PullRequestReviewGuideService::new(
        pull_requests.clone(),
        generator.clone(),
        Arc::new(MemoryRecords::default()),
        clock,
    );
    generator.enabled.store(false, Ordering::Release);
    assert!(guides.generate(support::reference()).await.is_err());
    generator.enabled.store(true, Ordering::Release);
    assert!(
        guides
            .generate(support::reference())
            .await
            .expect("start")
            .1
    );
    generator.started.notified().await;
    generator.released.add_permits(1);
    guides.drain().await;
    let result = guides.get(support::reference()).await.expect("guide");
    assert!(result.run.is_none());
    let guide = result.guide.expect("stored");
    assert_eq!(guide.head_sha.as_str(), "abc1234");
    assert_eq!(guide.stops.len(), 1);
    assert_eq!(generator.cleaned.load(Ordering::Acquire), 1);
    guides.shutdown().await;
    pull_requests.shutdown().await;
}
