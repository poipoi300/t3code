#![allow(clippy::unwrap_used)]
use serde_json::json;
use t3_client::{types::ThreadId, view_logic::shell::preview_state::*};
use t3_contracts::preview::*;

fn snapshot(id: &str, second: u8) -> PreviewSessionSnapshot {
    serde_json::from_value(json!({"threadId":"t", "tabId":id,
        "navStatus":{"_tag":"Loading","url":"http://localhost:5173/","title":""},
        "canGoBack":false,"canGoForward":false,"updatedAt":format!("2026-01-01T00:00:{second:02}.000Z")})).unwrap()
}
fn event(kind: &str, id: &str, revision: u64) -> PreviewEvent {
    serde_json::from_value(json!({"type":kind,"threadId":"t","tabId":id,
        "createdAt":"2026-01-01T00:01:00.000Z","serverEpoch":"a","revision":revision,
        "snapshot": snapshot(id,revision as u8)}))
    .unwrap()
}
fn list(epoch: &str, revision: u64, sessions: Vec<PreviewSessionSnapshot>) -> PreviewListResult {
    PreviewListResult {
        server_epoch: epoch.into(),
        revision,
        sessions,
    }
}
#[test]
fn resized_background_and_unknown_failures_preserve_focus() {
    let mut state = ThreadPreview::<String>::default();
    state.apply_snapshot(Some(snapshot("b", 0)));
    state.apply_snapshot(Some(snapshot("a", 0)));
    state.apply_event(event("resized", "b", 1));
    assert_eq!(state.active_tab.as_deref(), Some("a"));
    assert_eq!(state.sessions["b"].updated_at, snapshot("b", 1).updated_at);
    let failure = serde_json::from_value(json!({"type":"failed","threadId":"t","tabId":"missing",
        "createdAt":"later","serverEpoch":"a","revision":2,"url":"http://localhost:9999/",
        "title":"","code":-105,"description":"ERR_NAME_NOT_RESOLVED"}))
    .unwrap();
    state.apply_event(failure);
    assert!(matches!(
        state.snapshot().unwrap().nav_status,
        PreviewNavStatus::Loading { .. }
    ));
    assert_eq!(state.sessions.len(), 2);
}
#[test]
fn optimistic_close_suppresses_snapshots_lists_and_old_events_until_settled() {
    let mut state = ThreadPreview::<String>::default();
    state.reconcile(list("a", 1, vec![snapshot("a", 0), snapshot("b", 1)]));
    assert_eq!(state.active_tab.as_deref(), Some("b"));
    state.begin_close("b");
    assert_eq!(state.active_tab.as_deref(), Some("a"));
    assert!(!state.apply_snapshot(Some(snapshot("b", 2))));
    state.reconcile(list("a", 2, vec![snapshot("a", 0), snapshot("b", 1)]));
    assert!(!state.sessions.contains_key("b"));
    state.apply_event(event("closed", "b", 4));
    assert!(!state.suppressed.contains("b"));
    assert!(!state.apply_event(event("opened", "b", 3)));
    assert!(!state.sessions.contains_key("b"));
}
#[test]
fn failed_close_restores_suppressed_tab_and_overlay_is_tab_scoped() {
    let mut state = ThreadPreview::<String>::default();
    let a = snapshot("a", 0);
    state.apply_snapshot(Some(a.clone()));
    assert!(state.set_desktop("a", Some("overlay-a".into())));
    assert!(!state.set_desktop("a", Some("overlay-a".into())));
    state.apply_snapshot(Some(snapshot("b", 1)));
    state.set_desktop("b", Some("overlay-b".into()));
    assert_eq!(
        state.desktop_overlay().map(String::as_str),
        Some("overlay-b")
    );
    assert!(state.set_active("a"));
    assert_eq!(
        state.desktop_overlay().map(String::as_str),
        Some("overlay-a")
    );
    state.begin_close("a");
    assert!(state.cancel_close("a", Some(a.clone())));
    assert_eq!(state.snapshot(), Some(&a));
    assert!(!state.cancel_close("a", Some(a)));
    assert!(!state.set_active("missing"));
}
#[test]
fn authoritative_lists_preserve_focus_and_newer_streamed_snapshots() {
    let mut state = ThreadPreview::<String>::default();
    state.apply_event(event("opened", "a", 3));
    state.apply_event(event("opened", "b", 4));
    state.set_active("a");
    let sessions = vec![snapshot("a", 0), snapshot("b", 0), snapshot("c", 0)];
    state.reconcile(list("a", 5, sessions.clone()));
    assert_eq!(state.active_tab.as_deref(), Some("a"));
    assert_eq!(state.sessions["a"].updated_at, snapshot("a", 3).updated_at);
    assert!(!state.reconcile(list("a", 5, sessions)));
    assert!(!state.reconcile(list("a", 2, vec![])));
    assert_eq!(state.sessions.len(), 3);
    assert!(!state.apply_snapshot(Some(snapshot("a", 1))));
    state.reconcile(list("a", 6, vec![snapshot("b", 0), snapshot("c", 6)]));
    assert_eq!(state.active_tab.as_deref(), Some("c"));
}
#[test]
fn restart_accepts_lower_revision_and_discards_old_raw_tab_state() {
    let mut state = ThreadPreview::<String>::default();
    state.apply_event(event("opened", "a", 20));
    state.set_desktop("a", Some("raw".into()));
    state.begin_close("b");
    assert!(!state.apply_event(serde_json::from_value(json!({"type":"closed","threadId":"t","tabId":"a","createdAt":"now","serverEpoch":"b","revision":1})).unwrap()));
    state.reconcile(list("b", 1, vec![snapshot("a", 0), snapshot("b", 1)]));
    assert!(state.desktop.is_empty());
    assert!(state.suppressed.is_empty());
    assert_eq!(state.server_revision, 1);
    assert_eq!(state.sessions["a"].updated_at, snapshot("a", 0).updated_at);
}
#[test]
fn recent_urls_and_wait_decisions_are_bounded_and_clock_free() {
    let mut state = ThreadPreview::<String>::default();
    for i in 0..12 {
        state.remember_url(&format!("http://localhost/{i}"));
    }
    state.remember_url("http://localhost/5");
    state.remember_url("  ");
    assert_eq!(state.recent_urls.len(), 10);
    assert_eq!(state.recent_urls[0], "http://localhost/5");
    assert_eq!(tab_wait(&state, "a", 100, 99), TabWait::Waiting);
    assert_eq!(tab_wait(&state, "a", 100, 100), TabWait::Expired);
    state.apply_snapshot(Some(snapshot("a", 0)));
    assert_eq!(tab_wait(&state, "a", 100, 200), TabWait::Ready);
    assert!(state.apply_snapshot(None));
    assert!(!state.apply_snapshot(None));
}
#[test]
fn configured_urls_are_loopback_canonical_bounded_and_fragment_deduplicated() {
    let urls = [
        "http://localhost:3000#one",
        "http://localhost:3000/#two",
        "https://127.0.0.1/a",
        "http://[::1]:8000/",
        "http://0.0.0.0/",
        "https://example.com/",
        "ftp://localhost/",
        " http://localhost/",
        "http://localhost/ ",
        "not a url",
    ]
    .map(str::to_owned);
    assert_eq!(
        bound_configured_urls(&urls),
        vec![
            "http://localhost:3000/#one",
            "https://127.0.0.1/a",
            "http://[::1]:8000/",
            "http://0.0.0.0/"
        ]
    );
    let urls = (0..40)
        .map(|i| format!("http://localhost/{i}"))
        .collect::<Vec<_>>();
    assert_eq!(
        bound_configured_urls(&urls).len(),
        CONFIGURED_LOCAL_SERVER_URLS_MAX_ITEMS
    );
}
#[test]
fn discovered_ports_match_terminal_or_agent_owner_and_require_a_thread() {
    let servers:Vec<DiscoveredLocalServer>=serde_json::from_value(json!([
        {"host":"localhost","processName":null,"pid":null,"url":"http://localhost:3000/","port":3000,"terminal":{"threadId":"t","terminalId":"term"}},
        {"host":"localhost","processName":null,"pid":null,"url":"http://localhost:3001/","port":3001,"terminal":null,"agent":{"threadId":"t"}},
        {"host":"localhost","processName":null,"pid":null,"url":"http://localhost:3002/","port":3002,"terminal":{"threadId":"other","terminalId":"term"}}
    ])).unwrap();
    assert_eq!(
        thread_discovered_ports(&servers, Some(&ThreadId("t".into()))).len(),
        2
    );
    assert!(thread_discovered_ports(&servers, None).is_empty());
}

#[test]
fn preview_recents_record_explicit_addresses_without_recording_snapshots() {
    let mut state = ThreadPreview::<()>::default();
    state.remember_url("https://user.example/");
    state.apply_snapshot(Some(snapshot("explicit", 0)));
    state.apply_event(event("opened", "agent", 1));
    state.apply_event(event("navigated", "explicit", 2));
    state.update_snapshot(snapshot("explicit", 3));
    state.reconcile(list("a", 4, vec![snapshot("restored", 4)]));
    state.begin_close("restored");
    state.cancel_close("restored", Some(snapshot("restored", 4)));
    assert_eq!(state.recent_urls, ["https://user.example/"]);
}
