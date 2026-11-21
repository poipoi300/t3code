#![allow(clippy::unwrap_used)]

use futures::executor::block_on;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use t3_client::{
    testing::FakePlatform,
    types::EnvironmentId,
    workspace::{
        pr_list::*,
        pr_views::{
            detail::*, file_order::*, groups::*, guide::*, handoff::*, markdown::*, preferences::*,
            *,
        },
        pull_requests::PullRequestKey,
        review::*,
    },
};
use t3_contracts::{composer_context::PullRequestContextMetadata, pull_request::*};
fn decode<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}
fn env() -> EnvironmentId {
    EnvironmentId("origin".into())
}
fn make_reference() -> PullRequestRef {
    decode(json!({"projectId":"project","repository":"acme/web","number":7}))
}
fn row(number: u64) -> EnvironmentPullRequestEntry {
    EnvironmentPullRequestEntry {
        environment: env(),
        entry: decode(
            json!({"provider":"github","host":"github.com","projectId":"project","projectTitle":"Project","repository":"acme/web","number":number,
        "title":"Title", "url":format!("https://github.com/acme/web/pull/{number}"),"author":{"login":"maria","name":null,"avatarUrl":null},"headBranch":"feature","baseBranch":"main","state":"open","isDraft":false,
        "mergeability":"mergeable","additions":0,"deletions":0,"createdAt":"2026-07-01T00:00:00Z","updatedAt":"2026-07-02T00:00:00Z","viewerReviewRequested":false,"labels":[]}),
        ),
    }
}
fn make_metadata() -> PullRequestContextMetadata {
    decode(
        json!({"number":7,"title":"A title","url":"https://github.com/acme/web/pull/7","headBranch":"feature","baseBranch":"main","state":"open","isDraft":false}),
    )
}
fn make_detail() -> PullRequestDetail {
    let mut value = json!(row(7).entry);
    value["capabilities"] = json!({"actions":["merge","ready","delete-branch"],"mergeMethods":["merge","squash","rebase"],"diff":true,"comment":true,"search":true,"deleteBranch":true,
        "review":{"inlineComment":true,"reply":true,"resolve":true,"verdicts":[]},"reviewers":{"request":true,"listCandidates":true}});
    value["viewerPermissions"] = json!({"actions":["merge","ready","delete-branch"],"comment":true,"resolve":true,"verdicts":[],"requestReviewers":true});
    value["workspaceRoot"] = json!("/source");
    value["body"] = json!("");
    value["changedFiles"] = json!(0);
    value["mergedAt"] = Value::Null;
    value["closedAt"] = Value::Null;
    value["reviewers"] = json!([]);
    value["checks"] = json!([]);
    value["mergeCapabilities"] = json!({"merge":true,"squash":true,"rebase":false});
    decode(value)
}
fn comment(id: &str, body: &str) -> PullRequestComment {
    decode(
        json!({"id":id,"kind":"review-comment","author":{"login":"reviewer","name":null,"avatarUrl":null},"body":body,"createdAt":"2026-07-03T00:00:00Z","url":format!("https://github.com/acme/web/pull/7#discussion_{id}"),"path":null,"reviewState":null}),
    )
}
fn thread(id: &str, body: &str) -> PullRequestReviewThread {
    decode(
        json!({"id":id,"path":"src/page.tsx","line":12,"side":"right","isResolved":false,"isOutdated":false,
    "comments":[{"id":format!("{id}-comment"),"author":{"login":"reviewer","name":null,"avatarUrl":null},"body":body,"createdAt":"2026-07-03T00:00:00Z","url":null}]}),
    )
}
fn check(status: &str) -> PullRequestCheck {
    decode(json!({"name":"typecheck","status":status,"description":"2 errors","url":null}))
}

