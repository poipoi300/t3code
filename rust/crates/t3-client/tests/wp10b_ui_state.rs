#![allow(clippy::unwrap_used)]
use serde_json::json;
use t3_client::view_logic::shell::ui_state::*;
fn parse(s: &str) -> Option<i64> {
    s.parse().ok()
}
#[test]
fn legacy_preferences_preserve_collapsed_defaults_and_aliases() {
    let state = UiState::from_value(
        json!({"expandedProjectCwds":["/repo/a"],"projectOrderCwds":["/repo/b","/repo/a"],"sidebarProjectScopeKey":"repo","pullRequestMergeMethod":"fast-forward","future":{"x":1}}),
        &parse,
    );
    assert_eq!(
        state.project_order,
        vec![legacy_cwd_key("/repo/b"), legacy_cwd_key("/repo/a")]
    );
    assert!(resolve_project_expanded(
        &state.project_expanded_by_id,
        &[legacy_cwd_key("/repo/a")]
    ));
    assert!(!resolve_project_expanded(
        &state.project_expanded_by_id,
        &[legacy_cwd_key("/repo/b")]
    ));
    assert_eq!(state.sidebar_project_scope_keys, vec!["repo"]);
    let raw = state.encode().unwrap();
    assert_eq!(UiState::decode(&raw, &parse).unwrap(), state);
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["sidebarProjectScopeKey"], "repo");
    assert_eq!(v["future"], json!({"x":1}));
    assert_eq!(v["pullRequestMergeMethod"], "fast-forward");
    let state = UiState::from_value(
        json!({"collapsedProjectCwds":["/repo/b"],"expandedProjectCwds":["/repo/a"]}),
        &parse,
    );
    assert!(resolve_project_expanded(
        &state.project_expanded_by_id,
        &["unknown".into()]
    ));
    assert!(!resolve_project_expanded(
        &state.project_expanded_by_id,
        &[legacy_cwd_key("/repo/b")]
    ));
}
#[test]
fn raw_preferences_sanitize_values_and_version_changed_files() {
    let mut state = UiState::from_value(
        json!({"projectExpandedById":{"logical":false,"invalid":"no"},"projectOrder":["b","","a","b"],"threadLastVisitedAtById":{"t":"10","invalid":"date"},"threadChangedFilesExpansionVersion":2,"threadChangedFilesExpandedById":{"t":{"a":false,"b":true,"bad":0},"empty":{},"":{}},"sidebarProjectScopeKeys":["a","a","","b"],"defaultAdvertisedEndpointKey":"desktop:lan"}),
        &parse,
    );
    assert_eq!(state.project_order, vec!["b", "a"]);
    assert_eq!(state.project_expanded_by_id.len(), 1);
    assert_eq!(state.thread_last_visited_at_by_id.len(), 1);
    assert_eq!(state.thread_changed_files_expanded_by_id.len(), 1);
    assert_eq!(state.thread_changed_files_expanded_by_id["t"].len(), 2);
    assert_eq!(state.sidebar_project_scope_keys, vec!["a", "b"]);
    assert_eq!(
        state.default_advertised_endpoint_key.as_deref(),
        Some("desktop:lan")
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&state.encode().unwrap()).unwrap()["sidebarProjectScopeKey"],
        json!(null)
    );
    state.set_scope(&["new".into(), "new".into(), "".into()]);
    assert_eq!(state.sidebar_project_scope_keys, vec!["new"]);
    state.set_default_endpoint(Some(""));
    assert_eq!(state.default_advertised_endpoint_key, None);
    for version in [json!(null), json!(1)] {
        assert!(UiState::from_value(json!({"threadChangedFilesExpansionVersion":version,"threadChangedFilesExpandedById":{"t":{"a":true}}}),&parse).thread_changed_files_expanded_by_id.is_empty());
    }
}
#[test]
fn visits_only_advance_but_mark_unread_rewinds_from_completion() {
    let mut s = UiState::default();
    assert!(s.mark_visited("e:t", "10", &parse));
    assert!(!s.mark_visited("e:t", "9", &parse));
    assert!(!s.mark_visited("e:t", "bad", &parse));
    assert!(s.mark_unread("e:t", Some("5"), &parse, |v| Some(v.to_string())));
    assert_eq!(s.thread_last_visited_at_by_id["e:t"], "4");
    assert!(!s.mark_unread("e:t", None, &parse, |v| Some(v.to_string())));
    assert!(s.mark_visited("other:t", "2", &parse));
    assert_eq!(s.thread_last_visited_at_by_id.len(), 2);
}
#[test]
fn grouped_reorder_expansion_and_changed_files() {
    let mut s = UiState::default();
    let strings = |a: &[&str]| a.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
    let keys = strings(&["logical", "e1:/repo", "e2:/repo"]);
    s.set_project_expanded(&keys, false);
    assert_eq!(s.project_expanded_by_id.len(), 3);
    assert!(!resolve_project_expanded(&s.project_expanded_by_id, &keys));
    s.reorder_projects(
        &strings(&["a-local", "a-remote", "b", "c"]),
        &strings(&["a-local", "a-remote"]),
        &strings(&["c"]),
    );
    assert_eq!(s.project_order, strings(&["b", "c", "a-local", "a-remote"]));
    let before = s.project_order.clone();
    s.reorder_projects(&before, &strings(&["gone"]), &strings(&["b"]));
    assert_eq!(s.project_order, before);
    s.set_changed_files_expanded("e:t", "r", false);
    assert!(!s.thread_changed_files_expanded_by_id["e:t"]["r"]);
}

