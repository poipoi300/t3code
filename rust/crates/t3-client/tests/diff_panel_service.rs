#![allow(clippy::unwrap_used)]
//! The diff panel's client service against the fake server: DiffPanel.tsx's turn diff, its
//! typed failure, test-file hiding, folds, and review comments into the composer draft.

mod support {
    pub mod fake_server;
}
use futures::{StreamExt, stream::LocalBoxStream};
use serde_json::json;
use std::rc::Rc;
use support::fake_server::*;
use t3_client::{
    ClientStore,
    composition::ClientRuntime,
    connection::PairingInput,
    types::ThreadKey,
    workspace::diff_panel::{
        DiffPanelSnapshot,
        comments::Side,
        logic::{DiffBody, DiffLabel},
    },
};
use t3_contracts::{base_schemas::RunId, orchestration_v2::OrchestrationV2CreationSource};

const PATCH: &str = "diff --git a/src/a.ts b/src/a.ts\n--- a/src/a.ts\n+++ b/src/a.ts\n@@ -10,3 +10,3 @@\n keep\n-old\n+new\n tail\ndiff --git a/src/a.test.ts b/src/a.test.ts\n--- a/src/a.test.ts\n+++ b/src/a.test.ts\n@@ -1 +1 @@\n-x\n+y\n";

fn key() -> ThreadKey {
    ThreadKey {
        environment: environment("env"),
        thread: thread("t1"),
    }
}

fn checkpoint(id: &str, run: &str, ordinal: u64) -> serde_json::Value {
    json!({"id":id,"threadId":"t1","scopeId":"scope","runId":run,"nodeId":"node",
        "parentCheckpointId":null,"ordinalWithinScope":ordinal,"appRunOrdinal":ordinal,
        "ref":format!("refs/t3/{id}"),"status":"ready","files":[],"capturedAt":NOW})
}

fn script(h: &Harness, tag: &str, result: Result<serde_json::Value, serde_json::Value>) {
    h.server()
        .state
        .borrow_mut()
        .rpc_script
        .entry(tag.into())
        .or_default()
        .push_back(result);
}

fn paired(h: &mut Harness) -> ClientRuntime<TestPlatform> {
    let mut snapshot = thread_snapshot("t1");
    snapshot["projection"]["checkpoints"] =
        json!([checkpoint("c1", "r1", 1), checkpoint("c2", "r2", 2)]);
    h.server()
        .state
        .borrow_mut()
        .thread_snapshots
        .insert("t1".into(), snapshot);
    let c = ClientRuntime::new(
        h.platform.clone(),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        h.spawner(),
    );
    h.run(c.pair(PairingInput {
        pairing_url: Some(format!("{ORIGIN}/pair#token=pairing-token")),
        ..Default::default()
    }))
    .unwrap();
    c.open_thread(&key());
    h.settle();
    c
}

/// The next snapshot that satisfies `done`, settling the harness between answers.
fn until(
    h: &mut Harness,
    mut stream: LocalBoxStream<'static, Rc<DiffPanelSnapshot>>,
    done: impl Fn(&DiffPanelSnapshot) -> bool + 'static,
) -> (
    Rc<DiffPanelSnapshot>,
    LocalBoxStream<'static, Rc<DiffPanelSnapshot>>,
) {
    h.settle();
    h.run(async move {
        loop {
            let next = stream.next().await.expect("the watch keeps answering");
            if done(&next) {
                return (next, stream);
            }
        }
    })
}

#[test]
fn a_turn_reads_its_turn_diff_and_lists_its_files_and_totals() {
    let mut h = Harness::new("env");
    let c = paired(&mut h);
    script(
        &h,
        "orchestration.getTurnDiff",
        Ok(json!({"fromTurnCount":1,"toTurnCount":2,"threadId":"t1","diff":PATCH})),
    );
    let service = c.diff_panel();
    h.run(service.select_turn(&key(), RunId("r2".into()), None))
        .unwrap();
    let (snapshot, _watch) = until(&mut h, service.watch(key()), |s| {
        matches!(s.view.body, DiffBody::Files(_))
    });
    assert_eq!(snapshot.resolved.scope_label, DiffLabel::LatestTurn);
    assert_eq!(snapshot.turns.len(), 2);
    let paths: Vec<_> = snapshot
        .shown_files
        .iter()
        .map(|(_, p)| p.as_str())
        .collect();
    assert_eq!(paths, vec!["src/a.ts", "src/a.test.ts"]);
    let stat = snapshot.view.stat.unwrap();
    assert_eq!((stat.additions, stat.deletions), (2, 2));
    assert_eq!(snapshot.test_file_count, 1);
    let calls = h.server().state.borrow().rpc_calls.clone();
    let call = calls
        .iter()
        .find(|(tag, _)| tag == "orchestration.getTurnDiff")
        .expect("the turn diff was read");
    assert_eq!(call.1["fromTurnCount"], 1);
    assert_eq!(call.1["toTurnCount"], 2);
}