#[test]
fn file_order_ports_every_import_resolution_case() {
    type ImportCase<'a> = (Vec<(&'a str, Vec<&'a str>)>, Vec<&'a str>);
    let cases: Vec<ImportCase<'_>> = vec![
        (
            vec![
                ("src/ui/a.ts", vec!["import { b } from '../lib/b';"]),
                ("src/lib/b.ts", vec![]),
            ],
            vec!["src/lib/b.ts", "src/ui/a.ts"],
        ),
        (
            vec![
                ("src/a.ts", vec!["import { b } from './lib';"]),
                ("src/lib/index.ts", vec![]),
            ],
            vec!["src/lib/index.ts", "src/a.ts"],
        ),
        (
            vec![
                ("src/a.ts", vec!["import { b } from '~/lib/b';"]),
                ("lib/b.ts", vec![]),
            ],
            vec!["lib/b.ts", "src/a.ts"],
        ),
        (
            vec![
                ("src/a.ts", vec!["import { b } from '~/somewhere/b';"]),
                ("one/b.ts", vec![]),
                ("two/b.ts", vec![]),
            ],
            vec!["one/b.ts", "src/a.ts", "two/b.ts"],
        ),
        (
            vec![
                ("src/a.ts", vec!["import { b } from './b.js';"]),
                ("src/b.ts", vec![]),
            ],
            vec!["src/b.ts", "src/a.ts"],
        ),
        (
            vec![
                (
                    "src/a.ts",
                    vec!["const b = require('./b');", "import './c';"],
                ),
                ("src/b.ts", vec![]),
                ("src/c.ts", vec![]),
            ],
            vec!["src/b.ts", "src/c.ts", "src/a.ts"],
        ),
        (
            vec![
                ("src/b.ts", vec!["import { a } from './a';"]),
                ("src/a.ts", vec!["import { b } from './b';"]),
            ],
            vec!["src/a.ts", "src/b.ts"],
        ),
        (
            vec![
                ("pnpm-lock.yaml", vec![]),
                ("src/a.test.ts", vec![]),
                ("src/__tests__/b.ts", vec![]),
                ("src/a.ts", vec!["import { b } from './b';"]),
                ("src/b.ts", vec![]),
                ("dist/a.js", vec![]),
            ],
            vec![
                "src/b.ts",
                "src/a.ts",
                "src/__tests__/b.ts",
                "src/a.test.ts",
                "dist/a.js",
                "pnpm-lock.yaml",
            ],
        ),
        (
            vec![
                ("src/orphan.spec.ts", vec![]),
                ("src/a.test.ts", vec![]),
                ("src/a.ts", vec![]),
            ],
            vec!["src/a.ts", "src/a.test.ts", "src/orphan.spec.ts"],
        ),
        (
            vec![
                ("src/c.browser.test.tsx", vec![]),
                ("src/b.test.ts", vec![]),
                ("src/a.logic.test.ts", vec![]),
                ("src/c.tsx", vec![]),
                ("src/b.ts", vec![]),
                ("src/a.logic.ts", vec![]),
            ],
            vec![
                "src/a.logic.ts",
                "src/b.ts",
                "src/c.tsx",
                "src/a.logic.test.ts",
                "src/b.test.ts",
                "src/c.browser.test.tsx",
            ],
        ),
    ];
    for (input, expected) in cases {
        let files: Vec<_> = input
            .into_iter()
            .map(|(path, lines)| DiffFile {
                path: path.into(),
                addition_lines: lines.into_iter().map(str::to_owned).collect(),
                deletion_lines: vec![],
            })
            .collect();
        assert_eq!(
            order_diff_files(&files)
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn generated_and_test_tiers_follow_the_source_path_conventions() {
    for path in ["src/app.ts", "src/dist.ts", "src/testing.ts"] {
        assert_eq!(diff_file_tier(path), DiffFileTier::Source);
    }
    for path in [
        "src/__tests__/app.ts",
        "src/app.spec.tsx",
        "src/app.test.ts",
        "test/app.ts",
        "tests/helpers/app.ts",
    ] {
        assert_eq!(diff_file_tier(path), DiffFileTier::Test);
    }
    for path in [
        "apps/web/package-lock.json",
        "dist/app.js",
        "packages/core/vendor/lib.js",
        "pnpm-lock.yaml",
        "public/app.min.js",
        "src/__snapshots__/app.ts",
        "src/api.generated.ts",
        "src/app.test.ts.snap",
    ] {
        assert_eq!(diff_file_tier(path), DiffFileTier::Generated);
    }
}

#[test]
fn stats_batches_survive_pruning_and_reject_stale_scopes() {
    let rows: Vec<_> = (1..=501).map(row).collect();
    let candidates = rows.iter().map(EnvironmentPullRequestEntry::key).collect();
    let mut batches = stats_request_batches(
        &rows,
        &candidates,
        StatsPolicy::Visible,
        &[],
        &Default::default(),
        false,
    );
    assert_eq!(
        batches
            .iter()
            .map(|batch| batch.input.refs.len())
            .collect::<Vec<_>>(),
        vec![500, 1]
    );
    let mut stats = DiffStats::new();
    stats.insert(diff_stat_key(&rows[0]), (0, 0));
    retain_visible_stats_batches(&mut batches, &BTreeSet::new());
    assert!(batches.is_empty());
    let remaining = stats_request_batches(
        &rows,
        &candidates,
        StatsPolicy::Visible,
        &batches,
        &stats,
        false,
    );
    assert_eq!(
        remaining
            .iter()
            .map(|batch| batch.input.refs.len())
            .sum::<usize>(),
        500
    );
    let requested = StatsScope {
        key: "open".into(),
        policy: StatsPolicy::Visible,
    };
    for changed in [
        StatsScope {
            key: "closed".into(),
            policy: StatsPolicy::Visible,
        },
        StatsScope {
            key: "open".into(),
            policy: StatsPolicy::Eager,
        },
    ] {
        assert!(stats_refresh_batches(&requested, &changed, &rows, &candidates, &stats).is_none());
    }
    assert_eq!(
        stats_refresh_batches(&requested, &requested, &rows, &candidates, &stats).unwrap()[0]
            .input
            .refs
            .len(),
        500
    );
}

#[test]
fn author_and_label_facets_keep_counts_host_searches_and_selected_identities() {
    let mut first = row(1);
    first.entry.labels = decode(json!([{"name":"size:XL","color":"d93f0b"}]));
    let mut merged = row(2);
    merged.entry.state = PullRequestState::Merged;
    let (authors, labels) = collect_facets(&[first, merged], &PullRequestListState::Open);
    assert_eq!(authors[0].count, 1);
    assert_eq!(authors[0].merged_count, 1);
    let hosted: Vec<PullRequestActor> =
        decode(json!([{"login":"juliana","name":"Juliana","avatarUrl":null}]));
    let choices = author_choices(&authors, &hosted, "jul");
    assert_eq!(choices[0].actor.login.0, "juliana");
    assert_eq!(
        selected_author(&authors, &hosted, Some("JULIANA"))
            .unwrap()
            .actor
            .name
            .as_deref(),
        Some("Juliana")
    );
    assert_eq!(
        selected_author(&authors, &[], Some("missing"))
            .unwrap()
            .actor
            .login
            .0,
        "missing"
    );
    assert!(selected_author(&authors, &hosted, None).is_none());
    let choices = label_choices(
        &labels,
        &decode::<Vec<PullRequestLabel>>(
            json!([{"name":"size:XL","color":null},{"name":"bug","color":null}]),
        ),
        "xl-ish",
    );
    assert_eq!(
        choices
            .iter()
            .map(|choice| choice.count)
            .collect::<Vec<_>>(),
        vec![1, 0]
    );
    let many: Vec<_> = (0..40)
        .map(|index| AuthorFacet {
            actor: decode(json!({"login":format!("dev-{index}"),"name":null,"avatarUrl":null})),
            count: 1,
            merged_count: 0,
        })
        .collect();
    assert_eq!(
        author_choices(
            &many,
            &decode::<Vec<PullRequestActor>>(
                json!([{"login":"dev-host","name":null,"avatarUrl":null}])
            ),
            "dev"
        )[15]
            .actor
            .login
            .0,
        "dev-host"
    );
}

#[test]
fn priority_partitions_preserve_feed_order_and_environment_identity() {
    let authored = vec![row(1), row(2)];
    let reviewing = vec![row(1), row(3)];
    let rows = vec![row(4), row(2), row(5)];
    let groups = partition_with_priority(&rows, &authored, &reviewing);
    assert_eq!(
        groups
            .iter()
            .map(|group| group
                .entries
                .iter()
                .map(|entry| entry.entry.number)
                .collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        vec![vec![1, 2], vec![3], vec![4, 5]]
    );
    let mut other = row(1);
    other.environment = EnvironmentId("other".into());
    assert_ne!(other.key(), row(1).key());
}

#[test]
fn group_search_keeps_ranked_host_answers_and_appends_loaded_partial_word_matches() {
    let mut server = row(1);
    server.entry.title.0 = "Host matched body".into();
    let mut baseline = row(2);
    baseline.entry.title.0 = "Fix database".into();
    let filters = decode(json!({}));
    let viewers = Viewers::new();
    let known = [server];
    let baseline = [baseline];
    let providers: Vec<PullRequestProviderSummary> = decode(
        json!([{"host":"github.com","kind":"github","searchesOnHost":true,"projectCount":1,"configured":true,"detail":null}]),
    );
    let groups = pull_request_groups(GroupInput {
        known: &known,
        baseline: Some(&baseline),
        providers: &providers,
        viewers: &viewers,
        involvement: PullRequestInvolvement::All,
        local_filters: &filters,
        has_local_filters: false,
        text: "data",
        answered_locally: true,
        baseline_empty: false,
        held: None,
        authored: None,
        reviewing: None,
        partitions_wanted: false,
    });
    assert_eq!(
        groups
            .entries
            .iter()
            .map(|entry| entry.entry.number)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(groups.entries_by_stats_key.len(), 2);
    assert_eq!(
        surface_id(&row(1), true),
        "pull-request:origin:project:github.com:acme%2Fweb:1"
    );
}

#[test]
fn preferences_validate_bounds_normalize_defaults_and_recover_corruption() {
    assert_eq!(normalize_preferences(&json!({"involvement":"all","state":"open","q":"","labels":[],"sort":"ready","unknown":true})).unwrap(),default_preferences());
    for invalid in [
        json!({}),
        json!({"involvement":"invalid","state":"open"}),
        json!({"involvement":"all","state":"open","q":"a".repeat(201)}),
        json!({"involvement":"all","state":"open","labels":vec!["a";11]}),
        json!({"involvement":"all","state":"open","sort":"invalid"}),
    ] {
        assert!(normalize_preferences(&invalid).is_none());
    }
    let platform = FakePlatform::default();
    assert_eq!(block_on(read_preferences(&platform)), default_preferences());
    block_on(write_preferences(
        &platform,
        &json!({"involvement":"authored","state":"merged","sort":"largest"}),
    ))
    .unwrap();
    assert_eq!(block_on(read_preferences(&platform))["sort"], "largest");
}

#[test]
fn merge_controls_capabilities_and_checks_match_the_detail_rules() {
    for (supports, stack, pending, error, expected) in [
        (false, true, false, None, true),
        (false, false, true, None, true),
        (true, false, true, None, false),
        (true, false, false, Some("failed"), false),
        (true, true, false, None, false),
        (true, false, false, None, true),
    ] {
        assert_eq!(
            allows_single_merge(supports, stack, pending, error),
            expected
        );
    }
    let mut detail = make_detail();
    assert_eq!(
        allowed_merge_methods(&detail),
        vec![
            PullRequestMergeMethod::Merge,
            PullRequestMergeMethod::Squash
        ]
    );
    assert_eq!(thread_panel_action(&detail), Some("merge"));
    detail.checks = vec![check("pending")];
    assert_eq!(thread_panel_action(&detail), None);
    detail.checks = vec![check("failure")];
    assert_eq!(thread_panel_action(&detail), Some("fix"));
    detail.is_draft = true;
    assert_eq!(thread_panel_action(&detail), Some("ready"));
    detail.mergeability = PullRequestMergeability::Conflicting;
    assert_eq!(thread_panel_action(&detail), Some("resolve"));
    detail.state = PullRequestState::Closed;
    assert_eq!(thread_panel_action(&detail), None);
    assert!(can_delete_branch(&detail));
    detail.viewer_permissions.actions.clear();
    assert!(!can_delete_branch(&detail));
    assert_eq!(
        classify_checks(&[check("success"), check("cancelled")]),
        ChecksState::Failing
    );
    assert_eq!(
        classify_checks(&[check("action-required")]),
        ChecksState::Pending
    );
    assert_eq!(describe_checks(&[]), "No checks reported");
    assert_eq!(describe_checks(&[check("success")]), "All checks passed");
    assert_eq!(
        describe_checks(&[check("pending"), check("failure")]),
        "1 of 2 running · 1 failed"
    );
}

#[test]
fn primary_controls_keep_auto_merge_unknown_distinct_from_disabled() {
    for (auto, checks, expected) in [
        (
            Some(false),
            Some(PullRequestChecksState::Pending),
            "enable-auto-merge",
        ),
        (
            Some(false),
            Some(PullRequestChecksState::Failing),
            "enable-auto-merge",
        ),
        (None, Some(PullRequestChecksState::Pending), "merge"),
        (
            Some(true),
            Some(PullRequestChecksState::Passing),
            "auto-merge-armed",
        ),
    ] {
        assert_eq!(
            primary_control(PrimaryControlInput {
                state: &PullRequestState::Open,
                is_draft: false,
                mergeability: &PullRequestMergeability::Mergeable,
                checks: checks.as_ref(),
                auto_merge_enabled: auto,
                has_merge_method: true,
                can_merge: true,
                can_mark_ready: true,
                can_enable_auto_merge: true
            }),
            Some(expected)
        );
    }
}

#[test]
fn verdicts_keep_each_reviewers_last_decisive_word_and_invalidate_on_newer_commits() {
    let mut first = comment("1", "");
    first.author.as_mut().unwrap().login.0 = "maria".into();
    first.review_state = Some("APPROVED".into());
    let mut older = first.clone();
    older.id.0 = "older".into();
    older.review_state = Some("CHANGES_REQUESTED".into());
    older.created_at.0 = "2026-07-01T00:00:00Z".into();
    let mut nonverdict = first.clone();
    nonverdict.review_state = Some("COMMENTED".into());
    let commits: Vec<PullRequestCommit> = decode(
        json!([{"oid":"abc","messageHeadline":"new work","committedDate":"2026-07-04T00:00:00Z"}]),
    );
    let outcomes = latest_review_outcomes(&[first.clone(), older, nonverdict], &commits);
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].outcome, ReviewOutcome::Approved);
    assert!(outcomes[0].stale);
    let mut dismissed = first.clone();
    dismissed.review_state = Some("DISMISSED".into());
    assert!(latest_review_outcomes(&[first, dismissed], &[]).is_empty());
}

#[test]
fn conversations_group_threads_keep_loose_order_filter_bots_and_page_newest_items() {
    let mut root = comment("root", "Finding");
    root.created_at.0 = "2026-07-01T00:00:00Z".into();
    let mut reply = root.clone();
    reply.id.0 = "reply".into();
    reply.created_at.0 = "2026-07-03T00:00:00Z".into();
    let mut thread = thread("thread", "fallback");
    thread.comments = decode(json!([root, reply]));
    thread.comment_count = Some(Some(5));
    let loose = comment("loose", "Other");
    let comments = [root.clone(), loose, reply];
    let mut items = conversation_items(&comments, vec![ConversationThread::Review(&thread)]);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].comments[0].id.0, "root");
    assert_eq!(items[0].more_replies, 3);
    assert!(items[0].has_more_replies);
    assert_eq!(page_conversations(&items, 1).1, 1);
    filter_conversations(&mut items, false, false);
    assert_eq!(items.len(), 2);
    let mut bot = root;
    bot.author.as_mut().unwrap().is_bot = Some(Some(true));
    bot.review_state = Some("APPROVED".into());
    let bot_comments = [bot];
    let mut items = conversation_items(&bot_comments, vec![]);
    filter_conversations(&mut items, false, false);
    assert_eq!(items.len(), 1);
}

