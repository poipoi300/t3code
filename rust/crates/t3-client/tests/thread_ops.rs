#![allow(clippy::unwrap_used)]

mod support {
    pub mod fake_server;
}
use serde_json::json;
use std::rc::Rc;
use support::fake_server::*;
use t3_client::{
    ClientError, ClientStore, composition::ClientRuntime, connection::PairingInput,
    thread_views::thread_ops::approvals::PendingApprovals, types::ThreadKey,
};
use t3_contracts::{
    base_schemas::RuntimeRequestId, orchestration_v2::OrchestrationV2CreationSource,
    orchestration_v2_id_table::IdOrIndex, provider_policy::ProviderApprovalDecision,
};

fn key() -> ThreadKey {
    ThreadKey {
        environment: environment("env"),
        thread: thread("t1"),
    }
}
fn paired(h: &mut Harness) -> ClientRuntime<TestPlatform> {
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
fn request(status: &str) -> t3_contracts::orchestration_v2::OrchestrationV2RuntimeRequest {
    serde_json::from_value(json!({"id":"approval","nodeId":"n","providerTurnId":null,"nativeRequestRef":null,
        "kind":"command","status":status,"responseCapability":{"type":"live","providerSessionId":"s"},
        "createdAt":NOW,"resolvedAt":null})).unwrap()
}

#[test]
fn pending_projection_keeps_requests_without_cards_and_ignores_internal_requests() {
    let mut source: t3_contracts::orchestration_v2::OrchestrationV2ThreadProjection =
        serde_json::from_value(thread_snapshot("t1")["projection"].clone()).unwrap();
    source.runtime_requests = vec![request("pending"), {
        let mut r = request("pending");
        r.id = IdOrIndex::Id(RuntimeRequestId("internal".into()));
        r.kind = serde_json::from_value(json!("auth_refresh")).unwrap();
        r
    }];
    let p = t3_client::state::sharing::SharedThreadProjection::update(Rc::new(source), None);
    let mut selector = PendingApprovals::default();
    let first = selector.project(&p);
    assert_eq!(first.len(), 1);
    assert!(first.values().next().unwrap().live);
    assert!(Rc::ptr_eq(&first, &selector.project(&p)));
}

#[test]
fn resolved_or_absent_approval_is_refused_without_dispatch() {
    let mut h = Harness::new("env");
    let c = paired(&mut h);
    let error = h
        .run(c.approval_services().respond(
            key(),
            IdOrIndex::Id(RuntimeRequestId("old".into())),
            ProviderApprovalDecision::Accept,
        ))
        .unwrap_err();
    assert!(matches!(error, ClientError::InvalidPayload(..)));
}

#[test]
fn workspace_save_preserves_server_failure_and_denied_permission() {
    let mut h = Harness::new("env");
    let c = paired(&mut h);
    c.client(&environment("env"))
        .unwrap()
        .session()
        .unwrap()
        .update_grants(t3_client::permissions::SessionGrants {
            scopes: Default::default(),
            valid: true,
        });
    let plans = c.plan_services();
    assert_eq!(
        plans.plan_workspace_root(&key()).as_deref(),
        Some("/workspace/project")
    );
    assert!(!plans.can_write(&key()));
    let error = h
        .run(plans.write_workspace_file(
            key(),
            "/workspace/project".into(),
            "plan.md".into(),
            "# Plan\n".into(),
        ))
        .unwrap_err();
    assert!(matches!(error, ClientError::PermissionDenied { .. }));
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
fn seed_approval(h: &Harness) {
    let mut snapshot = thread_snapshot("t1");
    snapshot["projection"]["runtimeRequests"] = json!([request("pending")]);
    h.server()
        .state
        .borrow_mut()
        .thread_snapshots
        .insert("t1".into(), snapshot);
}
#[test]
fn approval_success_does_not_resolve_locally_and_failure_keeps_server_detail() {
    let mut h = Harness::new("env");
    seed_approval(&h);
    let c = paired(&mut h);
    let service = c.approval_services();
    let id = IdOrIndex::Id(RuntimeRequestId("approval".into()));
    script(
        &h,
        "orchestration.dispatchCommand",
        Ok(json!({"sequence":2})),
    );
    h.run(service.respond(key(), id.clone(), ProviderApprovalDecision::Accept))
        .unwrap();
    assert_eq!(service.pending_approvals(&key()).len(), 1);
    script(
        &h,
        "orchestration.dispatchCommand",
        Err(
            json!({"_tag":"OrchestrationV2DispatchCommandError","commandId":"c","commandType":"runtime-request.respond","message":"Could not respond","detail":"provider closed the callback"}),
        ),
    );
    let error = h
        .run(service.respond(key(), id.clone(), ProviderApprovalDecision::Decline))
        .unwrap_err();
    assert!(error.to_string().contains("provider closed the callback"));
    assert_eq!(service.pending_approvals(&key()).len(), 1);
    let endpoint = c.client(&environment("env")).unwrap().session().unwrap();
    endpoint.update_grants(t3_client::permissions::SessionGrants {
        scopes: Default::default(),
        valid: true,
    });
    assert!(!service.can_operate(&key()));
    assert!(matches!(
        h.run(service.respond(key(), id, ProviderApprovalDecision::Decline))
            .unwrap_err(),
        ClientError::PermissionDenied { .. }
    ));
    let calls = h.server().state.borrow();
    assert_eq!(
        calls
            .rpc_calls
            .iter()
            .filter(|(tag, _)| tag == "orchestration.dispatchCommand")
            .count(),
        2
    );
}
#[test]
fn workspace_save_reports_confirmed_path_and_typed_failure_and_download_is_explicit() {
    let mut h = Harness::new("env");
    h.server().state.borrow_mut().permissions = Some(vec![
        "orchestration:read".into(),
        "orchestration:operate".into(),
        "filesystem:write".into(),
    ]);
    let c = paired(&mut h);
    let plans = c.plan_services();
    script(
        &h,
        "projects.writeFile",
        Ok(json!({"relativePath":"plans/confirmed.md"})),
    );
    assert_eq!(
        h.run(plans.write_workspace_file(
            key(),
            "/workspace/project".into(),
            "plan.md".into(),
            "# Plan\n".into()
        ))
        .unwrap(),
        Some("plans/confirmed.md".into())
    );
    assert!(!plans.can_download());
    assert!(
        h.run(plans.download_text(
            "plan.md".into(),
            "# Plan".into(),
            "text/markdown;charset=utf-8".into()
        ))
        .is_err()
    );
}

#[test]
fn approval_resolved_between_render_and_activation_is_refused() {
    let mut h = Harness::new("env");
    seed_approval(&h);
    let c = paired(&mut h);
    let service = c.approval_services();
    assert_eq!(service.pending_approvals(&key()).len(), 1);
    let generation = c.store().snapshot().environments[&environment("env")].session;
    c.store().update_thread(&key(), generation, |state| {
        Rc::make_mut(state.projection.as_mut().unwrap()).runtime_requests[0].status =
            t3_contracts::orchestration_v2::RuntimeRequestStatus::Resolved;
        true
    });
    assert!(service.pending_approvals(&key()).is_empty());
    assert!(
        h.run(service.respond(
            key(),
            IdOrIndex::Id(RuntimeRequestId("approval".into())),
            ProviderApprovalDecision::Accept
        ))
        .is_err()
    );
    assert!(
        !h.server()
            .state
            .borrow()
            .rpc_calls
            .iter()
            .any(|(tag, _)| tag == "orchestration.dispatchCommand")
    );
}

#[test]
fn rename_overlay_is_visible_while_awaiting_and_rolls_back_on_rejection() {
    use futures::FutureExt;
    let mut h = Harness::new("env");
    let c = paired(&mut h);
    let (tx, rx) = futures::channel::oneshot::channel();
    let receiver = Rc::new(std::cell::RefCell::new(Some(rx)));
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(move |request| {
        if request.tag != "orchestration.dispatchCommand" {
            return None;
        }
        let rx = receiver.borrow_mut().take().unwrap();
        Some(t3_rpc::HandlerOutput::Unary(async move {
            let _: () = rx.await.unwrap();
            t3_rpc::Exit::fail(json!({"_tag":"OrchestrationV2DispatchCommandError","commandId":"c","commandType":"thread.metadata.update","message":"Rename refused","detail":"workspace is locked"}))
        }.boxed_local()))
    }));
    let service = c.thread_actions();
    let operation = service.rename_thread(key(), "  New title  ".into());
    let outcome = Rc::new(std::cell::RefCell::new(None));
    let sink = outcome.clone();
    h.spawn(async move {
        *sink.borrow_mut() = Some(operation.await);
    });
    h.settle();
    assert_eq!(service.thread_shell(&key()).unwrap().title, "New title");
    assert_eq!(
        c.store().shared_shell(&environment("env")).unwrap().threads[0].title,
        "Thread"
    );
    tx.send(()).unwrap();
    h.settle();
    assert!(
        outcome
            .borrow()
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("workspace is locked")
    );
    assert_eq!(service.thread_shell(&key()).unwrap().title, "Thread");
}

#[test]
fn rewind_head_refuses_a_moved_thread_and_unknown_rollback_fails_closed() {
    let mut h = Harness::new("env");
    let c = paired(&mut h);
    let mut projection: t3_contracts::orchestration_v2::OrchestrationV2ThreadProjection =
        serde_json::from_value(thread_snapshot("t1")["projection"].clone()).unwrap();
    let mut older: t3_contracts::orchestration_v2::OrchestrationV2ConversationMessage =
        serde_json::from_str(
            include_str!("../../../fixtures/samples/OrchestrationV2ConversationMessage.jsonl")
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
    older.id = IdOrIndex::Id(t3_contracts::base_schemas::MessageId("older".into()));
    let mut latest = older.clone();
    latest.id = IdOrIndex::Id(t3_contracts::base_schemas::MessageId("latest".into()));
    projection.messages = vec![older, latest];
    let head = t3_client::thread_views::thread_ops::rewind::RewindHead::capture(&projection);
    assert!(head.ensure_current(&projection).is_ok());
    projection.messages.remove(0);
    assert!(
        head.ensure_current(&projection).is_ok(),
        "bounded history refetch must retain the confirmed head"
    );
    projection.thread.active_provider_thread_id = Some(IdOrIndex::Id(
        t3_contracts::base_schemas::ProviderThreadId("new-session".into()),
    ));
    assert!(head.ensure_current(&projection).is_err());
    let service = c.timeline_rewind();
    let target = t3_client::thread_views::thread_ops::rewind::RewindTarget {
        message_id: t3_contracts::base_schemas::MessageId("old".into()),
        turn_count: 0,
    };
    assert!(!service.rewind_availability(&key(), &target).can_rewind);
    assert!(h.run(service.rewind(key(), target, true)).is_err());
    assert!(
        !h.server()
            .state
            .borrow()
            .rpc_calls
            .iter()
            .any(|(tag, _)| tag == "orchestration.dispatchCommand")
    );
}

#[test]
fn source_notice_groups_contiguous_actions_claims_shortcut_and_expires() {
    let mut h = Harness::new("env");
    let c = paired(&mut h);
    let service = c.thread_actions();
    let generation = c.store().snapshot().environments[&environment("env")].session;
    let mut config = (*c.store().snapshot().environments[&environment("env")]
        .config
        .clone()
        .unwrap())
    .clone();
    config.environment.capabilities.thread_settlement = Some(true);
    c.store()
        .set_config(&environment("env"), generation, config);
    for (index, id) in ["t1", "t2"].into_iter().enumerate() {
        script(
            &h,
            "orchestration.dispatchCommand",
            Ok(json!({"sequence":index+2})),
        );
        h.run(service.settle_thread(ThreadKey {
            environment: environment("env"),
            thread: thread(id),
        }))
        .unwrap();
    }
    let notice = service.thread_undo_notice().unwrap();
    assert_eq!(notice.action, "Settled");
    assert_eq!(notice.count, 2);
    assert!(service.thread_undo_claims_shortcut());
    service.release_thread_undo_shortcut();
    assert!(!service.thread_undo_claims_shortcut());
    script(
        &h,
        "orchestration.dispatchCommand",
        Ok(json!({"sequence":4})),
    );
    script(
        &h,
        "orchestration.dispatchCommand",
        Ok(json!({"sequence":5})),
    );
    assert_eq!(h.run(service.undo_latest_thread_action()).unwrap(), 2);
    assert!(service.thread_undo_notice().is_none());
    script(
        &h,
        "orchestration.dispatchCommand",
        Ok(json!({"sequence":6})),
    );
    h.run(service.settle_thread(key())).unwrap();
    assert!(service.thread_undo_claims_shortcut());
    h.platform.fake.advance(5_000);
    assert!(service.thread_undo_notice().is_none());
    assert!(!service.thread_undo_claims_shortcut());
    script(
        &h,
        "orchestration.dispatchCommand",
        Ok(json!({"sequence":7})),
    );
    h.run(service.settle_thread(key())).unwrap();
    assert!(service.thread_undo_notice().is_some());
    h.run(c.remove(&environment("env"))).unwrap();
    assert!(service.thread_undo_notice().is_none());
}

#[test]
fn sidebar_batch_deletes_only_accepted_targets_and_cleans_up_their_drafts() {
    use futures::FutureExt;
    let mut h = Harness::new("env");
    let archived = "orchestration.getArchivedShellSnapshot";
    h.server()
        .state
        .borrow_mut()
        .extra_methods
        .push((archived, t3_rpc::CallKind::Unary));
    let client = paired(&mut h);
    let service = client.thread_actions();
    let environment = environment("env");
    let thread_key = |id| ThreadKey {
        environment: environment.clone(),
        thread: thread(id),
    };
    let mut snapshot = shell_snapshot(&["t1", "t2", "t3", "side"]);
    snapshot["snapshotSequence"] = json!(2);
    snapshot["threads"][3]["lineage"] = json!({
        "rootThreadId":"t1","parentThreadId":"t1","relationshipToParent":null,"sideChat":true
    });
    let session = client.store().snapshot().environments[&environment]
        .shell
        .session;
    client.store().apply_shell(
        &environment,
        session,
        vec![serde_json::from_value(json!({"kind":"snapshot","snapshot":snapshot})).unwrap()],
    );
    let drafts = client.composer_drafts();
    let loading = drafts.clone();
    h.run(async move { loading.load().await }).unwrap();
    for id in ["t1", "t2", "t3", "side"] {
        drafts
            .set_prompt(&thread_key(id), format!("Draft {id}"), 0)
            .unwrap();
        client.terminals().set_open(&thread_key(id), true);
    }
    service.bind_confirmation(Rc::new(|message, destructive| {
        assert!(destructive);
        assert!(message.contains("Delete 3 threads?"));
        async { Ok(true) }.boxed_local()
    }));
    for sequence in [3, 4] {
        script(
            &h,
            archived,
            Ok(json!({"schemaVersion":1,"snapshotSequence":sequence,"projects":[],"threads":[]})),
        );
    }
    let dispatch = "orchestration.dispatchCommand";
    script(&h, dispatch, Ok(json!({"sequence":3})));
    script(
        &h,
        dispatch,
        Err(
            json!({"_tag":"OrchestrationV2DispatchCommandError","commandId":"fixture-command","commandType":"thread.delete","message":"Rejected t2"}),
        ),
    );
    script(&h, dispatch, Ok(json!({"sequence":4})));
    let report = h
        .run(service.perform_sidebar_batch(
            vec![thread_key("t1"), thread_key("t2"), thread_key("t3")],
            "delete".into(),
            None,
        ))
        .unwrap();
    h.settle();
    assert_eq!(report.completed, [thread_key("t1"), thread_key("t3")]);
    assert_eq!(
        report.failed.iter().map(|(key, _)| key).collect::<Vec<_>>(),
        [&thread_key("t2")]
    );
    for id in ["t1", "t3", "side"] {
        assert!(drafts.view(&thread_key(id)).prompt.is_empty(), "{id} draft");
    }
    assert_eq!(drafts.view(&thread_key("t2")).prompt.as_ref(), "Draft t2");
    assert!(!client.terminals().ui_state(&thread_key("t1")).terminal_open);
    assert!(!client.terminals().ui_state(&thread_key("t3")).terminal_open);
    assert!(
        !client
            .terminals()
            .ui_state(&thread_key("side"))
            .terminal_open
    );
    assert!(client.terminals().ui_state(&thread_key("t2")).terminal_open);
    assert_eq!(
        service
            .archived_shell(&environment)
            .unwrap()
            .snapshot_sequence,
        4
    );
    assert_eq!(
        h.server()
            .state
            .borrow()
            .rpc_calls
            .iter()
            .filter(|(method, _)| method == archived)
            .count(),
        2
    );
}

#[test]
fn sidebar_delete_releases_only_uploads_without_a_surviving_draft_owner() {
    use futures::{FutureExt, StreamExt};
    use t3_client::thread_views::thread_ops::deletion::DeletionEvent;
    let mut h = Harness::new("env");
    let unupload = "attachments.delete";
    h.server()
        .state
        .borrow_mut()
        .extra_methods
        .push((unupload, t3_rpc::CallKind::Unary));
    script(
        &h,
        unupload,
        Err(
            json!({"_tag":"EnvironmentAuthorizationError","message":"Fixture denied the first upload cleanup","requiredScope":"orchestration:operate"}),
        ),
    );
    script(&h, unupload, Ok(serde_json::Value::Null));
    let client = paired(&mut h);
    let drafts = client.composer_drafts();
    let loading = drafts.clone();
    h.run(async move { loading.load().await }).unwrap();
    let first = key();
    let second = ThreadKey {
        environment: first.environment.clone(),
        thread: thread("t2"),
    };
    drafts.set_prompt(&first, "one".into(), 0).unwrap();
    drafts.set_prompt(&second, "two".into(), 0).unwrap();
    let first_id = drafts.thread_draft_key(&first);
    let second_id = drafts.thread_draft_key(&second);
    let upload = |id: &str| {
        json!({
            "type":"file","id":id,"name":"notes.txt","mimeType":"text/plain","sizeBytes":1,
            "uploadedAttachmentId":id,"uploadEnvironmentId":"env"
        })
    };
    drafts
        .update(|document| {
            document["draftsByThreadKey"][&first_id]["files"] = json!([
                upload("unique-first"),
                upload("unique-second"),
                upload("shared")
            ]);
            document["draftsByThreadKey"][&second_id]["files"] = json!([upload("shared")]);
            Ok(())
        })
        .unwrap();
    let service = client.thread_actions();
    let mut events = service.deletion_events();
    service.bind_confirmation(Rc::new(|_, _| async { Ok(true) }.boxed_local()));
    script(
        &h,
        "orchestration.dispatchCommand",
        Ok(json!({"sequence":2})),
    );
    h.run(service.delete_thread(first)).unwrap();
    h.settle();
    let server = h.server();
    let state = server.state.borrow();
    let inputs = state
        .rpc_calls
        .iter()
        .filter(|(method, _)| method == unupload)
        .map(|(_, payload)| &payload["attachmentId"])
        .collect::<Vec<_>>();
    assert_eq!(inputs, [&json!("unique-first"), &json!("unique-second")]);
    assert!(matches!(
        events.next().now_or_never(),
        Some(Some(DeletionEvent::Accepted(_)))
    ));
    assert!(
        matches!(events.next().now_or_never(), Some(Some(DeletionEvent::CleanupFailed { title, .. })) if title.contains("drafts and uploads"))
    );
    assert!(drafts.view(&second).prompt.starts_with("two"));
    let document = drafts.document();
    let document = document.borrow();
    assert_eq!(
        document["draftsByThreadKey"][&second_id]["files"][0]["uploadedAttachmentId"],
        "shared"
    );
}

#[test]
fn sidebar_worktree_cleanup_requires_explicit_confirmation_and_an_accepted_delete() {
    use futures::FutureExt;
    let delete = "orchestration.dispatchCommand";
    let worktree = "vcs.removeWorktree";
    for (answer, accepted) in [(false, true), (true, false), (true, true)] {
        let mut h = Harness::new("env");
        {
            let server = h.server();
            let mut settings = server.state.borrow_mut();
            settings
                .extra_methods
                .push((worktree, t3_rpc::CallKind::Unary));
            settings.permissions = Some(
                [
                    "orchestration:read",
                    "orchestration:operate",
                    "source-control:write",
                ]
                .map(str::to_owned)
                .to_vec(),
            );
        }
        script(&h, worktree, Ok(serde_json::Value::Null));
        let client = paired(&mut h);
        let thread = key();
        let mut snapshot = shell_snapshot(&["t1", "t2"]);
        snapshot["snapshotSequence"] = json!(2);
        snapshot["threads"][0]["worktreePath"] = json!("/workspace/worktrees/lane");
        let session = client.store().snapshot().environments[&thread.environment].session;
        client.store().apply_shell(
            &thread.environment,
            session,
            vec![serde_json::from_value(json!({"kind":"snapshot","snapshot":snapshot})).unwrap()],
        );
        let service = client.thread_actions();
        let confirmations = Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = confirmations.clone();
        service.bind_confirmation(Rc::new(move |message, destructive| {
            assert!(destructive);
            let confirm = !message.contains("Delete the worktree too?") || answer;
            seen.borrow_mut().push(message);
            async move { Ok(confirm) }.boxed_local()
        }));
        script(
            &h,
            delete,
            if accepted {
                Ok(json!({"sequence":3}))
            } else {
                Err(
                    json!({"_tag":"OrchestrationV2DispatchCommandError","commandId":"fixture-command","commandType":"thread.delete","message":"Rejected deletion"}),
                )
            },
        );
        assert_eq!(h.run(service.delete_thread(thread)).is_ok(), accepted);
        h.settle();
        assert_eq!(confirmations.borrow().len(), 2);
        assert!(confirmations.borrow()[1].contains("\nlane\n"));
        let server = h.server();
        let state = server.state.borrow();
        let effects = state
            .rpc_calls
            .iter()
            .filter(|(method, _)| method == delete || method == worktree)
            .collect::<Vec<_>>();
        assert_eq!(effects.len(), if answer && accepted { 2 } else { 1 });
        assert_eq!(effects[0].0, delete);
        if answer && accepted {
            assert_eq!(effects[1].0, worktree);
            assert_eq!(
                effects[1].1,
                json!({"cwd":"/workspace/project","path":"/workspace/worktrees/lane","force":true})
            );
        }
    }
}

#[test]
fn sidebar_deletion_clears_a_side_chat_that_appears_while_the_worktree_prompt_is_open() {
    use futures::FutureExt;
    let mut h = Harness::new("env");
    h.server().state.borrow_mut().permissions = Some(
        [
            "orchestration:read",
            "orchestration:operate",
            "source-control:write",
        ]
        .map(str::to_owned)
        .to_vec(),
    );
    h.server()
        .state
        .borrow_mut()
        .extra_methods
        .push(("vcs.removeWorktree", t3_rpc::CallKind::Unary));
    let client = paired(&mut h);
    let parent = key();
    let child = ThreadKey {
        environment: parent.environment.clone(),
        thread: thread("late-child"),
    };
    let drafts = client.composer_drafts();
    let loading = drafts.clone();
    h.run(async move { loading.load().await }).unwrap();
    drafts
        .set_prompt(&child, "New side chat".into(), 0)
        .unwrap();
    client.terminals().set_open(&child, true);
    let mut before = shell_snapshot(&["t1"]);
    before["snapshotSequence"] = json!(2);
    before["threads"][0]["worktreePath"] = json!("/workspace/worktrees/lane");
    let mut during = before.clone();
    during["snapshotSequence"] = json!(3);
    let mut side_chat = shell_snapshot(&["late-child"])["threads"][0].clone();
    side_chat["lineage"] = json!({
        "rootThreadId":"t1","parentThreadId":"t1","relationshipToParent":null,"sideChat":true
    });
    during["threads"].as_array_mut().unwrap().push(side_chat);
    let store = client.store().clone();
    let environment = parent.environment.clone();
    let session = store.snapshot().environments[&environment].session;
    let publish = move |snapshot| {
        store.apply_shell(
            &environment,
            session,
            vec![serde_json::from_value(json!({"kind":"snapshot","snapshot":snapshot})).unwrap()],
        )
    };
    publish(before);
    let service = client.thread_actions();
    let prompted = Rc::new(std::cell::Cell::new(false));
    let observed = prompted.clone();
    service.bind_confirmation(Rc::new(move |message, _| {
        let worktree_prompt = message.contains("Delete the worktree too?");
        if worktree_prompt {
            observed.set(true);
            publish(during.clone());
        }
        async move { Ok(!worktree_prompt) }.boxed_local()
    }));
    script(
        &h,
        "orchestration.dispatchCommand",
        Ok(json!({"sequence":4})),
    );
    h.run(service.delete_thread(parent)).unwrap();
    h.settle();
    assert!(prompted.get());
    assert!(drafts.view(&child).prompt.is_empty());
    assert!(!client.terminals().ui_state(&child).terminal_open);
}

#[test]
fn sidebar_delete_completes_after_the_waiting_view_is_cancelled() {
    use futures::{
        FutureExt,
        future::{AbortHandle, Abortable},
    };
    let mut h = Harness::new("env");
    let client = paired(&mut h);
    let target = key();
    let drafts = client.composer_drafts();
    let loading = drafts.clone();
    h.run(async move { loading.load().await }).unwrap();
    drafts.set_prompt(&target, "Discard me".into(), 0).unwrap();
    client.terminals().set_open(&target, true);
    let (ack, response) = futures::channel::oneshot::channel();
    let receiver = Rc::new(std::cell::RefCell::new(Some(response)));
    let received = receiver.clone();
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(move |request| {
        if request.tag != "orchestration.dispatchCommand" {
            return None;
        }
        let response = received.borrow_mut().take().unwrap();
        Some(t3_rpc::HandlerOutput::Unary(
            async move {
                response.await.unwrap();
                t3_rpc::Exit::success(json!({"sequence":2}))
            }
            .boxed_local(),
        ))
    }));
    let service = client.thread_actions();
    service.bind_confirmation(Rc::new(|_, _| async { Ok(true) }.boxed_local()));
    let pending = service.delete_thread(target.clone());
    let (cancel, registration) = AbortHandle::new_pair();
    h.spawn(async move {
        assert!(Abortable::new(pending, registration).await.is_err());
    });
    h.settle();
    assert!(receiver.borrow().is_none(), "server received the delete");
    cancel.abort();
    h.settle();
    ack.send(())
        .expect("client still observing the accepted deletion");
    h.settle();
    assert!(drafts.view(&target).prompt.is_empty());
    assert!(!client.terminals().ui_state(&target).terminal_open);
}

#[test]
fn destructive_action_waits_for_confirmation_and_rechecks_permission() {
    use futures::FutureExt;
    let mut h = Harness::new("env");
    let c = paired(&mut h);
    let service = c.thread_actions();
    let (tx, rx) = futures::channel::oneshot::channel();
    let rx = Rc::new(std::cell::RefCell::new(Some(rx)));
    service.bind_confirmation(Rc::new(move |message, destructive| {
        assert!(destructive);
        assert!(message.contains("This permanently clears conversation history for this thread."));
        let rx = rx.borrow_mut().take().unwrap();
        async move { Ok(rx.await.unwrap()) }.boxed_local()
    }));
    let result = Rc::new(std::cell::RefCell::new(None));
    let slot = result.clone();
    let pending = service.delete_thread(key());
    h.spawn(async move {
        *slot.borrow_mut() = Some(pending.await);
    });
    h.settle();
    assert!(
        !h.server()
            .state
            .borrow()
            .rpc_calls
            .iter()
            .any(|(tag, _)| tag == "orchestration.dispatchCommand")
    );
    c.client(&environment("env"))
        .unwrap()
        .session()
        .unwrap()
        .update_grants(t3_client::permissions::SessionGrants {
            scopes: Default::default(),
            valid: true,
        });
    tx.send(true).unwrap();
    h.settle();
    assert!(matches!(
        result.borrow().as_ref().unwrap(),
        Err(ClientError::PermissionDenied { .. })
    ));
    assert_eq!(
        c.store()
            .shared_shell(&environment("env"))
            .unwrap()
            .threads
            .len(),
        2
    );
}

#[test]
fn operation_changes_emit_initial_and_updates_and_end_with_runtime() {
    use futures::StreamExt;
    let mut h = Harness::new("env");
    let c = paired(&mut h);
    let service = c.thread_actions();
    let mut changes = service.changes(&key());
    assert!(h.pool.run_until(changes.next()).is_some());
    let generation = c.store().snapshot().environments[&environment("env")].session;
    let mut cfg = (*c.store().snapshot().environments[&environment("env")]
        .config
        .clone()
        .unwrap())
    .clone();
    cfg.environment.capabilities.thread_settlement = Some(true);
    c.store().set_config(&environment("env"), generation, cfg);
    assert!(h.pool.run_until(changes.next()).is_some());
    drop(service);
    drop(c);
    h.settle();
    while h.pool.run_until(changes.next()).is_some() {}
}

#[test]
fn project_group_stops_at_first_typed_failure_and_scratch_identity_uses_config() {
    let mut h = Harness::new("env");
    let c = paired(&mut h);
    let service = c.project_actions();
    let generation = c.store().snapshot().environments[&environment("env")].session;
    let mut cfg = (*c.store().snapshot().environments[&environment("env")]
        .config
        .clone()
        .unwrap())
    .clone();
    cfg.environment.capabilities.project_organization = Some(true);
    cfg.scratch_workspace_root = Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
        "/workspace/project/".into(),
    ));
    c.store().set_config(&environment("env"), generation, cfg);
    let members = vec![
        t3_client::workspace::ProjectKey {
            environment: environment("env"),
            project: t3_client::types::ProjectId("project".into()),
        },
        t3_client::workspace::ProjectKey {
            environment: environment("env"),
            project: t3_client::types::ProjectId("second".into()),
        },
    ];
    assert!(service.can_organize_projects(&members));
    assert_eq!(service.header_state(&key()).is_scratch, Some(true));
    script(
        &h,
        "projects.mutate",
        Err(json!({"_tag":"ProjectMutationError","commandId":"c","message":"Checkout is locked"})),
    );
    let error = h
        .run(service.set_project_group_pinned(members, true))
        .unwrap_err();
    assert!(error.to_string().contains("Checkout is locked"), "{error}");
    assert_eq!(
        h.server()
            .state
            .borrow()
            .rpc_calls
            .iter()
            .filter(|(tag, _)| tag == "projects.mutate")
            .count(),
        1
    );
}

