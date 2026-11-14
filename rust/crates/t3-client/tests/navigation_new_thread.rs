#![allow(clippy::unwrap_used)]
mod support {
    pub mod fake_server;
}
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use support::fake_server::*;
use t3_client::{
    ClientError, ClientStore,
    composition::ClientRuntime,
    connection::PairingInput,
    navigation::new_thread::{
        DraftAuthority, DraftEdit, NewThreadRuntime, NewThreadState, NewThreadView, Target,
    },
    platform::PlatformError,
    services::resources::{Resource, ResourcePolicy},
    thread_views::composer::{
        attachment_intake::AttachmentSession, draft_service::ComposerDraftView,
        target::ComposerTarget, web_models,
    },
    types::{EnvironmentId, Generation, ProjectId},
    workspace::ProjectKey,
};

struct FakeDrafts {
    document: Rc<RefCell<Value>>,
    revision: Cell<u64>,
    changed: Resource<()>,
    deny: Cell<bool>,
}
impl Default for FakeDrafts {
    fn default() -> Self {
        Self {
            document: Rc::new(RefCell::new(
                json!({"draftsByThreadKey":{},"draftThreadsByThreadKey":{},"logicalProjectDraftThreadKeyByLogicalProjectKey":{}}),
            )),
            revision: Cell::new(0),
            changed: Resource::new(ResourcePolicy::default(), Generation::default()),
            deny: Cell::new(false),
        }
    }
}
impl DraftAuthority for FakeDrafts {
    fn document(&self) -> Rc<RefCell<Value>> {
        self.document.clone()
    }
    fn changes(&self) -> LocalBoxStream<'static, ()> {
        self.changed.observe(0).map(|_| ()).boxed_local()
    }
    fn view(&self, id: &str) -> Rc<ComposerDraftView> {
        let d = self.document.borrow();
        Rc::new(ComposerDraftView {
            draft_key: id.into(),
            revision: self.revision.get(),
            prompt: d["draftsByThreadKey"][id]["prompt"]
                .as_str()
                .unwrap_or_default()
                .into(),
            cursor: d["draftsByThreadKey"][id]["cursor"].as_u64().unwrap_or(0) as usize,
            loaded: true,
            writable: !self.deny.get(),
            error: None,
        })
    }
    fn update(&self, change: DraftEdit) -> Result<bool, ClientError> {
        if self.deny.get() {
            return Err(PlatformError::Denied("draft".into()).into());
        }
        let before = self.document.borrow().clone();
        let mut after = before.clone();
        change(&mut after)?;
        if before == after {
            return Ok(false);
        }
        *self.document.borrow_mut() = after;
        self.revision.set(self.revision.get() + 1);
        self.changed.invalidate();
        Ok(true)
    }
    fn set_prompt(&self, id: &str, prompt: String, cursor: usize) -> Result<u64, ClientError> {
        let id = id.to_owned();
        self.update(Box::new(move |doc| {
            web_models::set_prompt(doc, &id, &prompt);
            doc["draftsByThreadKey"][&id]["cursor"] =
                json!(cursor.min(prompt.encode_utf16().count()));
            Ok(())
        }))?;
        Ok(self.revision.get())
    }
    fn load(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        futures::future::ready(if self.deny.get() {
            Err(PlatformError::Denied("draft".into()).into())
        } else {
            Ok(())
        })
        .boxed_local()
    }
    fn clear_revision(&self, id: &str, expected: u64) -> Result<bool, ClientError> {
        if self.revision.get() != expected {
            return Ok(false);
        }
        let id = id.to_owned();
        self.update(Box::new(move |doc| {
            web_models::clear_content(doc, &id);
            Ok(())
        }))
    }
    fn flush(&self) -> LocalBoxFuture<'static, Result<bool, ClientError>> {
        futures::future::ready(Ok(true)).boxed_local()
    }
    fn attachments(&self, _: &ComposerTarget) -> Result<AttachmentSession, ClientError> {
        Err(ClientError::Unsupported("test attachment intake"))
    }
}
fn setup() -> (
    Harness,
    ClientRuntime<TestPlatform>,
    NewThreadRuntime<TestPlatform>,
    Rc<FakeDrafts>,
) {
    let mut h = Harness::new("env");
    h.server()
        .state
        .borrow_mut()
        .extra_methods
        .push(("orchestration.launchThread", t3_rpc::CallKind::Unary));
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(|request| {
        if request.tag == "orchestration.subscribeShell" {
            let mut shell = shell_snapshot(&["t1", "t2"]);
            let mut second = shell["projects"][0].clone();
            second["id"] = json!("other");
            second["title"] = json!("Other");
            second["workspaceRoot"] = json!("/workspace/other");
            shell["projects"].as_array_mut().unwrap().push(second);
            Some(t3_rpc::HandlerOutput::Stream(
                futures::stream::iter([Ok(vec![
                    json!({"kind":"snapshot","snapshot":shell}),
                    json!({"kind":"synchronized"}),
                ])])
                .chain(futures::stream::pending())
                .boxed_local(),
            ))
        } else {
            None
        }
    }));
    let c = ClientRuntime::new(
        h.platform.clone(),
        ClientStore::default(),
        t3_contracts::orchestration_v2::OrchestrationV2CreationSource::Web,
        h.spawner(),
    );
    h.run(c.pair(PairingInput {
        pairing_url: Some(format!("{ORIGIN}/pair#token=pairing-token")),
        ..Default::default()
    }))
    .unwrap();
    h.settle();
    let drafts = Rc::new(FakeDrafts::default());
    let service = NewThreadRuntime::with_drafts(
        c.clone(),
        h.platform.clone(),
        Rc::new(NewThreadState::default()),
        drafts.clone(),
    );
    (h, c, service, drafts)
}
fn project(id: &str) -> ProjectKey {
    ProjectKey {
        environment: EnvironmentId("env".into()),
        project: ProjectId(id.into()),
    }
}