#[test]
fn snapshots_are_bounded_promote_resaves_and_validate_legacy_hosts() {
    let platform = FakePlatform::default();
    let mut reference = make_reference();
    let mut detail = make_detail();
    for number in 1..=21 {
        reference.number = number;
        detail.number = number;
        block_on(write_detail_snapshot(
            &platform,
            &env(),
            &reference,
            &detail,
        ))
        .unwrap();
    }
    reference.number = 1;
    assert!(block_on(read_detail_snapshot(&platform, &env(), &reference)).is_none());
    reference.number = 2;
    assert_eq!(
        block_on(read_detail_snapshot(&platform, &env(), &reference))
            .unwrap()
            .number,
        2
    );
    block_on(write_detail_snapshot(&platform, &env(), &reference, &{
        let mut detail = detail.clone();
        detail.number = 2;
        detail
    }))
    .unwrap();
    reference.number = 22;
    detail.number = 22;
    block_on(write_detail_snapshot(
        &platform,
        &env(),
        &reference,
        &detail,
    ))
    .unwrap();
    reference.number = 2;
    assert!(block_on(read_detail_snapshot(&platform, &env(), &reference)).is_some());
    reference.number = 3;
    assert!(block_on(read_detail_snapshot(&platform, &env(), &reference)).is_none());
    let reference = make_reference();
    let detail = make_detail();
    block_on(write_detail_snapshot(
        &platform,
        &env(),
        &reference,
        &detail,
    ))
    .unwrap();
    let mut hosted = reference.clone();
    hosted.host = Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
        "github.com".into(),
    )));
    assert!(block_on(read_detail_snapshot(&platform, &env(), &hosted)).is_some());
    hosted.host.as_mut().unwrap().as_mut().unwrap().0 = "other.test".into();
    assert!(block_on(read_detail_snapshot(&platform, &env(), &hosted)).is_none());
}