#[test]
fn snooze_preserves_explicit_empty_presets_and_refuses_expired_choices() {
    let mut h = Harness::new("env");
    let c = paired(&mut h);
    let service = c.thread_actions();
    let generation = c.store().snapshot().environments[&environment("env")].session;
    let mut cfg = (*c.store().snapshot().environments[&environment("env")]
        .config
        .clone()
        .unwrap())
    .clone();
    cfg.environment.capabilities.thread_snooze = Some(true);
    cfg.settings.snooze_presets.clear();
    c.store().set_config(&environment("env"), generation, cfg);
    assert!(service.snooze_settings(&environment("env")).is_empty());
    assert!(!service.snooze_settings(&environment("missing")).is_empty());
    assert!(service.snooze_menu(&key()).can_snooze);
    assert!(
        h.run(service.snooze_thread(
            key(),
            t3_contracts::wire::WireDateTime("1970-01-01T00:00:00.000Z".into())
        ))
        .unwrap_err()
        .to_string()
        .contains("That snooze time has passed")
    );
    assert!(
        !h.server()
            .state
            .borrow()
            .rpc_calls
            .iter()
            .any(|(tag, _)| tag == "orchestration.dispatchCommand")
    );
}

#[test]
fn unsupported_save_is_a_typed_error_and_disables_the_capability_for_this_session() {
    use futures::FutureExt;
    let mut h = Harness::new("env");
    h.server().state.borrow_mut().permissions = Some(vec![
        "orchestration:read".into(),
        "orchestration:operate".into(),
        "filesystem:write".into(),
    ]);
    let c = paired(&mut h);
    let service = c.plan_services();
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(|request| {
        (request.tag == "projects.writeFile").then(|| {
            t3_rpc::HandlerOutput::Unary(
                async { t3_rpc::Exit::die("Unknown request tag: projects.writeFile") }
                    .boxed_local(),
            )
        })
    }));
    assert!(service.can_write(&key()));
    assert!(matches!(
        h.run(service.write_workspace_file(
            key(),
            "/workspace/project".into(),
            "plan.md".into(),
            "text".into()
        )),
        Err(ClientError::Unsupported(_))
    ));
    assert!(!service.can_write(&key()));
}

