#![allow(clippy::unwrap_used)]

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use t3_client::{
    types::{EnvironmentId, Generation},
    workspace::{pr_views::queries::*, pull_requests::*},
};
use t3_contracts::pull_request::*;

fn decode<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}
fn env() -> EnvironmentId {
    EnvironmentId("origin".into())
}
fn reference() -> PullRequestRef {
    decode(json!({"projectId":"project", "host":"github.com", "repository":"acme/web", "number":7}))
}
fn summary(state: &str) -> PullRequestSummary {
    decode(
        json!({"provider":"github", "projectId":"project", "repository":"acme/web", "number":7,
    "title":"New title", "url":"https://github.com/acme/web/pull/7", "state":state, "isDraft":false,
    "headBranch":"feature", "baseBranch":"main", "updatedAt":"2026-07-02T00:00:00Z", "observedAt":200}),
    )
}
fn shown(state: &str) -> Value {
    json!({"projectId":"project", "repository":"acme/web", "number":7,
    "title":"Old title", "state":state, "updatedAt":"2026-07-01T00:00:00Z", "observedAt":100,
    "checks":[{"name":"CI"}], "headSha":"old", "autoMergeEnabled":true, "autoMergeMethod":"squash"})
}
fn loaded(registry: &mut QueryRegistry, kind: QueryKind, input: Value, value: Value) -> QueryKey {
    let key = registry.register(env(), kind, &input).unwrap();
    let ticket = registry.begin(&key, Generation(1)).unwrap();
    registry.finish(&key, ticket, Ok(value), 10).unwrap();
    key
}

#[test]
fn references_normalize_property_order_and_keep_host_account_and_stale_scopes() {
    let mut registry = QueryRegistry::default();
    let first = registry
        .register(
            env(),
            QueryKind::Checks,
            &json!({"number":7,"repository":"acme/web","host":"github.com","projectId":"project"}),
        )
        .unwrap();
    assert_eq!(
        first,
        registry
            .register(env(), QueryKind::Checks, &reference())
            .unwrap()
    );
    for field in ["host", "expectedAccountId", "allowStale"] {
        let mut input = json!(reference());
        input[field] = match field {
            "host" => json!("enterprise.test"),
            "expectedAccountId" => json!("other"),
            _ => json!(false),
        };
        assert_ne!(
            first,
            registry.register(env(), QueryKind::Checks, &input).unwrap()
        );
    }
    assert_ne!(
        first,
        registry
            .register(
                EnvironmentId("other".into()),
                QueryKind::Checks,
                &reference()
            )
            .unwrap()
    );
}

#[test]
fn action_patch_keeps_checks_read_time_and_detail_read_version() {
    let mut registry = QueryRegistry::default();
    let key = loaded(
        &mut registry,
        QueryKind::Detail,
        json!(reference()),
        shown("open"),
    );
    assert_eq!(registry.detail_read_version(&key), 1);
    registry
        .apply_action_reply(
            &env(),
            &reference(),
            Some(&summary("closed")),
            &Default::default(),
            20,
        )
        .unwrap();
    let row = registry.get(&key).unwrap();
    assert_eq!(row.state.value.as_ref().unwrap()["state"], "closed");
    assert_eq!(
        row.state.value.as_ref().unwrap()["checks"],
        json!([{"name":"CI"}])
    );
    assert_eq!(row.state.value.as_ref().unwrap()["observedAt"], 100);
    assert_eq!(row.state.updated_at, Some(10));
    assert_eq!(registry.detail_read_version(&key), 1);
    let next = registry.begin(&key, Generation(1)).unwrap();
    assert_eq!(registry.detail_read_version(&key), 1);
    registry
        .finish(&key, next, Ok(shown("closed")), 30)
        .unwrap();
    assert_eq!(registry.detail_read_version(&key), 2);
}