#[test]
fn failure_text_bounds_diagnostics_and_replaces_host_tool_noise() {
    for noise in [
        "",
        "unknown error.",
        "GitHub CLI command failed.",
        "GitLab API failed",
        "exited with status 1",
        "Pull request operation update failed: Forgejo command failed",
    ] {
        assert_eq!(readable_failure(noise, "hint"), "hint");
    }
    assert_eq!(
        readable_failure(
            "Pull request operation update failed: specific message",
            "hint"
        ),
        "specific message"
    );
    assert_eq!(
        readable_failure(&"x".repeat(400), "hint").chars().count(),
        320
    );
    assert!(
        checkout_failure_description(None, "unknown error").starts_with("Check that the branch")
    );
}

#[test]
fn timeline_marks_merged_ahead_of_closed_and_keeps_commit_authors() {
    let mut value = json!(make_detail());
    value["commits"] = json!([{"oid":"abcdef123","messageHeadline":"Add feature","committedDate":"2026-07-02T00:00:00Z","authors":[{"login":"maria","name":null,"avatarUrl":null}]}]);
    value["comments"] = json!([]);
    value["commentCount"] = json!(0);
    value["reviewThreads"] = json!([]);
    value["commentsTruncated"] = json!(false);
    value["mergedAt"] = json!("2026-07-04T00:00:00Z");
    value["closedAt"] = value["mergedAt"].clone();
    let events = build_timeline(&decode(value));
    assert_eq!(events[0].kind, "merged");
    assert!(!events.iter().any(|event| event.kind == "closed"));
    assert_eq!(events[1].commit_authors.len(), 1);
}