#[test]
fn header_is_current_on_subscription_and_pin_uses_the_top_order_key() {
    use futures::StreamExt;
    let mut h = Harness::new("env");
    let c = paired(&mut h);
    let service = c.thread_actions();
    let mut states = service.watch_header(key());
    assert!(
        h.run(async move { states.next().await })
            .unwrap()
            .can_operate
    );
    assert!(
        service
            .thread_action_menu(&key())
            .items
            .iter()
            .filter(|item| matches!(item.id.as_str(), "archive" | "delete"))
            .all(|item| item.disabled == Some(true))
    );
    assert!(matches!(
        h.run(service.perform_action(key(), "delete".into())),
        Err(ClientError::Unsupported(_))
    ));
    let generation = c.store().snapshot().environments[&environment("env")].session;
    let mut config = (*c.store().snapshot().environments[&environment("env")]
        .config
        .clone()
        .unwrap())
    .clone();
    config.environment.capabilities.thread_pinning = Some(true);
    c.store()
        .set_config(&environment("env"), generation, config);
    script(
        &h,
        "orchestration.dispatchCommand",
        Ok(json!({"sequence":2})),
    );
    h.run(service.set_thread_pinned(key(), true)).unwrap();
    let server = h.server();
    let calls = server.state.borrow();
    let (_, command) = calls
        .rpc_calls
        .iter()
        .find(|(method, _)| method == "orchestration.dispatchCommand")
        .unwrap();
    assert_eq!(
        command["orderKey"],
        json!(t3_client::thread_views::threads::sort::pin_order_key_between(None, None).unwrap())
    );
}

