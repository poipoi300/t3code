mod support;
use serde_json::json;
use std::sync::atomic::Ordering;
use support::*;
use t3_contracts::pull_request::*;
use t3_pull_request::{Error, FailureReason, service::SummaryOptions};

#[tokio::test]
async fn summary_ttl_preflight_and_errors_are_not_cached() {
    let (service, host, clock, _) = setup();
    let reference = reference();
    service
        .summary(reference.clone(), SummaryOptions::default())
        .await
        .expect("first");
    service
        .summary(reference.clone(), SummaryOptions::default())
        .await
        .expect("cached");
    assert_eq!(host.reads.load(Ordering::Acquire), 1);
    service
        .summary(
            reference.clone(),
            SummaryOptions {
                bypass_cache: true,
                recover_transient_failure: Some(false),
            },
        )
        .await
        .expect("preflight");
    assert_eq!(host.reads.load(Ordering::Acquire), 2);
    clock.0.store(60_001, Ordering::Release);
    host.summaries
        .lock()
        .expect("queue")
        .push_back(Err(Error::invalid("summary", "Host failed.")));
    assert!(
        service
            .summary(
                reference.clone(),
                SummaryOptions {
                    recover_transient_failure: Some(false),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    service
        .summary(reference, SummaryOptions::default())
        .await
        .expect("retry");
    assert_eq!(host.reads.load(Ordering::Acquire), 4);
}

#[tokio::test]
async fn cached_reads_still_require_current_authoritative_project() {
    let (service, host, _, source) = setup();
    service
        .summary(reference(), Default::default())
        .await
        .expect("read");
    source.0.lock().expect("source").clear();
    assert!(
        service
            .summary(reference(), Default::default())
            .await
            .is_err()
    );
    assert_eq!(host.reads.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn pinned_account_verifies_even_on_cache_hits_and_refuses_mismatch() {
    let (service, host, _, _) = setup();
    let pinned: PullRequestRef = decode(input(json!({"expectedAccountId":"account-a"})));
    service
        .summary(pinned.clone(), Default::default())
        .await
        .expect("pinned");
    service
        .summary(pinned, Default::default())
        .await
        .expect("cached pinned");
    assert_eq!(host.verified.load(Ordering::Acquire), 2);
    let wrong = decode(input(json!({"expectedAccountId":"other"})));
    assert!(service.summary(wrong, Default::default()).await.is_err());
    assert_eq!(host.reads.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn comment_rechecks_access_preserves_untrusted_body_and_notifies_only_on_success() {
    let (service, host, _, _) = setup();
    let mut refresh = service.subscribe_refreshes();
    let body = "$(Remove-Item *) `echo hi`\n'; DROP TABLE threads; --";
    service
        .comment(decode(input(json!({"body":body}))))
        .await
        .expect("comment");
    assert!(refresh.has_changed().expect("refresh"));
    refresh.borrow_and_update();
    assert_eq!(host.writes.lock().expect("writes")[0]["body"], body);
    host.perms.lock().expect("permissions").comment = false;
    assert!(
        service
            .comment(decode(input(json!({"body":"another"}))))
            .await
            .is_err()
    );
    assert!(!refresh.has_changed().expect("refresh"));
    assert_eq!(host.writes.lock().expect("writes").len(), 1);
    assert!(
        service
            .comment(decode(input(json!({"body":" \n\t"}))))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn author_edits_and_reactions_use_host_authorization_instead_of_write_permission() {
    let (service, host, _, _) = setup();
    {
        let mut p = host.perms.lock().expect("perms");
        p.comment = false;
        p.edit_change_request = Some(Some(false));
    }
    service
        .update(decode(input(json!({"title":"New title"}))))
        .await
        .expect("author update");
    service
        .update_comment(decode(input(
            json!({"kind":"issue-comment","commentId":"c","body":"edited"}),
        )))
        .await
        .expect("author comment edit");
    service
        .set_reaction(decode(input(json!({"content":"heart","reacted":true}))))
        .await
        .expect("reader reaction");
    assert_eq!(host.writes.lock().expect("writes").len(), 3);
}

#[tokio::test]
async fn reviews_require_content_except_approvals_and_fresh_verdict_permission() {
    let (service, host, _, _) = setup();
    assert!(
        service
            .submit_review(decode(input(
                json!({"verdict":"comment","body":"","comments":[]})
            )))
            .await
            .is_err()
    );
    service
        .submit_review(decode(input(
            json!({"verdict":"approve","body":"","comments":[]}),
        )))
        .await
        .expect("empty approval");
    host.perms.lock().expect("perms").verdicts.clear();
    assert!(
        service
            .submit_review(decode(input(
                json!({"verdict":"approve","body":"yes","comments":[]})
            )))
            .await
            .is_err()
    );
    assert_eq!(host.writes.lock().expect("writes").len(), 1);
}

#[tokio::test]
async fn merge_event_requires_terminal_confirmation_and_summary_reply_is_opt_in() {
    let (service, host, _, _) = setup();
    let mut merges = service.subscribe_merges();
    let action = decode(input(json!({"action":"merge","mergeMethod":"squash"})));
    assert!(
        service
            .run_action(action)
            .await
            .expect("enqueued")
            .is_none()
    );
    assert!(merges.try_recv().is_err());
    *host.action.lock().expect("action") = Some(summary("merged"));
    let action = decode(input(json!({"action":"merge","replyWithSummary":true})));
    assert_eq!(
        service
            .run_action(action)
            .await
            .expect("merged")
            .expect("summary")
            .state,
        PullRequestState::Merged
    );
    assert_eq!(merges.recv().await.expect("merge").reference.number, 42);
}

#[tokio::test]
async fn unsupported_action_strategies_and_stack_missing_heads_are_refused() {
    let (service, host, _, _) = setup();
    for extra in [
        json!({"action":"merge","mergeMethod":"rebase"}),
        json!({"action":"merge","stackNumber":1}),
        json!({"action":"update-branch","stackNumber":1,"updateMethod":"merge"}),
    ] {
        assert!(service.run_action(decode(input(extra))).await.is_err());
    }
    assert!(host.writes.lock().expect("writes").is_empty());
}

#[tokio::test]
async fn file_expansion_walks_pages_and_denies_non_changes_or_cursor_cycles() {
    let (service, host, _, _) = setup();
    use t3_pull_request::host::DiffPaths;
    host.paths.lock().expect("paths").extend([
        DiffPaths {
            files: vec![("other".into(), "other".into())],
            next_cursor: Some("page2".into()),
        },
        DiffPaths {
            files: vec![("old.rs".into(), "new.rs".into())],
            next_cursor: None,
        },
    ]);
    let request = decode(input(
        json!({"changeType":"rename-changed","oldPath":"old.rs","newPath":"new.rs"}),
    ));
    assert_eq!(
        service
            .diff_file_contents(request)
            .await
            .expect("changed")
            .new_contents,
        "new"
    );
    host.paths.lock().expect("paths").push_back(DiffPaths {
        files: vec![],
        next_cursor: None,
    });
    let request = decode(input(
        json!({"changeType":"change","oldPath":"secrets.txt","newPath":"secrets.txt"}),
    ));
    assert_eq!(
        service
            .diff_file_contents(request)
            .await
            .expect_err("not changed")
            .reason,
        FailureReason::NotFound
    );
    host.paths.lock().expect("paths").extend([
        DiffPaths {
            files: vec![],
            next_cursor: Some("cycle".into()),
        },
        DiffPaths {
            files: vec![],
            next_cursor: Some("cycle".into()),
        },
    ]);
    let request = decode(input(
        json!({"changeType":"change","oldPath":"x","newPath":"x"}),
    ));
    assert!(service.diff_file_contents(request).await.is_err());
    assert_eq!(host.contents.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn filtered_list_advances_through_duplicate_boundary_rows() {
    let (service, host, _, _) = setup();
    use t3_pull_request::host::ListPage;
    let time = "2026-01-01T00:00:00.000Z";
    let mut first = row(1, time);
    first.is_draft = true;
    let second = row(2, time);
    host.pages.lock().expect("pages").extend([
        ListPage {
            entries: vec![first.clone()],
            truncated: true,
            continues: true,
            cursor_advance: None,
            resume_after: None,
        },
        ListPage {
            entries: vec![first, second],
            truncated: false,
            continues: false,
            cursor_advance: None,
            resume_after: None,
        },
    ]);
    let list = decode(json!({"state":"open","limit":1,"filters":{"draft":"hide"}}));
    let result = service.list(list).await.expect("list");
    assert_eq!(
        result.entries.iter().map(|e| e.number).collect::<Vec<_>>(),
        [2]
    );
    let queries = host.queries.lock().expect("queries");
    assert_eq!(queries.len(), 2);
    assert_eq!(queries[1].cursor.as_ref().expect("cursor").seen_at, [1]);
    drop(queries);
    let mut row_ref = reference();
    row_ref.number = 2;
    assert_eq!(
        service
            .preview(row_ref)
            .await
            .expect("list-seeded preview")
            .number,
        2
    );
}

#[tokio::test]
async fn environment_viewed_marks_follow_file_revisions_and_shown_revision_wins() {
    let (service, host, clock, _) = setup();
    host.revisions
        .lock()
        .expect("revisions")
        .insert("a.rs".into(), "head".into());
    service
        .set_files_viewed(decode(input(
            json!({"files":[{"path":"a.rs","viewed":true,"revision":"shown"}]}),
        )))
        .await
        .expect("mark shown");
    assert_eq!(
        service
            .files_viewed(reference())
            .await
            .expect("viewed")
            .files[0]
            .state,
        PullRequestFileViewedState::Dismissed
    );
    service
        .set_files_viewed(decode(input(
            json!({"files":[{"path":"a.rs","viewed":true}]}),
        )))
        .await
        .expect("mark head");
    assert_eq!(
        service
            .files_viewed(reference())
            .await
            .expect("viewed")
            .files[0]
            .state,
        PullRequestFileViewedState::Viewed
    );
    clock.0.store(60_001, Ordering::Release);
    host.revisions
        .lock()
        .expect("revisions")
        .insert("a.rs".into(), "changed".into());
    assert_eq!(
        service
            .files_viewed(reference())
            .await
            .expect("held view")
            .files[0]
            .state,
        PullRequestFileViewedState::Viewed
    );
    service.drain().await;
    assert_eq!(
        service
            .files_viewed(reference())
            .await
            .expect("cached view")
            .files[0]
            .state,
        PullRequestFileViewedState::Viewed
    );
    clock.0.store(75_002, Ordering::Release);
    assert_eq!(
        service
            .files_viewed(reference())
            .await
            .expect("refreshed view")
            .files[0]
            .state,
        PullRequestFileViewedState::Dismissed
    );
    service
        .set_files_viewed(decode(input(
            json!({"files":[{"path":"a.rs","viewed":false}]}),
        )))
        .await
        .expect("unmark");
    assert!(
        service
            .files_viewed(reference())
            .await
            .expect("viewed")
            .files
            .is_empty()
    );
}

#[tokio::test]
async fn host_pause_preserves_retry_at_and_interactive_mutations_can_proceed() {
    let (service, host, _, _) = setup();
    host.summaries.lock().expect("queue").push_back(Err(Error {
        reason: FailureReason::RateLimited,
        retry_at: Some(100_000),
        ..Error::invalid("summary", "Paused.")
    }));
    assert_eq!(
        service
            .summary(reference(), Default::default())
            .await
            .expect_err("limited")
            .retry_at,
        Some(100_000)
    );
    assert_eq!(
        service
            .summary(reference(), Default::default())
            .await
            .expect_err("paused")
            .reason,
        FailureReason::RateLimited
    );
    assert_eq!(host.reads.load(Ordering::Acquire), 1);
    service
        .comment(decode(input(json!({"body":"interactive"}))))
        .await
        .expect("interactive");
}

#[tokio::test]
async fn shutdown_refuses_late_reads() {
    let (service, _, _, _) = setup();
    service.shutdown().await;
    assert!(
        service
            .summary(reference(), Default::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn drain_completes_when_no_work_is_active() {
    let (service, _, _, _) = setup();
    let drain = service.drain();
    tokio::pin!(drain);
    assert!(futures_util::poll!(&mut drain).is_ready());
}

#[tokio::test]
async fn reported_state_only_notifies_sync_consumers_and_deduplicates_for_five_minutes() {
    let (service, host, clock, _) = setup();
    let mut changes = service.subscribe_state_changes();
    service
        .summary(reference(), Default::default())
        .await
        .expect("seed");
    let state = || PullRequestReportStateInput {
        reference: reference(),
        state: PullRequestState::Open,
        is_draft: Some(Some(false)),
    };
    service.report_state(state()).await.expect("report");
    changes.recv().await.expect("change");
    service.report_state(state()).await.expect("duplicate");
    assert!(changes.try_recv().is_err());
    service
        .summary(reference(), Default::default())
        .await
        .expect("still cached");
    assert_eq!(host.reads.load(Ordering::Acquire), 1);
    clock.0.store(300_000, Ordering::Release);
    service.report_state(state()).await.expect("later");
    assert_eq!(changes.recv().await.expect("later event").number, 42);
}

#[tokio::test]
async fn concurrent_errors_share_one_host_read_without_caching_the_failure() {
    use std::sync::{Arc, Mutex, atomic::AtomicUsize};
    use t3_pull_request::{
        PullRequestService,
        cache::{MemoryStore, PullRequestReadCache},
        context::{Binding, Context},
        host::Host,
    };
    struct Flaky {
        caps: PullRequestCapabilities,
        started: tokio::sync::Notify,
        release: tokio::sync::Semaphore,
        calls: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl Host for Flaky {
        fn kind(&self) -> &str {
            "github"
        }
        fn capabilities(&self) -> &PullRequestCapabilities {
            &self.caps
        }
        async fn summary(&self, _: &Context) -> t3_pull_request::Result<PullRequestSummary> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            self.started.notify_one();
            self.release.acquire().await.expect("release").forget();
            Err(Error::invalid("summary", "Transient failure."))
        }
    }
    let host = Arc::new(Flaky {
        caps: capabilities(),
        started: tokio::sync::Notify::new(),
        release: tokio::sync::Semaphore::new(0),
        calls: AtomicUsize::new(0),
    });
    let source = Arc::new(Source(Mutex::new(vec![Binding {
        project: project(),
        host: host.clone(),
    }])));
    let clock = Arc::new(TestClock::default());
    let service = PullRequestService::new(
        source,
        clock.clone(),
        Arc::new(PullRequestReadCache::new(
            Arc::new(MemoryStore::default()),
            clock,
        )),
    );
    let first = {
        let service = service.clone();
        tokio::spawn(async move { service.summary(reference(), Default::default()).await })
    };
    host.started.notified().await;
    let second = service.summary(reference(), Default::default());
    tokio::pin!(second);
    assert!(futures_util::poll!(&mut second).is_pending());
    host.release.add_permits(1);
    assert!(first.await.expect("first").is_err());
    assert!(futures_util::poll!(&mut second).is_ready());
    assert_eq!(host.calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn github_detail_seeds_checks_stats_summary_and_preview_without_more_host_reads() {
    let (service, host, _, _) = setup();
    service.detail(reference()).await.expect("detail");
    assert_eq!(
        service
            .summary(reference(), Default::default())
            .await
            .expect("summary")
            .number,
        42
    );
    assert_eq!(host.reads.load(Ordering::Acquire), 0);
    assert!(service.checks(reference()).await.expect("checks").is_some());
    assert_eq!(
        service.preview(reference()).await.expect("preview").number,
        42
    );
    assert_eq!(
        service
            .list_stats(PullRequestListStatsInput {
                refs: vec![reference()]
            })
            .await
            .expect("stats")
            .stats[0]
            .additions,
        1
    );
}

#[tokio::test]
async fn filter_options_match_the_source_hundred_item_limit() {
    let (service, host, _, _) = setup();
    host.options.lock().expect("options").labels = (0..101)
        .map(|i| decode(json!({"name":format!("label{i:03}"),"color":null})))
        .collect();
    let options = service
        .filter_options(decode(json!({"kind":"labels"})))
        .await
        .expect("options");
    assert_eq!(options.labels.len(), 100);
    assert!(options.truncated);
    host.options.lock().expect("options").labels = ["z", "éclair", "A", "a", "ada"]
        .map(|name| decode(json!({"name":name,"color":null})))
        .into();
    let options = service
        .filter_options(decode(json!({"kind":"labels"})))
        .await
        .expect("base collation");
    assert_eq!(
        options
            .labels
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>(),
        ["A", "ada", "éclair", "z"]
    );
}

#[tokio::test]
async fn newly_observed_summary_revision_invalidates_held_unpinned_diff() {
    let (service, host, _, _) = setup();
    let diff = || decode(input(json!({})));
    service
        .summary(reference(), Default::default())
        .await
        .expect("summary");
    assert_eq!(
        service.diff(diff()).await.expect("first diff").patch,
        "patch 1"
    );
    let mut newer = summary("open");
    newer.updated_at = t3_contracts::wire::WireDateTime("2026-01-02T00:00:00.000Z".into());
    host.summaries.lock().expect("queue").push_back(Ok(newer));
    service
        .summary(
            reference(),
            SummaryOptions {
                bypass_cache: true,
                recover_transient_failure: Some(false),
            },
        )
        .await
        .expect("new summary");
    assert_eq!(
        service.diff(diff()).await.expect("updated diff").patch,
        "patch 2"
    );
    service.shutdown().await;
}

#[tokio::test]
async fn commit_diff_expires_at_ten_minutes_and_maintenance_does_not_block_drain() {
    let (service, host, clock, _) = setup();
    let diff = || decode(input(json!({"commit":"abc1234"})));
    assert_eq!(service.diff(diff()).await.expect("first").patch, "patch 1");
    clock.0.store(599_999, Ordering::Release);
    assert_eq!(service.sweep_diff_cache(), 0);
    assert_eq!(service.diff(diff()).await.expect("held").patch, "patch 1");
    clock.0.store(600_001, Ordering::Release);
    assert_eq!(service.sweep_diff_cache(), 1);
    assert_eq!(
        service.diff(diff()).await.expect("expired").patch,
        "patch 2"
    );
    assert_eq!(host.diff_reads.load(Ordering::Acquire), 2);
    service.drain().await;
    service.shutdown().await;
}
