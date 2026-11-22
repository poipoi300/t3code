#![allow(clippy::unwrap_used)]

use serde_json::json;
use t3_client::{
    platform::{Storage, StorageArea, StorageKey, StorageWrite},
    testing::FakePlatform,
    view_logic::shell::ui_state::{PREFERENCES_NAMESPACE, STORAGE_KEY, UiPreferences, UiState},
    workspace::pr_views::reviewers::{
        ReviewerSearchInput, ReviewerSearchState, ReviewerTruncation, host_reviewer_query,
        merge_reviewer_candidates, reviewer_candidate_view, reviewer_entries,
    },
};
use t3_contracts::{base_schemas::TrimmedNonEmptyString, pull_request::*};

fn parse_ms(s: &str) -> Option<i64> {
    s.parse().ok()
}

fn reviewer_candidate(
    id: &str,
    login: &str,
    name: Option<&str>,
    is_requested: bool,
) -> PullRequestReviewerCandidate {
    PullRequestReviewerCandidate {
        is_bot: None,
        login: TrimmedNonEmptyString(login.into()),
        name: name.map(str::to_owned),
        avatar_url: None,
        id: TrimmedNonEmptyString(id.into()),
        kind: PullRequestReviewerKind::User,
        is_requested,
    }
}

#[test]
fn reviewer_candidate_merging_and_view_match_ts_rules() {
    let loaded = PullRequestReviewerCandidateList {
        candidates: vec![
            reviewer_candidate("1", "alice", Some("Alice"), true),
            reviewer_candidate("2", "bob", Some("Bob"), false),
        ],
        truncated: true,
    };
    let searched = PullRequestReviewerCandidateList {
        candidates: vec![
            reviewer_candidate("2", "bob", Some("Bob"), true),
            reviewer_candidate("3", "carol", Some("Carol Smith"), false),
        ],
        truncated: false,
    };

    // Loaded candidates retain their local `is_requested` state when also in search results.
    let merged = merge_reviewer_candidates(&loaded.candidates, &searched.candidates, "bob");
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].login.0, "bob");
    assert!(!merged[0].is_requested);
    assert_eq!(merged[1].login.0, "carol");

    let empty_search = ReviewerSearchState::default();
    let initial = reviewer_candidate_view(
        &ReviewerSearchInput {
            loaded: Some(&loaded),
            searchable: true,
            query: "",
            debounced_query: "",
        },
        &empty_search,
    );
    assert_eq!(initial.truncation, Some(ReviewerTruncation::Searchable));
    assert!(!initial.search_pending);

    let unsearchable = reviewer_candidate_view(
        &ReviewerSearchInput {
            loaded: Some(&loaded),
            searchable: false,
            query: "",
            debounced_query: "",
        },
        &empty_search,
    );
    assert_eq!(
        unsearchable.truncation,
        Some(ReviewerTruncation::Unsearchable)
    );

    assert_eq!(
        host_reviewer_query(&ReviewerSearchInput {
            loaded: Some(&loaded),
            searchable: true,
            query: "carol",
            debounced_query: "  carol ",
        }),
        Some("carol".into())
    );

    let pending = reviewer_candidate_view(
        &ReviewerSearchInput {
            loaded: Some(&loaded),
            searchable: true,
            query: "carol",
            debounced_query: "",
        },
        &empty_search,
    );
    assert!(pending.search_pending);

    let failed = reviewer_candidate_view(
        &ReviewerSearchInput {
            loaded: Some(&loaded),
            searchable: true,
            query: "carol",
            debounced_query: "carol",
        },
        &ReviewerSearchState {
            data: None,
            is_pending: false,
            error: Some("network"),
        },
    );
    assert_eq!(failed.truncation, Some(ReviewerTruncation::SearchFailed));
    assert_eq!(failed.search_error.as_deref(), Some("network"));
}

#[test]
fn merge_method_cas_save_preserves_sidebar_preferences_and_unknown_fields() {
    futures::executor::block_on(async {
        let storage = FakePlatform::default();
        let key = StorageKey::new(PREFERENCES_NAMESPACE, None, STORAGE_KEY);
        let initial = json!({
            "projectExpandedById": {"logical-a": false},
            "projectOrder": ["logical-a"],
            "pullRequestMergeMethod": "merge",
            "future": {"keep": true}
        });
        storage
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: key.clone(),
                    value: Some(serde_json::to_vec(&initial).unwrap()),
                }],
            )
            .await
            .unwrap();

        UiPreferences::save_pull_request_merge_method(&storage, PullRequestMergeMethod::Squash)
            .await
            .unwrap();

        assert_eq!(
            UiPreferences::read_pull_request_merge_method(&storage)
                .await
                .unwrap(),
            PullRequestMergeMethod::Squash
        );
        let raw = storage
            .get(StorageArea::Ordinary, key)
            .await
            .unwrap()
            .unwrap();
        let state = UiState::decode(std::str::from_utf8(&raw).unwrap(), &parse_ms).unwrap();
        assert_eq!(
            state.pull_request_merge_method,
            PullRequestMergeMethod::Squash
        );
        assert_eq!(state.project_expanded_by_id.get("logical-a"), Some(&false));
        assert_eq!(state.project_order, vec!["logical-a".to_string()]);
        assert_eq!(state.unknown.get("future"), Some(&json!({"keep": true})));
    });
}