#[test]
fn empty_logical_drafts_are_reused_while_invested_drafts_remain_reachable() {
    let (mut h, _client, s, d) = setup();
    let first = h
        .run({
            let service = s.clone();
            async move { service.open_project(project("project"), None, None).await }
        })
        .unwrap();
    let reused = h
        .run({
            let service = s.clone();
            async move { service.open_project(project("project"), None, None).await }
        })
        .unwrap();
    assert_eq!(first, reused);
    h.server().state.borrow_mut().fail_random = true;
    let handle = s.clone();
    assert_eq!(
        h.run(async move { handle.open_project(project("project"), None, None).await })
            .unwrap(),
        first
    );
    h.server().state.borrow_mut().fail_random = false;
    s.set_prompt(&first, "keep my draft".into(), 4).unwrap();
    let second = h
        .run({
            let service = s.clone();
            async move { service.open_project(project("project"), None, None).await }
        })
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(d.view(&first).prompt.as_ref(), "keep my draft");
    assert!(
        d.document.borrow()["draftThreadsByThreadKey"]
            .get(&first)
            .is_some()
    );
}
#[test]
fn project_retarget_preserves_content_and_empty_drafts_open_the_destinations_existing_session() {
    let (mut h, _client, s, d) = setup();
    let source = h
        .run({
            let service = s.clone();
            async move { service.open_project(project("project"), None, None).await }
        })
        .unwrap();
    let destination = h
        .run({
            let service = s.clone();
            async move { service.open_project(project("other"), None, None).await }
        })
        .unwrap();
    h.run({
        let service = s.clone();
        let id = source.clone();
        async move { service.select_project(id, project("other")).await }
    })
    .unwrap();
    assert_eq!(
        h.run({
            let service = s.clone();
            async move { service.resolve(&Target::Draft(source)).await }
        })
        .unwrap(),
        NewThreadView::Navigate(t3_client::navigation::new_thread::Destination::Draft(
            destination
        ))
    );
    let invested = h
        .run({
            let service = s.clone();
            async move { service.open_project(project("project"), None, None).await }
        })
        .unwrap();
    s.set_prompt(&invested, "typed 😀 prompt".into(), 9)
        .unwrap();
    h.run({
        let service = s.clone();
        let id = invested.clone();
        async move { service.select_project(id, project("other")).await }
    })
    .unwrap();
    assert_eq!(d.view(&invested).prompt.as_ref(), "typed 😀 prompt");
    assert_eq!(
        d.document.borrow()["draftThreadsByThreadKey"][&invested]["projectId"],
        "other"
    );
}
#[test]
fn denied_authority_retains_saved_content_and_cannot_create_a_session() {
    let (mut h, _client, s, d) = setup();
    let id = h
        .run({
            let service = s.clone();
            async move { service.open_project(project("project"), None, None).await }
        })
        .unwrap();
    s.set_prompt(&id, "saved".into(), 5).unwrap();
    let before = d.document.borrow().clone();
    d.deny.set(true);
    assert!(matches!(
        h.run({
            let service = s.clone();
            async move { service.open_project(project("other"), None, None).await }
        }),
        Err(ClientError::Platform(PlatformError::Denied(_)))
    ));
    assert_eq!(*d.document.borrow(), before);
}
#[test]
fn prompt_edits_use_authoritative_revisions_and_stale_send_does_not_enqueue() {
    let (mut h, c, s, d) = setup();
    let id = h
        .run({
            let service = s.clone();
            async move { service.open_project(project("project"), None, None).await }
        })
        .unwrap();
    let old = s.set_prompt(&id, "first".into(), 5).unwrap();
    let current = s.set_prompt(&id, "second".into(), 6).unwrap();
    assert!(current > old);
    assert!(
        h.run({
            let service = s.clone();
            let id = id.clone();
            async move {
                service
                    .submit(id, old, t3_client::operations::ComposerDispatchMode::Auto)
                    .await
            }
        })
        .is_err()
    );
    assert_eq!(d.view(&id).prompt.as_ref(), "second");
    assert!(c.pending_messages(&s.draft_key(&id).unwrap()).is_empty());
}