#[test]
fn review_drafts_keep_new_comments_and_revised_summaries_when_a_submission_finishes() {
    let mut store = PullRequestReviewStore::default();
    let key = PullRequestKey::new(env(), &make_reference());
    let draft =
        decode(json!({"body":"Finding","path":"a.ts","position":{"kind":"added","newLine":1}}));
    let first = store.add_comment(key.clone(), draft);
    let snapshot = BTreeSet::from([first]);
    store.add_comment(
        key.clone(),
        decode(json!({"body":"New finding","path":"a.ts","position":{"kind":"added","newLine":2}})),
    );
    store.remove_comments(&key, &snapshot);
    assert_eq!(store.comments(&key).len(), 1);
    store.set_summary(key.clone(), "revised".into());
    store.clear_submitted_summary(&key, "original");
    assert_eq!(store.summary(&key), Some("revised"));
    let mut other = key.clone();
    other.expected_account_id = Some("other-account".into());
    assert!(store.comments(&other).is_empty());
    other = key.clone();
    other.project.environment = EnvironmentId("other".into());
    assert_eq!(store.summary(&other), None);
}

#[test]
fn viewed_press_batches_wait_for_a_read_that_could_have_seen_them_and_keep_newer_presses() {
    let mut store = FilesViewedStore::default();
    let key = ViewedScopeKey {
        pull_request: PullRequestKey::new(env(), &make_reference()),
        allow_stale: None,
    };
    let answer: Rc<PullRequestFilesViewedResult> = Rc::new(decode(
        json!({"files":[{"path":"a.ts","state":"unviewed"}],"truncated":false}),
    ));
    store.subscribe(key.clone(), Some(answer.clone()));
    store.set_viewed(
        &key,
        decode(json!({"path":"a.ts","viewed":true,"revision":"abc"})),
        0,
    );
    assert!(store.take_due(399).is_empty());
    let batch = store.take_due(400).pop().unwrap();
    assert_eq!(
        batch.files[0]
            .revision
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .0,
        "abc"
    );
    assert!(store.complete(&batch, ViewedWriteResult::Success).refresh);
    store.answer(&key, Some(answer));
    assert_eq!(store.overlay(&key).unwrap().get("a.ts"), Some(&true));
    store.answer(
        &key,
        Some(Rc::new(decode(
            json!({"files":[{"path":"a.ts","state":"dismissed"}],"truncated":false}),
        ))),
    );
    assert!(store.overlay(&key).unwrap().is_empty());
    store.set_viewed(&key, decode(json!({"path":"a.ts","viewed":true})), 500);
    let old = store.flush(&key).unwrap();
    store.set_viewed(&key, decode(json!({"path":"a.ts","viewed":false})), 501);
    let new = store.flush(&key).unwrap();
    assert!(
        !store
            .complete(&old, ViewedWriteResult::Failure)
            .report_failure
    );
    assert_eq!(store.overlay(&key).unwrap().get("a.ts"), Some(&false));
    assert!(
        store
            .complete(&new, ViewedWriteResult::Failure)
            .report_failure
    );
    assert!(store.overlay(&key).unwrap().is_empty());
}

#[test]
fn last_viewed_subscriber_flushes_its_scope_and_ignores_late_answers() {
    let mut store = FilesViewedStore::default();
    let key = ViewedScopeKey {
        pull_request: PullRequestKey::new(env(), &make_reference()),
        allow_stale: None,
    };
    store.subscribe(key.clone(), None);
    store.subscribe(key.clone(), None);
    store.set_viewed(&key, decode(json!({"path":"a.ts","viewed":true})), 0);
    assert!(store.unsubscribe(&key).is_none());
    let batch = store.unsubscribe(&key).unwrap();
    assert!(store.overlay(&key).is_none());
    store.subscribe(key.clone(), None);
    let completion = store.complete(&batch, ViewedWriteResult::Failure);
    assert!(completion.refresh);
    assert!(!completion.report_failure);
    assert!(store.overlay(&key).unwrap().is_empty());
}