#[test]
fn reviewer_entries_deduplicate_case_insensitively_with_verdicts() {
    let detail: PullRequestDetail = serde_json::from_value(json!({
        "provider": "github",
        "capabilities": {
            "actions": [],
            "mergeMethods": ["merge"],
            "diff": true,
            "comment": true,
            "search": true,
            "review": {"inlineComment": true, "reply": true, "resolve": true, "verdicts": []},
            "reviewers": {"request": true, "listCandidates": true}
        },
        "viewerPermissions": {
            "actions": [],
            "comment": true,
            "resolve": true,
            "verdicts": [],
            "requestReviewers": true
        },
        "projectId": "p1",
        "projectTitle": "Project",
        "workspaceRoot": "/repo",
        "repository": "acme/web",
        "number": 10,
        "title": "PR",
        "body": "",
        "url": "https://github.com/acme/web/pull/10",
        "author": null,
        "state": "open",
        "isDraft": false,
        "mergeability": "mergeable",
        "additions": 1,
        "deletions": 0,
        "changedFiles": 1,
        "headBranch": "feat",
        "baseBranch": "main",
        "createdAt": "2026-01-01T00:00:00Z",
        "updatedAt": "2026-01-02T00:00:00Z",
        "mergedAt": null,
        "closedAt": null,
        "reviewers": [{"login": "Alice", "name": "Alice", "avatarUrl": null}],
        "labels": [],
        "checks": [],
        "mergeCapabilities": {"merge": true, "squash": false, "rebase": false}
    }))
    .unwrap();
    let comments: Vec<PullRequestComment> = serde_json::from_value(json!([
        {
            "id": "c1",
            "kind": "review",
            "url": "https://github.com/acme/web/pull/10#r1",
            "path": null,
            "author": {"login": "alice", "name": null, "avatarUrl": null},
            "body": "LGTM",
            "reviewState": "APPROVED",
            "createdAt": "2026-01-03T00:00:00Z"
        },
        {
            "id": "c2",
            "kind": "review",
            "url": "https://github.com/acme/web/pull/10#r2",
            "path": null,
            "author": {"login": "bob", "name": "Bob", "avatarUrl": null},
            "body": "Fix",
            "reviewState": "CHANGES_REQUESTED",
            "createdAt": "2026-01-04T00:00:00Z"
        }
    ]))
    .unwrap();

    let entries = reviewer_entries(&detail, &comments, &[]);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].key, "Alice");
    assert_eq!(entries[1].key, "bob");
}

#[test]
fn resolve_legacy_merge_method_override_matches_logical_project_scope() {
    use t3_client::{
        types::{EnvironmentId, ProjectId},
        workspace::{
            pr_views::detail::resolve_legacy_merge_method_override, projects::EnvironmentProject,
        },
    };
    use t3_contracts::{
        orchestration_project::OrchestrationProjectShell, settings::ClientSettingsSchema,
    };

    let env_id = EnvironmentId("env-1".into());
    let project_id = ProjectId("proj-1".into());
    let record: OrchestrationProjectShell = serde_json::from_value(json!({
        "id": "proj-1",
        "title": "Repo",
        "workspaceRoot": "/repo",
        "defaultModelSelection": null,
        "scripts": [],
        "createdAt": "2026-01-01T00:00:00Z",
        "updatedAt": "2026-01-01T00:00:00Z"
    }))
    .unwrap();
    let project = EnvironmentProject {
        environment: env_id.clone(),
        project: record,
        is_scratch: false,
    };

    let mut client_settings: ClientSettingsSchema = serde_json::from_value(json!({})).unwrap();
    assert_eq!(
        resolve_legacy_merge_method_override(
            &client_settings,
            std::slice::from_ref(&project),
            &env_id,
            &project_id,
        ),
        None
    );

    let key = TrimmedNonEmptyString(project.physical_key().legacy_preference_key());
    client_settings
        .pull_request_merge_method_overrides
        .insert(key, PullRequestMergeMethod::Rebase);
    assert_eq!(
        resolve_legacy_merge_method_override(
            &client_settings,
            std::slice::from_ref(&project),
            &env_id,
            &project_id,
        ),
        Some(PullRequestMergeMethod::Rebase)
    );
}
