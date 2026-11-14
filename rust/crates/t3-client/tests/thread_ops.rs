#![allow(clippy::unwrap_used)]

mod support { pub mod fake_server; }
use support::fake_server::*;
use t3_client::{ClientStore, ClientError, composition::ClientRuntime, connection::PairingInput,
    thread_views::thread_ops::approvals::PendingApprovals, types::ThreadKey};
use t3_contracts::{orchestration_v2::OrchestrationV2CreationSource,
    orchestration_v2_id_table::IdOrIndex, base_schemas::RuntimeRequestId,
    provider_policy::ProviderApprovalDecision};
use serde_json::json;
use std::rc::Rc;

fn key() -> ThreadKey { ThreadKey { environment: environment("env"), thread: thread("t1") } }
fn paired(h: &mut Harness) -> ClientRuntime<TestPlatform> {
    let c = ClientRuntime::new(h.platform.clone(), ClientStore::default(), OrchestrationV2CreationSource::Web, h.spawner());
    h.run(c.pair(PairingInput { pairing_url: Some(format!("{ORIGIN}/pair#token=pairing-token")), ..Default::default() })).unwrap();
    c.open_thread(&key()); h.settle(); c
}
fn request(status: &str) -> t3_contracts::orchestration_v2::OrchestrationV2RuntimeRequest {
    serde_json::from_value(json!({"id":"approval","nodeId":"n","providerTurnId":null,"nativeRequestRef":null,
        "kind":"command","status":status,"responseCapability":{"type":"live","providerSessionId":"s"},
        "createdAt":NOW,"resolvedAt":null})).unwrap()
}

#[test]
fn pending_projection_keeps_requests_without_cards_and_ignores_internal_requests() {
    let mut source: t3_contracts::orchestration_v2::OrchestrationV2ThreadProjection = serde_json::from_value(thread_snapshot("t1")["projection"].clone()).unwrap();
    source.runtime_requests = vec![request("pending"), { let mut r = request("pending"); r.id = IdOrIndex::Id(RuntimeRequestId("internal".into())); r.kind = serde_json::from_value(json!("auth_refresh")).unwrap(); r }];
    let p = t3_client::state::sharing::SharedThreadProjection::update(Rc::new(source), None);
    let mut selector = PendingApprovals::default();
    let first = selector.project(&p);
    assert_eq!(first.len(), 1);
    assert!(first.values().next().unwrap().live);
    assert!(Rc::ptr_eq(&first, &selector.project(&p)));
}

#[test]
fn resolved_or_absent_approval_is_refused_without_dispatch() {
    let mut h = Harness::new("env"); let c = paired(&mut h);
    let error = h.run(c.approval_services().respond(key(), IdOrIndex::Id(RuntimeRequestId("old".into())), ProviderApprovalDecision::Accept)).unwrap_err();
    assert!(matches!(error, ClientError::Rejected { .. }));
}

#[test]
fn workspace_save_preserves_server_failure_and_denied_permission() {
    let mut h = Harness::new("env"); let c = paired(&mut h);
    c.client(&environment("env")).unwrap().session().unwrap().update_grants(t3_client::permissions::SessionGrants { scopes: Default::default(), valid: true });
    let plans = c.plan_services();
    assert!(!plans.can_write(&key()));
    let error = h.run(plans.write_workspace_file(key(), "/workspace/project".into(), "plan.md".into(), "# Plan\n".into())).unwrap_err();
    assert!(matches!(error, ClientError::PermissionDenied { .. }));
}

fn script(h: &Harness, tag: &str, result: Result<serde_json::Value, serde_json::Value>) {
    h.server().state.borrow_mut().rpc_script.entry(tag.into()).or_default().push_back(result);
}
fn seed_approval(h: &Harness) {
    let mut snapshot = thread_snapshot("t1");
    snapshot["projection"]["runtimeRequests"] = json!([request("pending")]);
    h.server().state.borrow_mut().thread_snapshots.insert("t1".into(), snapshot);
}
#[test]
fn approval_success_does_not_resolve_locally_and_failure_keeps_server_detail() {
    let mut h = Harness::new("env"); seed_approval(&h); let c = paired(&mut h);
    let service = c.approval_services();
    let id = IdOrIndex::Id(RuntimeRequestId("approval".into()));
    script(&h, "orchestration.dispatchCommand", Ok(json!({"sequence":2})));
    h.run(service.respond(key(), id.clone(), ProviderApprovalDecision::Accept)).unwrap();
    assert_eq!(service.pending_approvals(&key()).len(), 1);
    script(&h, "orchestration.dispatchCommand", Err(json!({"_tag":"OrchestrationV2DispatchCommandError","commandId":"c","commandType":"runtime-request.respond","message":"Could not respond","detail":"provider closed the callback"})));
    let error = h.run(service.respond(key(), id.clone(), ProviderApprovalDecision::Decline)).unwrap_err();
    assert!(error.to_string().contains("provider closed the callback"));
    assert_eq!(service.pending_approvals(&key()).len(), 1);
    let endpoint = c.client(&environment("env")).unwrap().session().unwrap();
    endpoint.update_grants(t3_client::permissions::SessionGrants { scopes: Default::default(), valid: true });
    assert!(!service.can_operate(&key()));
    assert!(matches!(h.run(service.respond(key(), id, ProviderApprovalDecision::Decline)).unwrap_err(), ClientError::PermissionDenied { .. }));
    let calls = h.server().state.borrow();
    assert_eq!(calls.rpc_calls.iter().filter(|(tag,_)| tag == "orchestration.dispatchCommand").count(), 2);
}
#[test]
fn workspace_save_reports_confirmed_path_and_typed_failure_and_download_is_explicit() {
    let mut h = Harness::new("env"); let c = paired(&mut h); let plans = c.plan_services();
    script(&h, "projects.writeFile", Ok(json!({"relativePath":"plans/confirmed.md"})));
    assert_eq!(h.run(plans.write_workspace_file(key(), "/workspace/project".into(), "plan.md".into(), "# Plan\n".into())).unwrap(), Some("plans/confirmed.md".into()));
    assert!(!plans.can_download());
    assert!(h.run(plans.download_text("plan.md".into(), "# Plan".into(), "text/markdown;charset=utf-8".into())).is_err());
}