#[test]
fn action_patch_cancels_old_reads_but_keeps_a_post_commit_read() {
    for post_commit in [false, true] {
        let mut registry = QueryRegistry::default();
        let key = loaded(
            &mut registry,
            QueryKind::Detail,
            json!(reference()),
            shown("open"),
        );
        let before = if post_commit {
            Default::default()
        } else {
            registry.begin(&key, Generation(1));
            registry.reads_in_flight()
        };
        let ticket = if post_commit {
            registry.begin(&key, Generation(1)).unwrap()
        } else {
            before[&key]
        };
        registry
            .apply_action_reply(&env(), &reference(), Some(&summary("closed")), &before, 20)
            .unwrap();
        assert_eq!(registry.get(&key).unwrap().state.loading, post_commit);
        assert_eq!(
            registry
                .finish(&key, ticket, Ok(shown("closed")), 30)
                .unwrap(),
            post_commit
        );
    }
}

#[test]
fn reopen_and_changed_head_refresh_incomplete_details_while_showing_the_reply() {
    for (state, head) in [("closed", None), ("open", Some("new"))] {
        let mut registry = QueryRegistry::default();
        let key = loaded(
            &mut registry,
            QueryKind::Detail,
            json!(reference()),
            shown(state),
        );
        let mut reply = json!(summary("open"));
        if let Some(head) = head {
            reply["headSha"] = json!(head);
        }
        registry
            .apply_action_reply(
                &env(),
                &reference(),
                Some(&decode(reply)),
                &Default::default(),
                20,
            )
            .unwrap();
        assert!(registry.get(&key).unwrap().refresh_requested);
        assert_eq!(
            registry.get(&key).unwrap().state.value.as_ref().unwrap()["state"],
            "open"
        );
    }
}

#[test]
fn disarming_auto_merge_drops_its_method_and_does_not_unmerge_a_row() {
    let mut registry = QueryRegistry::default();
    let key = loaded(
        &mut registry,
        QueryKind::Detail,
        json!(reference()),
        shown("open"),
    );
    let mut reply = json!(summary("open"));
    reply["autoMergeEnabled"] = json!(false);
    registry
        .apply_action_reply(
            &env(),
            &reference(),
            Some(&decode(reply)),
            &Default::default(),
            20,
        )
        .unwrap();
    assert!(
        registry
            .get(&key)
            .unwrap()
            .state
            .value
            .as_ref()
            .unwrap()
            .get("autoMergeMethod")
            .is_none()
    );
    let merged = loaded(
        &mut registry,
        QueryKind::PreviewCard,
        json!(reference()),
        shown("merged"),
    );
    registry
        .apply_action_reply(
            &env(),
            &reference(),
            Some(&summary("open")),
            &Default::default(),
            30,
        )
        .unwrap();
    assert_eq!(
        registry.get(&merged).unwrap().state.value.as_ref().unwrap()["state"],
        "merged"
    );
}

#[test]
fn unstamped_previews_and_legacy_action_replies_reread_only_loaded_readers() {
    let mut registry = QueryRegistry::default();
    let preview = loaded(
        &mut registry,
        QueryKind::Preview,
        json!(reference()),
        json!({"state":"open"}),
    );
    let card = loaded(
        &mut registry,
        QueryKind::PreviewCard,
        json!(reference()),
        shown("open"),
    );
    registry
        .apply_action_reply(
            &env(),
            &reference(),
            Some(&summary("closed")),
            &Default::default(),
            20,
        )
        .unwrap();
    assert!(registry.get(&preview).unwrap().refresh_requested);
    assert_eq!(
        registry
            .get(&preview)
            .unwrap()
            .state
            .value
            .as_ref()
            .unwrap()["state"],
        "open"
    );
    assert_eq!(
        registry.get(&card).unwrap().state.value.as_ref().unwrap()["state"],
        "closed"
    );
    registry
        .apply_action_reply(&env(), &reference(), None, &Default::default(), 30)
        .unwrap();
    assert!(registry.get(&card).unwrap().refresh_requested);
}