#[test]
fn thread_link_lookup_keeps_the_query_alive_and_surfaces_unsupported() {
    use futures::StreamExt;
    let mut h = Harness::new("env");
    h.server().state.borrow_mut().extra_methods.push((
        "orchestration.getThreadLinkMetadata",
        t3_rpc::CallKind::Unary,
    ));
    let c = paired(&mut h);
    script(
        &h,
        "orchestration.getThreadLinkMetadata",
        Ok(json!({"t1":{"title":"Linked title","projectId":"project"}})),
    );
    let mut lookup = c
        .thread_metadata()
        .watch_thread_link_metadata(environment("env"), vec![thread("t1")]);
    h.settle();
    let (result, lookup) = h.run(async move { (lookup.next().await.unwrap(), lookup) });
    assert!(result.settled);
    assert_eq!(result.metadata[&thread("t1")].title, "Linked title");
    assert!(result.error.is_none());
    drop(lookup);
    let mut unsupported = c
        .thread_metadata()
        .watch_thread_link_metadata(environment("env"), vec![thread("missing")]);
    h.settle();
    let result = h.run(async move { unsupported.next().await.unwrap() });
    assert!(result.settled);
    assert!(result.error.is_some());
    assert!(result.metadata.is_empty());
    script(&h, "orchestration.getThreadLinkMetadata", Ok(json!({})));
    let mut ending = c
        .thread_metadata()
        .watch_thread_link_metadata(environment("env"), vec![thread("t2")]);
    h.settle();
    let ending = h.run(async move {
        ending.next().await.unwrap();
        ending
    });
    drop(c);
    h.settle();
    assert!(
        h.run(async move { ending.collect::<Vec<_>>().await })
            .iter()
            .all(|lookup| lookup.settled)
    );
}