#[test]
fn revoked_destination_permission_refuses_submission_without_changing_the_draft() {
    let (mut h, c, s, d) = setup();
    let handle = s.clone();
    let id = h
        .run(async move { handle.open_project(project("project"), None, None).await })
        .unwrap();
    let revision = s
        .set_prompt(&id, "keep after revocation".into(), 4)
        .unwrap();
    c.client(&EnvironmentId("env".into()))
        .unwrap()
        .session()
        .unwrap()
        .update_grants(t3_client::permissions::SessionGrants {
            valid: true,
            scopes: ["orchestration:read".into()].into(),
        });
    let handle = s.clone();
    let draft_id = id.clone();
    assert!(matches!(
        h.run(async move {
            handle
                .submit(
                    draft_id,
                    revision,
                    t3_client::operations::ComposerDispatchMode::Auto,
                )
                .await
        }),
        Err(ClientError::PermissionDenied { .. })
    ));
    assert_eq!(d.view(&id).prompt.as_ref(), "keep after revocation");
    assert!(c.pending_messages(&s.draft_key(&id).unwrap()).is_empty());
}

#[test]
fn production_authority_restores_a_rejected_creation_and_releases_busy() {
    let (mut h, c, _, _) = setup();
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(|request| {
        (request.tag=="orchestration.launchThread").then(|| {
        let rejection=json!({"_tag":"OrchestrationV2ThreadLaunchError","commandId":request.payload["commandId"],"projectId":"project","message":"provider unavailable"});
        t3_rpc::HandlerOutput::Unary(async move {t3_rpc::Exit::fail(rejection)}.boxed_local())
    })
    }));
    let s = c.new_threads();
    let handle = s.clone();
    let id = h
        .run(async move { handle.open_project(project("project"), None, None).await })
        .unwrap();
    let draft_id = id.clone();
    c.composer_drafts()
        .update(move |doc| {
            web_models::ModelPicks::default().set(
                doc,
                &draft_id,
                Some(
                    &serde_json::from_value(json!({"instanceId":"codex","model":"gpt-5.4"}))
                        .unwrap(),
                ),
                true,
                true,
            );
            Ok(())
        })
        .unwrap();
    let revision = s
        .set_prompt(&id, "restore after refusal".into(), 5)
        .unwrap();
    let handle = s.clone();
    let draft_id = id.clone();
    let result = h.run(async move {
        handle
            .submit(
                draft_id,
                revision,
                t3_client::operations::ComposerDispatchMode::Auto,
            )
            .await
    });
    assert!(
        matches!(result, Err(ClientError::Rejected { .. })),
        "{result:?}"
    );
    assert_eq!(
        c.composer_drafts().view_key(&id).prompt.as_ref(),
        "restore after refusal"
    );
    let handle = s.clone();
    let view = h
        .run(async move { handle.resolve(&Target::Draft(id)).await })
        .unwrap();
    assert!(matches!(view,NewThreadView::Draft(view) if !view.busy && view.banner.is_some()));
}

