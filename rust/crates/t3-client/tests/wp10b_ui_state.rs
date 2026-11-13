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