#[test]
fn patched_summaries_preserve_identity_and_read_time_and_persist_the_answer() {
    let mut registry = QueryRegistry::default();
    let key = loaded(
        &mut registry,
        QueryKind::LinkedSummary,
        json!(reference()),
        shown("open"),
    );
    registry.take_answers();
    registry
        .apply_action_reply(
            &env(),
            &reference(),
            Some(&summary("closed")),
            &Default::default(),
            20,
        )
        .unwrap();
    let value = registry.get(&key).unwrap().state.value.as_ref().unwrap();
    assert_eq!(value["observedAt"], 100);
    assert_eq!(value["projectId"], "project");
    let answers = registry.take_answers();
    assert_eq!(answers.len(), 1);
    assert!(answers[0].action_patch);
    assert_eq!(registry.take_action_replies().len(), 1);
    let persisted: Value = registry
        .persistence
        .load(
            &env(),
            &persistence_key(QueryKind::LinkedSummary, &reference()).unwrap(),
            20,
        )
        .unwrap();
    assert_eq!(persisted["state"], "closed");
}

#[test]
fn edits_invalidate_preview_and_hover_card_and_labels_keep_pending_reads_refreshable() {
    let mut registry = QueryRegistry::default();
    let preview = loaded(
        &mut registry,
        QueryKind::Preview,
        json!(reference()),
        shown("open"),
    );
    let card = loaded(
        &mut registry,
        QueryKind::PreviewCard,
        json!(reference()),
        shown("open"),
    );
    registry
        .command_succeeded(
            &env(),
            "pullRequests.update",
            &json!(reference()),
            &Value::Null,
            Generation(1),
        )
        .unwrap();
    assert!(registry.get(&preview).unwrap().refresh_requested);
    assert!(registry.get(&card).unwrap().refresh_requested);
    let detail = loaded(
        &mut registry,
        QueryKind::Detail,
        json!(reference()),
        json!({"labels":[]}),
    );
    registry.begin(&detail, Generation(1));
    registry.apply_labels(&env(), &reference(), &["bug".into()], true);
    assert!(registry.get(&detail).unwrap().refresh_requested);
    assert_eq!(
        registry.get(&detail).unwrap().state.value.as_ref().unwrap()["labels"],
        json!([{"name":"bug","color":null}])
    );
}

#[test]
fn server_refresh_excludes_pages_and_viewed_files_and_polls_only_eligible_readers() {
    let mut registry = QueryRegistry::default();
    let ordinary = loaded(
        &mut registry,
        QueryKind::PreviewCard,
        json!(reference()),
        shown("open"),
    );
    let page = loaded(
        &mut registry,
        QueryKind::List,
        json!({"state":"open","involvement":"all","cursors":{}}),
        Value::Null,
    );
    let viewed = loaded(
        &mut registry,
        QueryKind::FilesViewed,
        json!(reference()),
        Value::Null,
    );
    for key in [&ordinary, &page, &viewed] {
        registry.subscribe(key);
    }
    registry.server_refresh(&env(), Generation(1), false);
    assert_eq!(registry.due_refreshes(20, true), vec![ordinary]);
    assert!(!registry.get(&page).unwrap().refresh_requested);
    assert!(!registry.get(&viewed).unwrap().refresh_requested);
}

#[test]
fn linked_polling_uses_loaded_status_and_server_capability() {
    let mut registry = QueryRegistry::default();
    let linked = loaded(
        &mut registry,
        QueryKind::LinkedSummary,
        json!(reference()),
        shown("open"),
    );
    let threads = loaded(
        &mut registry,
        QueryKind::LinkedThreads,
        json!(reference()),
        Value::Null,
    );
    for key in [&linked, &threads] {
        registry.subscribe(key);
    }
    assert_eq!(registry.due_refreshes(10_010, false), vec![threads.clone()]);
    assert!(registry.due_refreshes(10_010, true).is_empty());
    assert!(registry.due_refreshes(60_010, true).contains(&linked));
    let ticket = registry.begin(&linked, Generation(1)).unwrap();
    registry
        .finish(&linked, ticket, Ok(shown("merged")), 60_010)
        .unwrap();
    assert!(!registry.due_refreshes(120_010, true).contains(&linked));
}

