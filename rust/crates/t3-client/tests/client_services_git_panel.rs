#![allow(clippy::unwrap_used)]
mod support { pub mod fake_server; }
use support::fake_server::*;
use futures::{FutureExt, StreamExt};
use serde_json::json;
use std::{cell::RefCell, rc::Rc};
use t3_client::{ClientError, ClientStore, composition::ClientRuntime, connection::PairingInput, permissions::SessionGrants, types::ThreadKey, workspace::git_panel::{actions::*, providers}};
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
use t3_rpc::{Exit, HandlerOutput};

fn setup(h: &mut Harness) -> ClientRuntime<TestPlatform> {
    let runtime = ClientRuntime::new(h.platform.clone(), ClientStore::default(), OrchestrationV2CreationSource::Web, h.spawner());
    h.run(runtime.pair(PairingInput { pairing_url: Some(format!("{ORIGIN}/pair#token=pairing-token")), ..PairingInput::default() })).unwrap();
    h.settle(); runtime
}
fn scope() -> GitActionsScope { GitActionsScope { thread: ThreadKey { environment: environment("env"), thread: thread("t1") }, cwd: "/repo".into(), draft_id: None } }

#[test]
fn commands_preserve_typed_failure_and_recheck_live_grants() {
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    let calls = Rc::new(RefCell::new(Vec::new()));
    let recorded = calls.clone();
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(move |request| {
        if request.tag != "vcs.pull" { return None; }
        recorded.borrow_mut().push(request.payload.clone());
        Some(HandlerOutput::Unary(async { Exit::fail(json!({"_tag":"VcsError","message":"upstream refused","operation":"pull","detail":"no remote"})) }.boxed_local()))
    }));
    let service = runtime.git_panel();
    let error = h.run(service.pull(&scope())).unwrap_err();
    assert!(matches!(error, ClientError::Rejected { .. }));
    assert!(error.to_string().contains("upstream refused"));
    let future = service.pull(&scope());
    runtime.client(&environment("env")).unwrap().session().unwrap().update_grants(SessionGrants { valid: true, scopes: Default::default() });
    assert!(matches!(h.run(future), Err(ClientError::PermissionDenied { .. })));
    assert_eq!(calls.borrow().len(), 1);
}

#[test]
fn status_watch_emits_current_then_updates_and_releases_the_server_stream() {
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    let (send, receive) = futures::channel::mpsc::unbounded();
    let receive = Rc::new(RefCell::new(Some(receive)));
    let open = Rc::new(std::cell::Cell::new(0));
    let observed = open.clone();
    struct Guard(Rc<std::cell::Cell<usize>>);
    impl Drop for Guard { fn drop(&mut self) { self.0.set(self.0.get() - 1); } }
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(move |request| match request.tag.as_str() {
        "vcs.refreshStatus" => Some(HandlerOutput::Unary(async { Exit::success(json!({})) }.boxed_local())),
        "subscribeVcsStatus" => {
            observed.set(observed.get()+1);
            let guard = Guard(observed.clone());
            let stream = receive.borrow_mut().take().unwrap().map(move |value| { let _ = &guard; Ok(vec![value]) }).boxed_local();
            Some(HandlerOutput::Stream(stream))
        },
        _ => None,
    }));
    let mut watch = runtime.git_panel().watch_actions(&scope(), GitActionsWatch { show_actions: false, show_changes: false });
    assert!(futures::executor::block_on(watch.next()).unwrap().status.is_none());
    h.settle();
    assert_eq!(open.get(), 1);
    let mut status: serde_json::Value = serde_json::from_str(include_str!("../../../fixtures/samples/VcsStatusResult.jsonl").lines().next().unwrap()).unwrap();
    status["refName"] = json!("feature");
    send.unbounded_send(json!({"type":"snapshot","status":status})).unwrap();
    h.settle();
    let latest = futures::executor::block_on(watch.next()).unwrap();
    assert_eq!(latest.status.unwrap().ref_name.unwrap().0, "feature");
    drop(watch); h.settle(); assert_eq!(open.get(), 0);
}

#[test]
fn unknown_hosts_keep_generic_change_request_terms_and_gitcafe_has_its_mark() {
    assert_eq!(providers::presentation(None).short_label, "PR");
    assert_eq!(providers::presentation(Some("gitlab")).singular, "merge request");
    let unknown = providers::presentation(Some("new-forge"));
    assert_eq!(unknown.short_label, "change request"); assert!(unknown.icon_svg.is_none());
    assert!(providers::publish_providers().iter().find(|p| p.kind == "gitcafe").unwrap().icon_svg.contains("#a78bfa"));
}
