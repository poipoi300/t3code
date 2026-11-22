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

#[test]
fn interleaved_reorder_projects_and_merge_method_save_preserve_both_fields() {
    use futures::{
        FutureExt,
        channel::{mpsc, oneshot},
        executor::LocalPool,
        future::LocalBoxFuture,
        stream::LocalBoxStream,
        task::LocalSpawnExt,
    };
    use std::{cell::RefCell, rc::Rc};
    use t3_client::{
        ClientStore,
        composition::{ClientRuntime, Spawner},
        navigation::RoutePolicy,
        platform::{
            HostAction, HostFuture, HostReply, HttpRequest, HttpResponse, Platform, PlatformError,
            PlatformEvent, SocketRequest, StorageTransaction, WebSocket,
        },
    };
    use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;

    type HeldRead = Rc<RefCell<Option<oneshot::Receiver<Option<Vec<u8>>>>>>;

    #[derive(Clone)]
    struct RacePlatform {
        inner: FakePlatform,
        held: HeldRead,
    }
    impl Storage for RacePlatform {
        fn transaction(&self, tx: StorageTransaction) -> HostFuture<'_, bool> {
            self.inner.transaction(tx)
        }
        fn get(&self, area: StorageArea, key: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
            if area == StorageArea::Ordinary
                && key.record == STORAGE_KEY
                && let Some(rx) = self.held.borrow_mut().take()
            {
                return async move { rx.await.map_err(|_| PlatformError::Cancelled) }.boxed_local();
            }
            self.inner.get(area, key)
        }
        fn write(&self, area: StorageArea, writes: Vec<StorageWrite>) -> HostFuture<'_, ()> {
            self.inner.write(area, writes)
        }
        fn list(&self, area: StorageArea, namespace: String) -> HostFuture<'_, Vec<StorageKey>> {
            self.inner.list(area, namespace)
        }
    }
    impl Platform for RacePlatform {
        fn now_epoch_ms(&self) -> u64 {
            self.inner.now_epoch_ms()
        }
        fn monotonic_ms(&self) -> u64 {
            self.inner.monotonic_ms()
        }
        fn random_bytes(&self, length: usize) -> Result<Vec<u8>, PlatformError> {
            self.inner.random_bytes(length)
        }
        fn sleep(&self, milliseconds: u64) -> LocalBoxFuture<'_, ()> {
            self.inner.sleep(milliseconds)
        }
        fn yield_to_host(&self) -> LocalBoxFuture<'_, ()> {
            self.inner.yield_to_host()
        }
        fn set_route_policy(&self, policy: RoutePolicy) {
            self.inner.set_route_policy(policy)
        }
        fn events(&self) -> LocalBoxStream<'static, PlatformEvent> {
            let (_tx, rx) = mpsc::unbounded();
            Box::pin(rx)
        }
        fn http(&self, request: HttpRequest) -> HostFuture<'_, HttpResponse> {
            self.inner.http(request)
        }
        fn websocket(&self, request: SocketRequest) -> HostFuture<'_, Box<dyn WebSocket>> {
            self.inner.websocket(request)
        }
        fn host(&self, action: HostAction) -> HostFuture<'_, HostReply> {
            self.inner.host(action)
        }
    }

    let mut pool = LocalPool::new();
    let pool_spawner = pool.spawner();
    let spawner: Spawner = Rc::new(move |fut| {
        pool_spawner.spawn_local(fut).expect("spawn local");
    });

    let base = FakePlatform::default();
    let key = StorageKey::new(PREFERENCES_NAMESPACE, None, STORAGE_KEY);
    let initial =
        br#"{"projectOrder":["a","b"],"pullRequestMergeMethod":"merge","futureFlag":42}"#.to_vec();
    pool.run_until(async {
        base.write(
            StorageArea::Ordinary,
            vec![StorageWrite {
                key: key.clone(),
                value: Some(initial.clone()),
            }],
        )
        .await
        .unwrap();
    });

    let (held_tx, held_rx) = oneshot::channel();
    let platform = RacePlatform {
        inner: base.clone(),
        held: Rc::new(RefCell::new(Some(held_rx))),
    };
    let client = ClientRuntime::new(
        platform,
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        spawner,
    );

    let ops = client.project_actions();
    let reorder_handle = pool
        .spawner()
        .spawn_local_with_handle(
            ops.reorder_projects(
                vec!["a".into(), "b".into()],
                vec!["b".into()],
                vec!["a".into()],
            )
            .map(|r| r.expect("reorder_projects")),
        )
        .expect("spawn reorder");

    // Advance reorder_projects until it is parked awaiting the held storage read.
    pool.run_until_stalled();

    // Commit a concurrent merge-method update while reorder_projects holds the stale snapshot.
    pool.run_until(async {
        client
            .set_pull_request_merge_method(PullRequestMergeMethod::Squash)
            .await
            .expect("save merge method");
    });

    // Deliver the stale pre-Squash snapshot to reorder_projects and let it finish.
    held_tx.send(Some(initial)).expect("deliver stale snapshot");
    pool.run_until(reorder_handle);

    let (method, raw) = pool.run_until(async {
        let method = UiPreferences::read_pull_request_merge_method(&base)
            .await
            .unwrap();
        let raw = base.get(StorageArea::Ordinary, key).await.unwrap().unwrap();
        (method, raw)
    });
    let decoded = UiState::decode(std::str::from_utf8(&raw).unwrap(), &parse_ms).unwrap();
    assert_eq!(method, PullRequestMergeMethod::Squash);
    assert_eq!(
        decoded.pull_request_merge_method,
        PullRequestMergeMethod::Squash
    );
    assert_eq!(
        decoded.project_order,
        vec!["b".to_string(), "a".to_string()]
    );
    assert_eq!(decoded.unknown.get("futureFlag"), Some(&json!(42)));
}

#[test]
fn save_pull_request_merge_method_recovers_from_corrupt_v1_record() {
    futures::executor::block_on(async {
        let platform = FakePlatform::default();
        let key = StorageKey::new(PREFERENCES_NAMESPACE, None, STORAGE_KEY);
        platform
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: key.clone(),
                    value: Some(b"{not valid json".to_vec()),
                }],
            )
            .await
            .unwrap();

        UiPreferences::save_pull_request_merge_method(&platform, PullRequestMergeMethod::Rebase)
            .await
            .expect("corrupt v1 record should recover to defaults with updated method");
        assert_eq!(
            UiPreferences::read_pull_request_merge_method(&platform)
                .await
                .unwrap(),
            PullRequestMergeMethod::Rebase
        );
    });
}
