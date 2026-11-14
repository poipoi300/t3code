#![allow(clippy::unwrap_used)]
use serde_json::json;
use t3_client::{
    types::{EnvironmentId, SyncStatus, ThreadId, ThreadKey},
    view_logic::shell::{navigation::*, terminal_state::*},
};
fn key(e: &str, t: &str) -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId(e.into()),
        thread: ThreadId(t.into()),
    }
}
#[test]
fn encoded_routes_and_draft_promotions() {
    let k = key("e:/é", "t#?% +");
    let path = format!(
        "/{}/{}",
        encode_component(&k.environment.0),
        encode_component(&k.thread.0)
    );
    assert!(is_thread_path(&path, &k));
    assert!(is_thread_path(&format!("{path}/"), &k));
    for p in ["/e/t/x", "/e/%FF", "/e/%2", "/e/%zz"] {
        assert!(!is_thread_path(p, &k));
    }
    let target = route_target(Some("e"), Some("t"), Some("draft")).unwrap();
    assert_eq!(
        active_thread(Some(&target), None, None),
        Some(&key("e", "t"))
    );
    let draft = route_target(None, None, Some("draft")).unwrap();
    assert_eq!(active_thread(Some(&draft), None, Some(&k)), Some(&k));
    assert_eq!(active_thread(None, None, Some(&k)), None);
    assert!(snapshot_authoritative(Some(SyncStatus::Live)));
    assert!(!snapshot_authoritative(Some(SyncStatus::Cached)));
}
#[test]
fn route_render_waits_for_detail_and_only_treats_live_shell_as_authoritative() {
    for (bootstrap, exists, deleted, status, detail, draft, expected) in [
        (
            false,
            false,
            false,
            SyncStatus::Empty,
            None,
            false,
            RenderState::Loading,
        ),
        (
            true,
            false,
            false,
            SyncStatus::Empty,
            None,
            false,
            RenderState::Loading,
        ),
        (
            true,
            false,
            false,
            SyncStatus::Live,
            Some(false),
            false,
            RenderState::Ready,
        ),
        (
            true,
            false,
            false,
            SyncStatus::Live,
            Some(true),
            false,
            RenderState::Missing,
        ),
        (
            true,
            false,
            false,
            SyncStatus::Deleted,
            None,
            false,
            RenderState::Missing,
        ),
        (
            true,
            true,
            true,
            SyncStatus::Empty,
            None,
            false,
            RenderState::Missing,
        ),
        (
            false,
            true,
            false,
            SyncStatus::Empty,
            None,
            false,
            RenderState::Ready,
        ),
        (
            false,
            false,
            false,
            SyncStatus::Empty,
            None,
            true,
            RenderState::Ready,
        ),
    ] {
        assert_eq!(
            render_state(bootstrap, exists, deleted, status, detail, draft),
            expected
        );
    }
}
#[test]
fn terminal_split_cap_groups_and_active_neighbor() {
    let mut s = TerminalState::default();
    s.set_open(true);
    assert_eq!(s.active_terminal_id, "term-1");
    for id in ["b", "c", "d", "e"] {
        s.upsert(id, true, true);
    }
    assert_eq!(s.terminal_ids, vec!["term-1", "b", "c", "d"]);
    assert_eq!(
        s.terminal_groups[0].split_direction.as_deref(),
        Some("vertical")
    );
    s.close("d");
    assert_eq!(s.active_terminal_id, "c");
    s.upsert("new", false, false);
    assert_eq!(s.terminal_groups.len(), 2);
    s.activate("b");
    assert_eq!(s.active_terminal_group_id, "group-term-1");
    s.close("b");
    assert_eq!(s.active_terminal_id, "c");
    s.set_height(f64::NAN);
    assert_eq!(s.terminal_height, 280.0);
}
#[test]
fn terminal_suppression_is_environment_scoped_and_clears_on_explicit_open() {
    let a = key("a", "t");
    let b = key("b", "t");
    let mut store = TerminalStore::default();
    store.upsert(&a, "term-1", false, false);
    store.upsert(&b, "term-1", false, false);
    store.close(&a, "term-1");
    assert!(!store.by_thread.contains_key(&a));
    assert!(store.by_thread.contains_key(&b));
    store.reconcile(&a, &["term-1".into()]);
    assert!(!store.by_thread.contains_key(&a));
    store.set_open(&a, true);
    assert!(store.by_thread[&a].terminal_open);
    assert!(!store.suppressed.contains_key(&a));
    store.close(&a, "term-1");
    store.remove(&a);
    assert!(!store.suppressed.contains_key(&a));
}
#[test]
fn terminal_legacy_migration_rejects_unscoped_keys_and_roundtrips_v4() {
    let state = TerminalState {
        terminal_open: true,
        terminal_ids: vec!["one".into()],
        active_terminal_id: "one".into(),
        ..Default::default()
    };
    let raw=json!({"state":{"terminalStateByThreadKey":{"a:t":state,"bare":state,":invalid":state,"invalid:":state}},"version":1}).to_string();
    let store = TerminalStore::decode(&raw).unwrap();
    assert_eq!(store.by_thread.len(), 1);
    let encoded = store.encode().unwrap();
    let v: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(v["version"], 4);
    assert_eq!(
        TerminalStore::decode(&encoded).unwrap().by_thread,
        store.by_thread
    );
    assert!(TerminalStore::decode("null").unwrap().by_thread.is_empty());
}
#[test]
fn terminal_normalization_deduplicates_and_repairs_groups() {
    let mut state = TerminalState {
        terminal_height: -1.0,
        terminal_ids: vec![" a ".into(), "a".into(), "".into(), "b".into(), "c".into()],
        terminal_groups: vec![
            Group {
                id: "x".into(),
                terminal_ids: vec!["a".into(), "bad".into()],
                split_direction: None,
            },
            Group {
                id: "x".into(),
                terminal_ids: vec!["a".into(), "b".into()],
                split_direction: None,
            },
        ],
        ..Default::default()
    };
    state.normalize();
    assert_eq!(state.terminal_ids, vec!["a", "b", "c"]);
    assert_eq!(
        state
            .terminal_groups
            .iter()
            .map(|g| g.id.as_str())
            .collect::<Vec<_>>(),
        vec!["x", "x-2", "group-c"]
    );
    assert_eq!(state.active_terminal_id, "a");
    assert_eq!(state.terminal_height, 280.0);
}