#[test]
fn preferences_round_trip_and_fall_back_to_legacy_records_without_deleting_them() {
    use futures::executor::block_on;
    use t3_client::platform::{Storage, StorageArea, StorageKey, StorageWrite};
    let storage = t3_client::testing::FakePlatform::default();
    let key = |record: &str| StorageKey {
        namespace: PREFERENCES_NAMESPACE.into(),
        environment: None,
        record: record.into(),
    };
    block_on(async {
        assert_eq!(
            UiPreferences::load(&storage).await.unwrap(),
            UiPreferences::default()
        );
        storage
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: key("t3code:renderer-state:v8"),
                    value: Some(br#"{"projectOrder":["a"]}"#.to_vec()),
                }],
            )
            .await
            .unwrap();
        let mut preferences = UiPreferences::load(&storage).await.unwrap();
        assert_eq!(preferences.state.project_order, vec!["a".to_owned()]);
        preferences.working_shelf_expanded = true;
        preferences.sidebar_width = Some(320.5);
        let expected_state = preferences.state.clone();
        UiPreferences::update_state(&storage, |state| *state = expected_state.clone())
            .await
            .unwrap();
        storage
            .write(
                StorageArea::Ordinary,
                vec![
                    StorageWrite {
                        key: key(WORKING_SHELF_EXPANDED_KEY),
                        value: Some(
                            serde_json::to_vec(&preferences.working_shelf_expanded).unwrap(),
                        ),
                    },
                    StorageWrite {
                        key: key(SIDEBAR_WIDTH_KEY),
                        value: Some(serde_json::to_vec(&320.5f64).unwrap()),
                    },
                ],
            )
            .await
            .unwrap();
        let loaded = UiPreferences::load(&storage).await.unwrap();
        assert_eq!(loaded, preferences);
        assert!(
            storage
                .get(StorageArea::Ordinary, key("t3code:renderer-state:v8"))
                .await
                .unwrap()
                .is_some(),
            "the legacy record stays for older clients"
        );
        storage
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: key(STORAGE_KEY),
                    value: Some(b"{not json".to_vec()),
                }],
            )
            .await
            .unwrap();
        assert_eq!(
            UiPreferences::load(&storage).await.unwrap().state,
            UiState::default(),
            "an unreadable record reads as defaults"
        );
    });
}