#[test]
fn viewed_overlay_fold_choices_and_hunk_ranges_match_both_sides() {
    let mut overlay = BTreeMap::from([("a".into(), true)]);
    let states = BTreeMap::from([("a".into(), PullRequestFileViewedState::Dismissed)]);
    settle_file_viewed_overlay(
        &mut overlay,
        Some(&states),
        &BTreeSet::new(),
        &BTreeSet::from(["a".into()]),
    );
    assert!(overlay.is_empty());
    let choices = BTreeMap::from([("a".into(), false)]);
    assert!(!is_file_diff_collapsed(
        "a",
        Some(DiffFoldOverride::Folded),
        &choices,
        true
    ));
    assert!(is_file_diff_collapsed("a", None, &BTreeMap::new(), true));
    let files = ["a", "b", "c"];
    assert_eq!(
        file_viewed_range(&files, "c", Some("a"), |file| *file),
        &files
    );
    assert_eq!(file_viewed_range(&files, "b", None, |file| *file), &["b"]);
    let ranges = [HunkRange {
        deletion_start: 3,
        deletion_count: 2,
        addition_start: 8,
        addition_count: 1,
    }];
    assert!(is_line_in_file_diff(&ranges, &PullRequestDiffSide::Left, 3));
    assert!(!is_line_in_file_diff(
        &ranges,
        &PullRequestDiffSide::Left,
        5
    ));
    assert!(is_line_in_file_diff(
        &ranges,
        &PullRequestDiffSide::Right,
        8
    ));
    assert!(!is_line_in_file_diff(
        &ranges,
        &PullRequestDiffSide::Right,
        3
    ));
}

#[test]
fn excerpt_expansion_ignores_other_hunk_offsets_and_rejects_deleted_files() {
    let source = (1..=100)
        .map(|line| {
            if line == 30 {
                "line 30 changed\n".into()
            } else {
                format!("line {line}\n")
            }
        })
        .collect::<String>();
    let rows = [
        ExcerptRow {
            hunk: 0,
            new_at: 30,
            old_at: 30,
            new_line: None,
            old_line: Some(30),
            text: "line 30\n".into(),
        },
        ExcerptRow {
            hunk: 0,
            new_at: 30,
            old_at: 31,
            new_line: Some(30),
            old_line: None,
            text: "line 30 changed\n".into(),
        },
    ];
    assert!(
        excerpt_expansion_old_contents(&source, &rows)
            .unwrap()
            .contains("line 29\nline 30\nline 31")
    );
    assert!(!excerpt_loader_allowed("deleted", &rows));
    assert!(!excerpt_loader_allowed("new", &rows));
    let excerpt = decode(json!({"path":"file.ts","side":"new","start":30,"end":30,"caption":""}));
    let (_, old) = excerpt_loaded_files(
        &excerpt,
        &rows,
        LoadedFile {
            name: "file.ts".into(),
            contents: source,
            cache_key: Some("hash".into()),
        },
        None,
    );
    assert_eq!(
        old.unwrap().cache_key.as_deref(),
        Some("hash:excerpt:new:30-30")
    );
}

#[test]
fn guide_run_watches_deduplicate_and_progress_is_scoped_to_environment_account_and_head() {
    let mut store = GuideRunStore::default();
    let key = PullRequestKey::new(env(), &make_reference());
    let watch = GuideRunWatch {
        key: key.clone(),
        title: Some("first".into()),
        previous_guide_at: None,
        previous_failed_at: None,
        since: 1,
        answered_running: true,
    };
    store.watch(watch.clone());
    let mut again = watch;
    again.title = Some("second".into());
    store.watch(again);
    assert_eq!(store.watches[&key].title.as_deref(), Some("first"));
    let progress = GuideProgressKey {
        run: key.clone(),
        head_sha: "head".into(),
    };
    store.set_progress(progress.clone(), |current| {
        current.position = 2;
        current.seen.insert(1);
    });
    let mut next = progress.clone();
    next.head_sha = "next".into();
    assert!(!store.progress.contains_key(&next));
    store.show_page_detail_tab(key.clone(), "guide".into());
    store.remove_environment(&env());
    assert!(store.progress.is_empty());
    assert!(store.watches.is_empty());
    assert!(store.page_detail_tab.is_none());
}

#[test]
fn summary_collapse_scroll_only_moves_a_pinned_section_that_is_above_the_viewport() {
    assert_eq!(
        section_collapse_anchor_scroll_top(500.0, 10.0, -30.0, 10.0),
        Some(460.0)
    );
    assert_eq!(
        section_collapse_anchor_scroll_top(20.0, 10.0, -30.0, 11.0),
        Some(0.0)
    );
    assert_eq!(
        section_collapse_anchor_scroll_top(500.0, 10.0, 10.0, 10.0),
        None
    );
    assert_eq!(
        section_collapse_anchor_scroll_top(500.0, 10.0, -30.0, 12.0),
        None
    );
}

#[test]
fn handoffs_bound_findings_attach_line_context_and_prioritize_failing_checks() {
    let readable = |body: &str| Some(body.to_owned());
    let text = HandoffText {
        provider: "github",
        host_label: "GitHub",
        readable_body: &readable,
    };
    let metadata = make_metadata();
    let threads = [thread("finding", "rename helper")];
    let handoff = fix_findings(
        &metadata,
        Findings {
            threads: &threads,
            comments: &[],
            checks: &[],
            comments_truncated: false,
        },
        false,
        &text,
    );
    assert!(!handoff.prompt.contains("rename helper"));
    assert!(handoff.prompt.contains("untrusted data"));
    assert_eq!(handoff.review_comments[1].text, "reviewer: rename helper");
    assert_eq!(handoff.review_comments[1].start_index, 11);
    assert_eq!(handoff.review_comments[1].range_label, "L12");
    let threads: Vec<_> = (0..24)
        .map(|index| thread(&index.to_string(), &"x".repeat(2_000)))
        .collect();
    let checks = [check("failure"), check("cancelled")];
    let handoff = fix_findings(
        &metadata,
        Findings {
            threads: &threads,
            comments: &[],
            checks: &checks,
            comments_truncated: true,
        },
        true,
        &text,
    );
    assert_eq!(handoff.review_comments.len(), 19);
    assert!(
        handoff
            .review_comments
            .iter()
            .skip(1)
            .all(|comment| comment.text.encode_utf16().count() <= 1_000)
    );
    assert!(handoff.prompt.contains("6 further findings"));
    assert!(handoff.prompt.contains("Failing checks:"));
    assert!(handoff.prompt.contains("GitHub"));
    assert!(handoff.prompt.contains("switching to it"));
}