#[test]
fn route_render_state_reads_the_shell_and_the_threads_own_detail_from_the_store() {
    let store = t3_client::ClientStore::default();
    let environment = EnvironmentId("e".into());
    let session = store.begin_session(environment.clone());
    let route = key("e", "new");
    let state = |draft| route_render_state(&store.snapshot(), &route, draft);
    assert_eq!(state(false), RenderState::Loading, "no shell yet");
    assert_eq!(state(true), RenderState::Ready, "a draft renders at once");
    store.apply_shell(
        &environment,
        session,
        vec![
            serde_json::from_value(json!({"kind":"snapshot","snapshot":{"schemaVersion":1,"snapshotSequence":1,"threads":[],"archivedThreads":[],"projects":[]}})).unwrap(),
        ],
    );
    assert_eq!(
        state(false),
        RenderState::Loading,
        "a synchronizing shell is not authoritative"
    );
    store.apply_shell(
        &environment,
        session,
        vec![serde_json::from_value(json!({"kind":"synchronized"})).unwrap()],
    );
    assert_eq!(
        state(false),
        RenderState::Loading,
        "not listed yet, no detail: wait for it"
    );
    store.update_thread(&route, session, |thread| {
        thread.status = SyncStatus::Deleted;
        true
    });
    assert_eq!(state(false), RenderState::Missing);
}