#[test]
fn draft_stream_is_current_first_and_updates_after_authoritative_edits() {
    let (mut h, _, s, _) = setup();
    let handle = s.clone();
    let id = h
        .run(async move { handle.open_project(project("project"), None, None).await })
        .unwrap();
    let mut changes = s.watch(Target::Draft(id.clone()));
    let (first, returned) = h.run(async move { (changes.next().await, changes) });
    changes = returned;
    assert!(matches!(first.as_deref(),Some(NewThreadView::Draft(view)) if view.prompt.is_empty()));
    s.set_prompt(&id, "new stream content".into(), 18).unwrap();
    let next=h.run(async move {loop {let next=changes.next().await.unwrap();if matches!(next.as_ref(),NewThreadView::Draft(view) if view.prompt=="new stream content"){break next;}}});
    assert!(matches!(next.as_ref(),NewThreadView::Draft(view) if view.cursor==18));
}

#[test]
fn server_origin_carries_its_shell_model_without_a_composer_override() {
    let (mut h, _, s, d) = setup();
    let handle = s.clone();
    let id = h
        .run(async move {
            handle
                .open_project(project("other"), Some("env:t1"), None)
                .await
        })
        .unwrap();
    assert_eq!(
        d.document.borrow()["draftsByThreadKey"][&id]["modelSelectionByProvider"]["codex"]["model"],
        "gpt-5.4"
    );
}

#[test]
fn creation_navigates_only_after_accepted_delivery_and_authoritative_shell_projection() {
    let (mut h, c, _, _) = setup();
    let s = c.new_threads();
    let handle = s.clone();
    let id = h
        .run(async move { handle.open_project(project("project"), None, None).await })
        .unwrap();
    let draft_id = id.clone();
    c.composer_drafts()
        .update(move |doc| {
            web_models::ModelPicks::default().set(
                doc,
                &draft_id,
                Some(
                    &serde_json::from_value(json!({"instanceId":"codex","model":"gpt-5.4"}))
                        .unwrap(),
                ),
                true,
                true,
            );
            Ok(())
        })
        .unwrap();
    let key = s.draft_key(&id).unwrap();
    let thread = key.thread.0.clone();
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(move |request| {
        if request.tag == "orchestration.launchThread" {
            let projection = thread_snapshot(&thread)["projection"].clone();
            let answer =
                json!({"threadId":thread,"projection":projection,"resumed":false,"sequence":20});
            Some(t3_rpc::HandlerOutput::Unary(
                async move { t3_rpc::Exit::success(answer) }.boxed_local(),
            ))
        } else if request.tag == "orchestration.subscribeShell" {
            let mut shell = shell_snapshot(&["t1", "t2", &thread]);
            shell["snapshotSequence"] = json!(20);
            shell["threads"][2]["latestUserMessageAt"] = json!("2026-11-14T00:00:00.000Z");
            Some(t3_rpc::HandlerOutput::Stream(
                futures::stream::iter([Ok(vec![
                    json!({"kind":"snapshot","snapshot":shell}),
                    json!({"kind":"synchronized"}),
                ])])
                .chain(futures::stream::pending())
                .boxed_local(),
            ))
        } else {
            None
        }
    }));
    let revision = s
        .set_prompt(&id, "create through the real service".into(), 4)
        .unwrap();
    let send = c.messages(&key.environment).unwrap();
    let client = c.client(&key.environment).unwrap();
    let handle = s.clone();
    let draft_id = id.clone();
    let result = h.run(async move {
        let operation = handle.submit(
            draft_id,
            revision,
            t3_client::operations::ComposerDispatchMode::Auto,
        );
        let project = async move {
            let mut changes = send.changes();
            while changes.next().await.is_some() {
                if send.pending_messages(&key).iter().any(|m| {
                    matches!(
                        m.state,
                        t3_client::thread_views::composer::send::SendState::Accepted { .. }
                    )
                }) {
                    client.reconnect().unwrap();
                    break;
                }
                // Accepted rows leave pending_messages; the shell resubscription supplies final evidence.
                if !send.pending_messages(&key).is_empty() {
                    continue;
                }
                if !c.composer_drafts().view_key(&id).prompt.is_empty() {
                    continue;
                }
                client.reconnect().unwrap();
                break;
            }
        };
        futures::join!(operation, project).0
    });
    assert!(result.is_ok(), "{result:?}");
}