#[test]
fn handoffs_preserve_existing_requests_and_replace_only_matching_chips() {
    let metadata = make_metadata();
    let fixed = Handoff {
        prompt: "Fix this.\n\nKeep the checks.\n".into(),
        review_comments: vec![pull_request_context(&metadata, "github")],
    };
    let ask = ask_about(&metadata, "github");
    let combined = append_handoff(&fixed, &ask);
    assert_eq!(combined, fixed);
    let mut incoming = Handoff {
        prompt: "Second line\nKeep the checks.".into(),
        review_comments: vec![reference_context(&metadata, "github")],
    };
    incoming.review_comments[0].pull_request = None;
    let combined = append_handoff(&fixed, &incoming);
    assert!(
        combined
            .prompt
            .contains("\n\nSecond line\nKeep the checks.")
    );
    assert_eq!(combined.review_comments.len(), 2);
    assert_eq!(append_handoff(&combined, &incoming), combined);
    let mut changed = incoming.clone();
    changed.review_comments[0].text = "new text".into();
    assert_eq!(
        append_handoff(&combined, &changed).review_comments[1].text,
        "new text"
    );
}

#[test]
fn handoffs_keep_repository_and_remark_namespaces_separate_and_use_gitlab_terms() {
    let readable = |body: &str| (!body.trim().is_empty()).then(|| body.to_owned());
    let text = HandoffText {
        provider: "gitlab",
        host_label: "GitLab",
        readable_body: &readable,
    };
    let mut metadata = make_metadata();
    metadata.url = "https://gitlab.com/acme/web/-/merge_requests/7".into();
    let comment = comment("same", "Useful remark");
    let first = comment_context(&metadata, &Finding::Comment(&comment), &text).unwrap();
    assert!(first.id.starts_with("pr-comment:"));
    assert_eq!(first.file_path, "@reviewer");
    let mut other = metadata.clone();
    other.url = "https://gitlab.com/acme/other/-/merge_requests/7".into();
    assert_ne!(
        first.id,
        comment_context(&other, &Finding::Comment(&comment), &text)
            .unwrap()
            .id
    );
    assert_eq!(
        explain(&metadata, "gitlab").prompt,
        "Explain this merge request."
    );
    assert!(resolve_conflicts(&metadata, false, "gitlab").starts_with("MR #7"));
    assert!(
        fix_finding(&metadata, &Finding::Comment(&comment), false, &text)
            .prompt
            .contains("merge request")
    );
    let mut empty = comment.clone();
    empty.body.clear();
    assert!(comment_context(&metadata, &Finding::Comment(&empty), &text).is_none());
}

#[test]
fn selection_handoffs_keep_requests_in_the_prompt_and_only_code_in_the_chip() {
    let metadata = make_metadata();
    let comment = pull_request_context(&metadata, "github");
    let handoff = add_selection(&metadata, &comment, "Explain these lines", "github");
    assert_eq!(handoff.prompt, "Explain these lines");
    assert!(handoff.review_comments[1].text.is_empty());
    assert_eq!(handoff.review_comments.len(), 2);
    let excerpts: Vec<t3_contracts::pull_request_review_guide::PullRequestReviewGuideExcerpt> =
        decode(json!([{"path":"a.ts","side":"old","start":1,"end":3,"caption":""}]));
    let guide = ask_about_guide_stop(
        &metadata,
        GuideStop {
            index: 0,
            title: "Read this",
            summary: "Summary",
            attention: Some("Risk"),
            excerpts: &excerpts,
        },
        "Why?",
        "github",
    );
    assert_eq!(guide.prompt, "Why?");
    assert!(
        guide.review_comments[1]
            .text
            .contains("removed; numbered as in the base branch")
    );
    assert!(
        guide.review_comments[0]
            .text
            .contains("Do not change any code")
    );
}