fn rewind_snapshot() -> serde_json::Value {
    let mut snapshot = thread_snapshot("t1");
    let p = &mut snapshot["projection"];
    p["thread"]["activeProviderThreadId"] = json!("pt");
    p["thread"]["worktreePath"] = json!("/workspace/worktree");
    let mut session: serde_json::Value = serde_json::from_str(
        include_str!("../../../fixtures/samples/OrchestrationV2ProviderSession.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    session["id"] = json!("session");
    session["status"] = json!("ready");
    session["driver"] = json!("codex");
    session["providerInstanceId"] = json!("codex");
    session["cwd"] = json!("/workspace/worktree");
    session["model"] = json!("gpt-5.4");
    p["providerSessions"] = json!([session]);
    p["providerThreads"] = json!([{"id":"pt","driver":"codex","providerInstanceId":"codex","providerSessionId":"session","appThreadId":"t1","ownerNodeId":null,"nativeThreadRef":null,"nativeConversationHeadRef":null,"status":"idle","firstRunOrdinal":1,"lastRunOrdinal":1,"handoffIds":[],"forkedFrom":null,"createdAt":NOW,"updatedAt":NOW}]);
    p["runs"] = json!([{"id":"run","threadId":"t1","ordinal":1,"providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"providerThreadId":"pt","userMessageId":"message","rootNodeId":null,"activeAttemptId":null,"status":"completed","requestedAt":NOW,"startedAt":NOW,"completedAt":NOW,"checkpointId":null,"contextHandoffId":null}]);
    let attachment =
        json!({"type":"file","id":"file","name":"note.txt","mimeType":"text/plain","sizeBytes":4});
    p["messages"] = json!([{"id":"message","threadId":"t1","runId":"run","nodeId":null,"role":"user","text":"Recalled prompt","attachments":[attachment.clone(),attachment],"streaming":false,"createdBy":"user","creationSource":"web","createdAt":NOW,"updatedAt":NOW}]);
    let item = json!({"id":"item","type":"user_message","threadId":"t1","runId":"run","nodeId":null,"providerThreadId":"pt","providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":NOW,"completedAt":NOW,"updatedAt":NOW,"messageId":"message","text":"Recalled prompt","inputIntent":"turn_start","attachments":[],"createdBy":"user","creationSource":"web"});
    p["turnItems"] = json!([item.clone()]);
    p["visibleTurnItems"] = json!([{"position":0,"visibility":"local","sourceThreadId":"t1","sourceItemId":"item","item":item}]);
    p["checkpoints"] = json!([{"id":"checkpoint","threadId":"t1","scopeId":"scope","runId":null,"nodeId":"node","parentCheckpointId":null,"ordinalWithinScope":0,"appRunOrdinal":null,"ref":"root","status":"ready","files":[],"capturedAt":NOW}]);
    snapshot
}
fn fork_response(source: &str, visibility: &str) -> serde_json::Value {
    let row = json!({
        "id":"response","type":"assistant_message","threadId":source,"runId":"run",
        "nodeId":null,"providerThreadId":"response-thread","providerTurnId":null,
        "nativeItemRef":null,"parentItemId":null,"ordinal":2,"status":"completed",
        "title":null,"startedAt":NOW,"completedAt":NOW,"updatedAt":NOW,
        "messageId":"answer","text":"Response","streaming":false,
    });
    json!({"position":0,"visibility":visibility,"sourceThreadId":source,"sourceItemId":"response","item":row})
}

#[test]
fn an_inherited_fork_can_dispatch_with_its_source_projection_unloaded() {
    let mut h = Harness::new("env");
    let mut snapshot = thread_snapshot("t1");
    snapshot["projection"]["visibleTurnItems"] = json!([fork_response("parent", "inherited")]);
    h.server()
        .state
        .borrow_mut()
        .thread_snapshots
        .insert("t1".into(), snapshot);
    let c = paired(&mut h);
    let source = ThreadKey {
        thread: thread("parent"),
        ..key()
    };
    assert!(c.store().shared_thread(&source).is_none());
    let _ = h.run(c.timeline_rewind().fork_from_response(
        key(),
        source.thread,
        t3_contracts::base_schemas::TurnItemId("response".into()),
        t3_contracts::base_schemas::RunId("run".into()),
    ));
    assert!(
        h.server()
            .state
            .borrow()
            .rpc_calls
            .iter()
            .any(|(tag, command)| {
                tag == "orchestration.dispatchCommand"
                    && command["type"] == "thread.fork"
                    && command["sourceThreadId"] == "parent"
                    && command["sourcePoint"]["runId"] == "run"
            })
    );
}

#[test]
fn fork_preflight_uses_the_completed_responses_session() {
    for response_supports_fork in [true, false] {
        let mut h = Harness::new("env");
        let mut snapshot = rewind_snapshot();
        let p = &mut snapshot["projection"];
        let mut response_thread = p["providerThreads"][0].clone();
        response_thread["id"] = json!("response-thread");
        response_thread["providerSessionId"] = json!("response-session");
        p["providerThreads"]
            .as_array_mut()
            .unwrap()
            .push(response_thread);
        let mut response_session = p["providerSessions"][0].clone();
        response_session["id"] = json!("response-session");
        for (session, supports) in [
            (&mut p["providerSessions"][0], !response_supports_fork),
            (&mut response_session, response_supports_fork),
        ] {
            session["capabilities"]["threads"]["canForkThread"] = json!(false);
            session["capabilities"]["context"]["supportsFullThreadHandoff"] = json!(supports);
        }
        p["providerSessions"]
            .as_array_mut()
            .unwrap()
            .push(response_session);
        p["visibleTurnItems"] = json!([fork_response("t1", "local")]);
        h.server()
            .state
            .borrow_mut()
            .thread_snapshots
            .insert("t1".into(), snapshot);
        let c = paired(&mut h);
        assert!(c.store().shared_thread(&key()).is_some());
        let _ = h.run(c.timeline_rewind().fork_from_response(
            key(),
            key().thread,
            t3_contracts::base_schemas::TurnItemId("response".into()),
            t3_contracts::base_schemas::RunId("run".into()),
        ));
        assert_eq!(
            h.server()
                .state
                .borrow()
                .rpc_calls
                .iter()
                .any(|(tag, command)| {
                    tag == "orchestration.dispatchCommand" && command["type"] == "thread.fork"
                }),
            response_supports_fork
        );
    }
}

#[test]
fn accepted_rewind_survives_observer_disposal_and_restores_duplicates_beside_new_edits() {
    use futures::{
        FutureExt,
        future::{AbortHandle, Abortable},
    };
    let mut h = Harness::new("env");
    let _: t3_contracts::orchestration_v2::OrchestrationV2ThreadProjection =
        serde_json::from_value(rewind_snapshot()["projection"].clone()).unwrap();
    h.server()
        .state
        .borrow_mut()
        .thread_snapshots
        .insert("t1".into(), rewind_snapshot());
    h.server()
        .state
        .borrow_mut()
        .extra_methods
        .push(("assets.createUrl", t3_rpc::CallKind::Unary));
    h.server().state.borrow_mut().permissions = Some(vec![
        "orchestration:read".into(),
        "orchestration:operate".into(),
        "assets:read".into(),
    ]);
    let c = paired(&mut h);
    let service = c.timeline_rewind();
    for _ in 0..2 {
        script(
            &h,
            "assets.createUrl",
            Ok(json!({
                "relativeUrl":"/test-assets/rewind-note","expiresAt":4102444800000_u64,
            })),
        );
    }
    let target = t3_client::thread_views::thread_ops::rewind::RewindTarget {
        message_id: t3_contracts::base_schemas::MessageId("message".into()),
        turn_count: 0,
    };
    let availability = service.rewind_availability(&key(), &target);
    assert!(availability.can_restore_files, "{availability:?}");
    script(
        &h,
        "orchestration.dispatchCommand",
        Ok(json!({"sequence":2})),
    );
    let (abort, registration) = AbortHandle::new_pair();
    let operation = service.rewind(key(), target, false);
    h.spawn(Abortable::new(operation, registration).map(|_| ()));
    h.settle();
    assert!(service.rewind_state(&key()).reverting);
    abort.abort();
    c.close_thread(&key());
    h.settle();
    assert_eq!(h.server().open_threads_of("t1"), 1);
    c.composer_drafts()
        .update(|document| {
            t3_client::thread_views::composer::web_models::set_prompt(
                document, "env:t1", "New edit",
            );
            Ok(())
        })
        .unwrap();
    let generation = c.store().snapshot().environments[&environment("env")].session;
    c.store().update_thread(&key(), generation, |state| {
        let p = Rc::make_mut(state.projection.as_mut().unwrap());
        p.runs[0].status = t3_contracts::orchestration_v2::OrchestrationV2RunStatus::RolledBack;
        p.provider_threads[0].last_run_ordinal = None;
        true
    });
    h.settle();
    assert_eq!(
        t3_client::view_logic::context_references::strip_references(
            &c.composer_drafts().view(&key()).prompt
        )
        .trim(),
        "New edit\n\nRecalled prompt"
    );
    assert_eq!(
        c.composer_drafts().document().borrow()["draftsByThreadKey"]["env:t1"]["files"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(!service.rewind_state(&key()).reverting);
    assert_eq!(h.server().open_threads_of("t1"), 0);
    let drafts = Rc::new(
        t3_client::thread_views::composer::draft_service::ComposerDraftService::new(Rc::new(
            h.platform.clone(),
        )),
    );
    let platform = Rc::new(h.platform.clone());
    let persisted = h.run(async move {
        drafts.load().await.unwrap();
        let session = t3_client::thread_views::composer::attachment_intake::AttachmentSession::new(
            platform,
            drafts.document(),
            "env:t1".into(),
            key().environment,
        );
        session.set_connection(
            t3_client::thread_views::composer::attachment_intake::Capabilities {
                known: true,
                uploads: true,
                max_file_bytes: Some(1024),
            },
            None,
        );
        assert!(session.items().iter().all(|item| !item.needs_reattach));
        assert!(
            session
                .draft_attachments()
                .unwrap()
                .iter()
                .all(|file| { file.value()["dataUrl"] == "data:text/plain;base64,bm90ZQ==" })
        );
        (
            drafts.view(&key()).prompt.clone(),
            drafts.document().borrow()["draftsByThreadKey"]["env:t1"]["files"]
                .as_array()
                .unwrap()
                .len(),
        )
    });
    assert_eq!(
        t3_client::view_logic::context_references::strip_references(&persisted.0).trim(),
        "New edit\n\nRecalled prompt"
    );
    assert_eq!(persisted.1, 2);
}

#[test]
fn preparation_retry_uses_the_selected_failed_run_and_rechecks_permission() {
    let mut h = Harness::new("env");
    let mut snapshot = rewind_snapshot();
    snapshot["projection"]["runs"][0]["status"] = json!("failed");
    snapshot["projection"]["runs"][0]["workspacePreparation"] =
        json!({"type":"worktree","baseRef":"main"});
    let mut failure = snapshot["projection"]["turnItems"][0].clone();
    failure["type"] = json!("error");
    failure["status"] = json!("failed");
    failure["failure"] = json!({"class":"validation_error","message":"fetch failed","code":"workspace_preparation_failed","retryable":false});
    snapshot["projection"]["turnItems"] = json!([failure]);
    h.server()
        .state
        .borrow_mut()
        .thread_snapshots
        .insert("t1".into(), snapshot);
    let c = paired(&mut h);
    let service = c.thread_metadata();
    let run = t3_contracts::base_schemas::RunId("run".into());
    script(
        &h,
        "orchestration.dispatchCommand",
        Ok(json!({"sequence":2})),
    );
    h.run(service.retry_workspace_preparation_run(key(), run.clone()))
        .unwrap();
    script(
        &h,
        "orchestration.dispatchCommand",
        Err(
            json!({"_tag":"OrchestrationV2DispatchCommandError","commandId":"c","commandType":"prepared-run.retry","message":"Retry refused","detail":"Worktree is locked"}),
        ),
    );
    assert!(
        h.run(service.retry_workspace_preparation_run(key(), run.clone()))
            .unwrap_err()
            .to_string()
            .contains("Worktree is locked")
    );
    c.client(&environment("env"))
        .unwrap()
        .session()
        .unwrap()
        .update_grants(t3_client::permissions::SessionGrants {
            scopes: ["orchestration:read".into()].into(),
            valid: true,
        });
    assert!(matches!(
        h.run(service.retry_workspace_preparation_run(key(), run.clone())),
        Err(ClientError::PermissionDenied { .. })
    ));
    let generation = c.store().snapshot().environments[&environment("env")].session;
    c.store().update_thread(&key(), generation, |state| {
        Rc::make_mut(state.projection.as_mut().unwrap()).runs[0].status =
            t3_contracts::orchestration_v2::OrchestrationV2RunStatus::Completed;
        true
    });
    assert!(matches!(
        h.run(service.retry_workspace_preparation_run(key(), run)),
        Err(ClientError::InvalidPayload(..))
    ));
    let server = h.server();
    let calls = server.state.borrow();
    let calls: Vec<_> = calls
        .rpc_calls
        .iter()
        .filter(|(method, _)| method == "orchestration.dispatchCommand")
        .collect();
    assert_eq!(calls.len(), 2);
    assert!(
        calls.iter().all(
            |(_, command)| command["type"] == "prepared-run.retry" && command["runId"] == "run"
        )
    );
}

#[test]
fn header_project_new_thread_reuses_the_shared_local_draft_without_dispatch() {
    let mut h = Harness::new("env");
    let c = paired(&mut h);
    let generation = c.store().snapshot().environments[&environment("env")].session;
    let mut config = (*c.store().snapshot().environments[&environment("env")]
        .config
        .clone()
        .unwrap())
    .clone();
    config.settings.default_thread_env_mode = Some(t3_contracts::environment::ThreadEnvMode::Local);
    c.store()
        .set_config(&environment("env"), generation, config);
    let project = t3_client::workspace::ProjectKey {
        environment: environment("env"),
        project: t3_client::types::ProjectId("project".into()),
    };
    let service = c.thread_header();
    assert!(
        service
            .project_new_thread_reason(&project, &key())
            .is_none()
    );
    let first = h
        .run(service.new_thread_in_project(project.clone(), key()))
        .unwrap();
    assert!(
        matches!(&first,t3_client::navigation::new_thread::Destination::Draft(id) if !id.is_empty())
    );
    assert_eq!(
        h.run(service.new_thread_in_project(project, key()))
            .unwrap(),
        first
    );
    assert!(
        !h.server()
            .state
            .borrow()
            .rpc_calls
            .iter()
            .any(|(method, _)| method == "orchestration.dispatchCommand")
    );
}