#[test]
fn the_first_turn_reads_the_full_thread_diff_and_shows_its_typed_failure() {
    let mut h = Harness::new("env");
    h.server()
        .state
        .borrow_mut()
        .extra_methods
        .push(("orchestration.getFullThreadDiff", t3_rpc::CallKind::Unary));
    let c = paired(&mut h);
    script(
        &h,
        "orchestration.getFullThreadDiff",
        Err(
            json!({"_tag":"OrchestrationGetFullThreadDiffError","message":"Checkpoint is missing."}),
        ),
    );
    let service = c.diff_panel();
    h.run(service.select_turn(&key(), RunId("r1".into()), None))
        .unwrap();
    let (snapshot, _watch) = until(&mut h, service.watch(key()), |s| s.view.error.is_some());
    assert_eq!(snapshot.resolved.scope_label, DiffLabel::Turn(Some(1)));
    assert_eq!(
        snapshot.view.body,
        DiffBody::Empty {
            no_net_changes: false
        }
    );
    let calls = h.server().state.borrow().rpc_calls.clone();
    assert!(
        calls
            .iter()
            .any(|(tag, input)| tag == "orchestration.getFullThreadDiff"
                && input["toTurnCount"] == 1),
        "error {:?}, calls {:?}",
        snapshot.view.error,
        calls.iter().map(|(tag, _)| tag.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn hiding_test_files_folds_and_review_comments_follow_the_client_state() {
    let mut h = Harness::new("env");
    let c = paired(&mut h);
    script(
        &h,
        "orchestration.getTurnDiff",
        Ok(json!({"fromTurnCount":1,"toTurnCount":2,"threadId":"t1","diff":PATCH})),
    );
    let service = c.diff_panel();
    h.run(service.select_turn(&key(), RunId("r2".into()), None))
        .unwrap();
    let (snapshot, watch) = until(&mut h, service.watch(key()), |s| {
        matches!(s.view.body, DiffBody::Files(_))
    });

    h.run(service.set_hide_test_files(true)).unwrap();
    let (hidden, watch) = until(&mut h, watch, |s| s.hide_test_files);
    let paths: Vec<_> = hidden.shown_files.iter().map(|(_, p)| p.clone()).collect();
    assert_eq!(paths, vec!["src/a.ts".to_owned()]);
    assert!(!hidden.shown_patch.as_deref().unwrap().contains("a.test.ts"));

    // `diffFilesCollapsed` defaults to true: every file starts folded.
    let first = snapshot.shown_files[0].0.clone();
    assert!(hidden.collapsed.contains(&first));
    service.toggle_file(&hidden, &first);
    let unfolded_key = first.clone();
    let (unfolded, watch) = until(&mut h, watch, move |s| !s.collapsed.contains(&unfolded_key));
    // One shown file is open, so "Collapse all files" folds every shown file.
    service.toggle_all_files(&unfolded);
    let folded_key = first.clone();
    let (folded, watch) = until(&mut h, watch, move |s| s.collapsed.contains(&folded_key));
    assert!(folded.collapsed.contains(&first));

    // The new side's second excerpt line is src/a.ts line 11, the added line.
    service
        .add_review_comment(&key(), &folded, Side::New, (1, 1), "Why this?")
        .unwrap();
    let (commented, watch) = until(&mut h, watch, |s| !s.annotations.is_empty());
    let annotation = &commented.annotations[0];
    assert_eq!(annotation.text, "Why this?");
    assert_eq!(annotation.range_label, "+11");
    assert_eq!((annotation.side, annotation.line), (Side::New, 1));
    let id = annotation.id.clone();
    service.remove_review_comment(&key(), &id).unwrap();
    let (cleared, _watch) = until(&mut h, watch, |s| s.annotations.is_empty());
    assert!(cleared.annotations.is_empty());
}

#[test]
fn a_comment_across_two_files_is_refused() {
    let mut h = Harness::new("env");
    let c = paired(&mut h);
    script(
        &h,
        "orchestration.getTurnDiff",
        Ok(json!({"fromTurnCount":1,"toTurnCount":2,"threadId":"t1","diff":PATCH})),
    );
    let service = c.diff_panel();
    h.run(service.select_turn(&key(), RunId("r2".into()), None))
        .unwrap();
    let (snapshot, _watch) = until(&mut h, service.watch(key()), |s| {
        matches!(s.view.body, DiffBody::Files(_))
    });
    assert!(
        service
            .draft_comment(&snapshot, Side::New, (2, 3))
            .is_none()
    );
    assert!(
        service
            .add_review_comment(&key(), &snapshot, Side::New, (2, 3), "x")
            .is_err()
    );
}

#[test]
fn an_unserved_diff_method_shows_its_unsupported_reason_not_an_empty_success() {
    let mut h = Harness::new("env");
    let c = paired(&mut h);
    let service = c.diff_panel();
    h.run(service.select_turn(&key(), RunId("r1".into()), None))
        .unwrap();
    let (snapshot, _watch) = until(&mut h, service.watch(key()), |s| s.view.error.is_some());
    assert!(
        snapshot
            .view
            .error
            .as_deref()
            .unwrap()
            .contains("does not serve")
    );
    assert_eq!(
        snapshot.view.body,
        DiffBody::Empty {
            no_net_changes: false
        }
    );
}