#[test]
fn markdown_ast_transforms_skip_existing_links_and_code_and_cache_per_repository() {
    let text = |value: &str| MarkdownNode {
        kind: "text".into(),
        value: Some(value.into()),
        ..Default::default()
    };
    let mut tree = MarkdownNode {
        kind: "root".into(),
        children: vec![
            text("Fix #12"),
            MarkdownNode {
                kind: "link".into(),
                children: vec![text("#13")],
                ..Default::default()
            },
            MarkdownNode {
                kind: "inlineCode".into(),
                value: Some("#14".into()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    remark_autolinks(&mut tree, &|value| {
        if value == "Fix #12" {
            vec![ReferenceAutolink {
                index: 4,
                text: "#12".into(),
                label: "#12".into(),
                url: "https://example.test/issues/12".into(),
                kind: "reference".into(),
            }]
        } else {
            panic!("must not visit existing links or code")
        }
    });
    assert_eq!(tree.children[1].kind, "link");
    assert_eq!(tree.children[1].autolink_kind.as_deref(), Some("reference"));
    assert_eq!(tree.children[2].children[0].value.as_deref(), Some("#13"));
    let mut cache = MarkdownPluginCache::default();
    let first = cache
        .plugins("github", Some("https://github.com/acme/web"), true)
        .unwrap();
    assert!(Rc::ptr_eq(
        &first,
        &cache
            .plugins("github", Some("https://github.com/acme/web"), true)
            .unwrap()
    ));
    assert!(cache.plugins("github", None, true).is_none());
}

#[test]
fn markdown_excerpts_and_html_previews_extract_words_and_bound_utf16_safely() {
    let tree = MarkdownNode {
        kind: "root".into(),
        children: vec![
            MarkdownNode {
                kind: "text".into(),
                value: Some("İ [!WARNING] Words".into()),
                ..Default::default()
            },
            MarkdownNode {
                kind: "code".into(),
                value: Some("hidden".into()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    assert_eq!(markdown_excerpt(&tree).as_deref(), Some("İ Words"));
    let html = HtmlNode::Root(vec![
        HtmlNode::Element {
            tag: "p".into(),
            alt: None,
            children: vec![HtmlNode::Text("First".into())],
        },
        HtmlNode::Element {
            tag: "img".into(),
            alt: Some("badge".into()),
            children: vec![],
        },
    ]);
    assert_eq!(comment_preview(&html), "First badge");
    assert_eq!(
        bound_utf16(&"😀".repeat(200), 280, "…")
            .encode_utf16()
            .count(),
        279
    );
}

#[test]
fn comment_order_preserves_the_hosts_sequence_and_reverses_it_only_for_newest() {
    let mut first = comment("host-first", "one");
    first.created_at.0 = "2026-07-05T00:00:00Z".into();
    let second = comment("host-second", "two");
    let mut comments = [first, second];
    order_comments(&mut comments, false);
    assert_eq!(comments[0].id.0, "host-first");
    order_comments(&mut comments, true);
    assert_eq!(comments[0].id.0, "host-second");
}

#[test]
fn base_freshness_exposes_only_the_intersection_of_offered_and_permitted_updates() {
    let mut detail = make_detail();
    detail.base_comparison = Some(Some(PullRequestBaseComparison::Behind));
    detail.behind_by = Some(Some(12));
    detail.capabilities.update_methods = Some(Some(vec![
        PullRequestUpdateMethod::Merge,
        PullRequestUpdateMethod::Rebase,
    ]));
    detail.viewer_permissions.update_methods = Some(Some(vec![PullRequestUpdateMethod::Merge]));
    assert_eq!(
        base_freshness(&detail),
        Some((Some(12), vec![PullRequestUpdateMethod::Merge]))
    );
    detail.base_comparison = Some(Some(PullRequestBaseComparison::Unknown));
    assert!(base_freshness(&detail).is_none());
    detail.base_comparison = None;
    assert!(base_freshness(&detail).is_none());
    let refs: Vec<t3_contracts::git::VcsRef> = decode(
        json!([{"name":"origin/feature/main","isDefault":true,"isRemote":true,"remoteName":"origin","current":false,"worktreePath":null}]),
    );
    assert!(is_stacked_base("main", &refs));
}

#[test]
fn repository_breadcrumb_preserves_the_forge_origin_and_namespace() {
    for (raw, expected) in [
        (
            "https://dev.azure.com/org/proj/_git/repo/pullrequest/14",
            Some("https://dev.azure.com/org/proj/_git/repo"),
        ),
        (
            "https://github.com/team/widget/pull/14#conversation",
            Some("https://github.com/team/widget"),
        ),
        (
            "https://forge.example:8443/group/sub/widget/-/merge_requests/14?view=1",
            Some("https://forge.example:8443/group/sub/widget"),
        ),
        (
            "https://codeberg.org/team/widget/pulls/14",
            Some("https://codeberg.org/team/widget"),
        ),
        ("https://forge.example/team/widget/pull/12", None),
        ("https://forge.example/team/widget/issues/14", None),
        ("https://token@forge.example/team/widget/pull/14", None),
        ("javascript:alert(14)", None),
    ] {
        assert_eq!(repository_link(raw, 14).as_deref(), expected, "{raw}");
    }
}

#[test]
fn deep_link_resolution_preserves_server_and_host_scope() {
    use t3_client::{view_logic::shell::panels::Surface, workspace::projects::EnvironmentProject};
    let checkout = |env: &str| EnvironmentProject {
        environment: EnvironmentId(env.into()),
        is_scratch: false,
        project: decode(json!({
            "id":"shared","title":"Widget","workspaceRoot":"/widget",
            "repositoryIdentity":{"canonicalKey":"forge.example/team/widget","displayName":"team/widget","locator":{"source":"git-remote","remoteName":"origin","remoteUrl":"https://forge.example/team/widget.git"}},
            "defaultModelSelection":null,"scripts":[],"createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"
        })),
    };
    let first = checkout("first");
    let second = checkout("second");
    let query =
        |extra: &str| ListSearch::parse(&format!("?repository=team%2Fwidget&number=14&{extra}"));
    let scoped = query("selectedProjectId=shared&environmentId=second&host=forge.example");
    assert!(
        scoped
            .detail_surface(std::slice::from_ref(&first), false)
            .is_none()
    );
    let projects = [first, second];
    let expected = Some(Surface::pull_request(
        Some("second"),
        "shared",
        Some("forge.example"),
        "team/widget",
        14,
    ));
    assert_eq!(scoped.detail_surface(&projects, true), expected);
    assert_eq!(query("selectedEnvironmentId=second&environmentId=first&selectedHost=forge.example&host=elsewhere").detail_surface(&projects,true),expected);
    assert!(
        query("selectedEnvironmentId=second&host=elsewhere")
            .detail_surface(&projects, true)
            .is_none()
    );
    assert!(query("").detail_surface(&projects, true).is_none());
    assert!(query("").detail_surface(&projects[..1], false).is_none());
}