#[test]
fn stats_restore_individual_rows_after_batch_regrouping_and_isolate_hosts() {
    let mut cache = PullRequestQueryCache::default();
    let input = PullRequestListStatsInput {
        refs: vec![reference()],
    };
    let stats = decode(
        json!({"stats":[{"projectId":"project","repository":"acme/web","number":7,"additions":0,"deletions":0}]}),
    );
    cache.save_stats(env(), &input, &stats, 10).unwrap();
    let mut next = input.clone();
    let mut absent = reference();
    absent.number = 8;
    next.refs.push(absent);
    assert_eq!(cache.load_stats(&env(), &next, 20).unwrap(), stats);
    next.refs[0].host = Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
        "enterprise.test".into(),
    )));
    assert!(cache.load_stats(&env(), &next, 20).is_none());
    assert!(
        cache
            .load_stats(&EnvironmentId("other".into()), &input, 20)
            .is_none()
    );
}

#[test]
fn command_lanes_separate_hosts_for_diff_and_order_viewed_writes_per_pull_request() {
    let mut input = json!(reference());
    input["oldPath"] = json!("a.ts");
    input["newPath"] = json!("a.ts");
    let first = command_lane(&env(), "pullRequests.diffFileContents", &input);
    input["host"] = json!("enterprise.test");
    assert_ne!(
        first,
        command_lane(&env(), "pullRequests.diffFileContents", &input)
    );
    assert!(matches!(
        command_lane(&env(), "pullRequests.setFilesViewed", &input),
        CommandLane::SerialViewed(..)
    ));
    assert_eq!(
        command_lane(&env(), "pullRequests.generateReviewGuide", &input),
        CommandLane::Unscheduled
    );
}

#[test]
fn query_tickets_never_accept_a_reply_from_a_removed_registration_or_twice() {
    let mut registry = QueryRegistry::default();
    let key = registry
        .register(env(), QueryKind::Detail, &reference())
        .unwrap();
    let old = registry.begin(&key, Generation(1)).unwrap();
    registry.remove_environment(&env());
    let again = registry
        .register(env(), QueryKind::Detail, &reference())
        .unwrap();
    let current = registry.begin(&again, Generation(1)).unwrap();
    assert!(!registry.finish(&again, old, Ok(shown("open")), 10).unwrap());
    assert!(
        registry
            .finish(&again, current, Ok(shown("closed")), 20)
            .unwrap()
    );
    assert!(
        !registry
            .finish(&again, current, Ok(shown("open")), 30)
            .unwrap()
    );
}

#[test]
fn action_list_patches_honor_quoted_and_negated_draft_qualifiers_and_page_bounds() {
    for (query, retained) in [
        ("draft:true", false),
        ("draft:\"true\"", false),
        ("-draft:false", false),
        ("-draft:true", true),
        ("is:draft", true),
        ("draft:false draft:true", false),
    ] {
        let input: PullRequestListInput =
            decode(json!({"involvement":"all","state":"open","query":query}));
        let mut value = json!(summary("open"));
        value["host"] = json!("github.com");
        value["projectTitle"] = json!("Project");
        value["author"] = Value::Null;
        value["mergeability"] = json!("unknown");
        value["additions"] = json!(0);
        value["deletions"] = json!(0);
        value["createdAt"] = json!("2026-07-01T00:00:00Z");
        value["viewerReviewRequested"] = json!(false);
        value["labels"] = json!([]);
        let entry: PullRequestListEntry = decode(value);
        let ListPatch::Patched(entries) =
            patch_list_entries(&input, &[entry], &reference(), &summary("open")).unwrap()
        else {
            panic!("{query}")
        };
        assert_eq!(!entries.is_empty(), retained, "{query}");
    }
    let first: PullRequestListInput = decode(json!({"involvement":"all","state":"open"}));
    assert_eq!(
        patch_list_entries(&first, &[], &reference(), &summary("open")).unwrap(),
        ListPatch::Refresh
    );
    let page: PullRequestListInput =
        decode(json!({"involvement":"all","state":"open","cursors":{}}));
    assert_eq!(
        patch_list_entries(&page, &[], &reference(), &summary("open")).unwrap(),
        ListPatch::Unchanged
    );
}
