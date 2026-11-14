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
    checkout_urls: RefCell<Vec<Option<String>>>,
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
            checkout_urls: RefCell::default(),
        }
    }
}
impl DraftAuthority for FakeDrafts {
    fn creation(
        &self,
        mut creation: t3_client::thread_views::outbox::QueuedCreation,
        checkout_url: Option<String>,
    ) -> Result<t3_client::thread_views::outbox::QueuedCreation, ClientError> {
        self.checkout_urls.borrow_mut().push(checkout_url.clone());
        creation.checkout_pull_request_url = checkout_url;
        if creation.checkout_pull_request_url.is_some() {
            creation.pull_request_reference = None;
        }
        Ok(creation)
    }
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
    for retarget in [false, true] {
        creation_projection_scenario(retarget);
    }
}
fn creation_projection_scenario(retarget: bool) {
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
    let draft_id = id.clone();
    c.composer_drafts()
        .update(move |doc| {
            doc["draftThreadsByThreadKey"][&draft_id]["envMode"] = json!("worktree");
            doc["draftThreadsByThreadKey"][&draft_id]["branch"] = json!("feature");
            doc["draftThreadsByThreadKey"][&draft_id]["worktreePath"] = json!("/worktrees/pr-7");
            doc["draftThreadsByThreadKey"][&draft_id]["checkoutPullRequest"]=json!({"url":"https://github.com/acme/web/pull/7","branch":"feature","reference":"https://github.com/acme/web/pull/7"});
            Ok(())
        })
        .unwrap();
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(move |request| {
        if request.tag == "orchestration.launchThread" {
            assert_eq!(
                request.payload["workspaceStrategy"]["type"],
                "existing_worktree"
            );
            assert_eq!(
                request.payload["workspaceStrategy"]["worktreePath"],
                "/worktrees/pr-7"
            );
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
    let retained = c.composer_drafts();
    let original_id = id.clone();
    let result = h.run(async move {
        let operation = handle.submit(
            draft_id,
            revision,
            t3_client::operations::ComposerDispatchMode::Auto,
        );
        let project = async move {
            let mut changes = send.changes();
            let project = || {
                if retarget {
                    c.composer_drafts()
                        .update(|doc| {
                            doc["draftThreadsByThreadKey"][&id]["projectId"] = json!("other");
                            web_models::set_prompt(doc, &id, "newer draft content");
                            Ok(())
                        })
                        .unwrap();
                }
                client.reconnect().unwrap();
            };
            while changes.next().await.is_some() {
                if send.pending_messages(&key).iter().any(|m| {
                    matches!(
                        m.state,
                        t3_client::thread_views::composer::send::SendState::Accepted { .. }
                    )
                }) {
                    project();
                    break;
                }
                // Accepted rows leave pending_messages; the shell resubscription supplies final evidence.
                if !send.pending_messages(&key).is_empty() {
                    continue;
                }
                if !c.composer_drafts().view_key(&id).prompt.is_empty() {
                    continue;
                }
                project();
                break;
            }
        };
        futures::join!(operation, project).0
    });
    assert!(
        h.server()
            .state
            .borrow()
            .rpc_calls
            .iter()
            .any(|(tag, input)| tag == "orchestration.dispatchCommand"
                && input["type"] == "thread.pull-request.link"
                && input["number"] == 7)
    );
    if retarget {
        assert!(matches!(result, Err(ClientError::Cancelled)), "{result:?}");
        assert_eq!(
            retained.view_key(&original_id).prompt.as_ref(),
            "newer draft content"
        );
        assert_eq!(
            retained.document().borrow()["draftThreadsByThreadKey"][&original_id]["projectId"],
            "other"
        );
    } else {
        assert!(result.is_ok(), "{result:?}");
    }
}

type LinkEffects = Rc<RefCell<Vec<(String, Value)>>>;
type LinkHarness = (
    Harness,
    ClientRuntime<TestPlatform>,
    NewThreadRuntime<TestPlatform>,
    Rc<FakeDrafts>,
    LinkEffects,
);
fn link_setup() -> LinkHarness {
    use t3_contracts::rpc::{RpcMethod, WsSubscribeServerConfigRpc};
    let mut h = Harness::new("env");
    let drafts = Rc::new(FakeDrafts::default());
    let effects = LinkEffects::default();
    let mut shell = shell_snapshot(&["t1", "t2"]);
    shell["projects"][0]["repositoryIdentity"] = json!({"canonicalKey":"github.com/acme/web","locator":{"source":"git-remote","remoteName":"origin","remoteUrl":"https://github.com/acme/web.git"}});
    let shell = Rc::new(RefCell::new(shell));
    let (updates, receiver) =
        futures::channel::mpsc::unbounded::<Result<Vec<Value>, t3_rpc::Exit>>();
    let receiver = Rc::new(RefCell::new(Some(receiver)));
    let calls = effects.clone();
    let server = h.server();
    server.state.borrow_mut().extra_methods.extend([
        ("projectClone.start", t3_rpc::CallKind::Unary),
        ("projects.ensureScratch", t3_rpc::CallKind::Unary),
        ("git.preparePullRequestThread", t3_rpc::CallKind::Unary),
        ("pullRequests.linkedThreads", t3_rpc::CallKind::Unary),
        ("projects.createNew", t3_rpc::CallKind::Unary),
        ("projects.listIgnoredFiles", t3_rpc::CallKind::Unary),
    ]);
    server.state.borrow_mut().extra_rpc = Some(Rc::new(move |request| {
        let reply = |value| {
            Some(t3_rpc::HandlerOutput::Unary(
                async move { t3_rpc::Exit::success(value) }.boxed_local(),
            ))
        };
        if request.tag == WsSubscribeServerConfigRpc::TAG {
            let mut cfg = config("env");
            cfg["newProjectsRoot"] = json!("/new-projects");
            cfg["scratchWorkspaceRoot"] = json!("/scratch");
            cfg["settings"]["ghqCloneLayout"] = json!(false);
            cfg["environment"]["capabilities"]["ignoredFileListing"] = json!(true);
            cfg["environment"]["capabilities"]["ignoredFileSearch"] = json!(true);
            return Some(t3_rpc::HandlerOutput::Stream(
                futures::stream::iter([Ok(vec![
                    json!({"version":1,"type":"snapshot","config":cfg}),
                ])])
                .chain(futures::stream::pending())
                .boxed_local(),
            ));
        }
        if request.tag == "orchestration.subscribeShell" {
            let initial = vec![
                json!({"kind":"snapshot","snapshot":shell.borrow().clone()}),
                json!({"kind":"synchronized"}),
            ];
            let events = receiver.borrow_mut().take().expect("single link shell");
            return Some(t3_rpc::HandlerOutput::Stream(
                futures::stream::iter([Ok(initial)])
                    .chain(events)
                    .boxed_local(),
            ));
        }
        if request.tag == "pullRequests.linkedThreads" {
            return reply(json!({"threads":[]}));
        }
        if request.tag == "projects.listIgnoredFiles" {
            calls
                .borrow_mut()
                .push((request.tag.clone(), request.payload.clone()));
            let searched = request.payload["query"]
                .as_str()
                .is_some_and(|s| !s.is_empty());
            return reply(
                json!({"files": if searched {json!([{"path":"hidden.env","byteLength":33}])} else {json!([{"path":".env","byteLength":11},{"path":"cache.bin","byteLength":1048577}])},"truncated":!searched}),
            );
        }
        if matches!(
            request.tag.as_str(),
            "projectClone.start"
                | "projects.ensureScratch"
                | "projects.mutate"
                | "projects.createNew"
        ) {
            calls
                .borrow_mut()
                .push((request.tag.clone(), request.payload.clone()));
            let mut project = shell.borrow()["projects"][0].clone();
            let clone = request.tag == "projectClone.start";
            let create_new = request.tag == "projects.createNew";
            let mutate = request.tag == "projects.mutate";
            project["id"] = if create_new {
                json!("created-project")
            } else if clone || mutate {
                request.payload["projectId"].clone()
            } else {
                json!("scratch")
            };
            project["workspaceRoot"] = if create_new {
                json!("/new-projects/garden-example")
            } else if mutate {
                request.payload["workspaceRoot"].clone()
            } else if clone {
                request.payload["destinationPath"].clone()
            } else {
                json!("/scratch")
            };
            project["repositoryIdentity"] = if clone {
                json!({"canonicalKey":"github.com/acme/missing","locator":{"source":"git-remote","remoteName":"origin","remoteUrl":"https://github.com/acme/missing.git"}})
            } else {
                Value::Null
            };
            if create_new {
                project["title"] = request.payload["name"].clone();
            }
            shell.borrow_mut()["projects"]
                .as_array_mut()
                .unwrap()
                .push(project.clone());
            let sequence = shell.borrow()["snapshotSequence"].as_u64().unwrap() + 1;
            shell.borrow_mut()["snapshotSequence"] = json!(sequence);
            updates
                .unbounded_send(Ok(vec![
                    json!({"kind":"snapshot","snapshot":shell.borrow().clone()}),
                    json!({"kind":"synchronized"}),
                ]))
                .unwrap();
            return reply(if create_new {
                json!({"projectId":project["id"],"workspaceRoot":project["workspaceRoot"],"commitError":"initial commit refused"})
            } else if mutate {
                let mut response = project;
                response["deletedAt"] = Value::Null;
                response
            } else if clone {
                json!({"projectId":project["id"],"cwd":project["workspaceRoot"],"remoteUrl":request.payload["remoteUrl"],"repository":null})
            } else {
                json!({"projectId":"scratch"})
            });
        }
        if request.tag == "git.preparePullRequestThread" {
            calls
                .borrow_mut()
                .push((request.tag.clone(), request.payload.clone()));
            return reply(
                json!({"pullRequest":{"number":7,"title":"Fix","url":"https://github.com/acme/web/pull/7","baseBranch":"main","headBranch":"feature","state":"open"},"branch":"feature","worktreePath":"/worktrees/pr-7","isOnPullRequestHead":true}),
            );
        }
        None
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
    c.client(&environment("env"))
        .unwrap()
        .session()
        .unwrap()
        .update_grants(t3_client::permissions::SessionGrants {
            valid: true,
            scopes: [
                "orchestration:read",
                "orchestration:operate",
                "filesystem:read",
                "filesystem:write",
                "source-control:read",
                "source-control:write",
            ]
            .map(str::to_owned)
            .into(),
        });
    let service =
        NewThreadRuntime::with_drafts(c.clone(), h.platform.clone(), Rc::default(), drafts.clone());
    (h, c, service, drafts, effects)
}
#[test]
fn repository_link_choices_clone_or_start_without_a_project_only_on_explicit_activation() {
    use t3_client::navigation::new_thread::{Destination, LinkAction};
    for action in [LinkAction::CloneRepository, LinkAction::UseWithoutProject] {
        let (mut h, _c, s, d, effects) = link_setup();
        let search = "?repo=acme/missing&prompt=Read%20the%20playbook".to_owned();
        let handle = s.clone();
        let query = search.clone();
        let view = h
            .run(async move { handle.resolve(&Target::Link(query)).await })
            .unwrap();
        assert!(
            matches!(view,NewThreadView::LinkChoice{title,choices,..} if title.key=="You don't have {name}"&&choices.len()==2&&choices.iter().all(|choice|choice.disabled_reason.is_none()))
        );
        assert!(effects.borrow().is_empty());
        let handle = s.clone();
        let result = h
            .run(async move { handle.link_action(search, action).await })
            .unwrap();
        let Some(Destination::Draft(id)) = result else {
            panic!("draft destination");
        };
        assert_eq!(d.view(&id).prompt.as_ref(), "Read the playbook");
        assert_eq!(
            effects.borrow()[0].0,
            if action == LinkAction::CloneRepository {
                "projectClone.start"
            } else {
                "projects.ensureScratch"
            }
        );
        if action == LinkAction::CloneRepository {
            assert_eq!(
                effects.borrow()[0].1["destinationPath"],
                "/new-projects/missing"
            );
        }
    }
}
#[test]
fn pull_request_link_requires_checkout_confirmation_and_carries_the_prepared_workspace() {
    use t3_client::navigation::new_thread::{Destination, LinkAction};
    let (mut h, _c, s, d, effects) = link_setup();
    let search =
        "?pr=https%3A%2F%2Fgithub.com%2Facme%2Fweb%2Fpull%2F7&prompt=Review%20this".to_owned();
    let handle = s.clone();
    let query = search.clone();
    let view = h
        .run(async move { handle.resolve(&Target::Link(query)).await })
        .unwrap();
    assert!(
        matches!(view,NewThreadView::LinkChoice{title,choices,..} if title.key=="Check out {name}?"&&choices.len()==1&&choices[0].action==LinkAction::CheckoutPullRequest&&choices[0].label.key=="Check out")
    );
    assert!(effects.borrow().is_empty());
    let handle = s.clone();
    let Some(Destination::Draft(id)) = h
        .run(async move {
            handle
                .link_action(search, LinkAction::CheckoutPullRequest)
                .await
        })
        .unwrap()
    else {
        panic!("checkout draft");
    };
    assert_eq!(d.view(&id).prompt.as_ref(), "Review this");
    assert_eq!(
        d.document.borrow()["draftThreadsByThreadKey"][&id]["worktreePath"],
        "/worktrees/pr-7"
    );
    assert_eq!(effects.borrow()[0].0, "git.preparePullRequestThread");
    assert_eq!(effects.borrow()[0].1["mode"], "worktree");
}
#[test]
fn replacing_an_existing_thread_prompt_requires_an_explicit_choice_and_retains_other_content() {
    use t3_client::navigation::new_thread::{Destination, LinkAction};
    let (mut h, c, s, d, _) = link_setup();
    let fallback = h.server().state.borrow().extra_rpc.clone();
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(move |request| {
        (request.tag=="pullRequests.linkedThreads").then(|| t3_rpc::HandlerOutput::Unary(async {t3_rpc::Exit::success(json!({"threads":[{"id":"t1","projectId":"project","title":"Existing","archivedAt":null}]}))}.boxed_local()))
            .or_else(|| fallback.as_ref().and_then(|handler| handler(request)))
    }));
    s.set_prompt("env:t1", "keep until confirmed".into(), 4)
        .unwrap();
    d.update(Box::new(|doc| {
        doc["draftsByThreadKey"]["env:t1"]["files"] = json!([{"id":"keep-file"}]);
        Ok(())
    }))
    .unwrap();
    let query =
        "?pr=https%3A%2F%2Fgithub.com%2Facme%2Fweb%2Fpull%2F7&prompt=Replace%20me".to_owned();
    let handle = s.clone();
    let search = query.clone();
    let view = h
        .run(async move { handle.resolve(&Target::Link(search)).await })
        .unwrap();
    assert!(
        matches!(&view,NewThreadView::LinkChoice{choices,..} if choices[0].action==LinkAction::ReplaceDraft),
        "{view:?}"
    );
    assert_eq!(d.view("env:t1").prompt.as_ref(), "keep until confirmed");
    let handle = s.clone();
    assert!(
        matches!(h.run(async move{handle.link_action(query,LinkAction::ReplaceDraft).await}).unwrap(),Some(Destination::Thread(key)) if key.thread.0=="t1")
    );
    assert_eq!(d.view("env:t1").prompt.as_ref(), "Replace me");
    assert_eq!(
        d.document.borrow()["draftsByThreadKey"]["env:t1"]["files"][0]["id"],
        "keep-file"
    );
    assert!(
        c.pending_messages(&t3_client::types::ThreadKey {
            environment: environment("env"),
            thread: thread("t1")
        })
        .is_empty()
    );
}

#[test]
fn link_clone_rechecks_revoked_permissions_and_retains_the_choice_for_retry() {
    use t3_client::navigation::new_thread::LinkAction;
    let (mut h, c, s, _, effects) = link_setup();
    let query = "?repo=acme/missing&prompt=Read%20this".to_owned();
    let service = s.clone();
    let search = query.clone();
    h.run(async move { service.resolve(&Target::Link(search)).await })
        .unwrap();
    c.client(&environment("env"))
        .unwrap()
        .session()
        .unwrap()
        .update_grants(t3_client::permissions::SessionGrants {
            valid: true,
            scopes: ["orchestration:read".to_owned()].into(),
        });
    let service = s.clone();
    let search = query.clone();
    let error = h
        .run(async move {
            service
                .link_action(search, LinkAction::CloneRepository)
                .await
        })
        .unwrap_err();
    assert!(
        matches!(error, ClientError::PermissionDenied { .. }),
        "{error:?}"
    );
    assert!(effects.borrow().is_empty());
    let view = h
        .run(async move { s.resolve(&Target::Link(query)).await })
        .unwrap();
    assert!(matches!(view, NewThreadView::LinkChoice { choices, .. }
        if choices.len() == 2 && choices.iter().all(|choice| choice.disabled_reason.is_none())));
}

#[test]
fn pending_checkout_disables_reentry_and_cancellation_restores_the_source_choice() {
    use t3_client::navigation::new_thread::LinkAction;
    let (mut h, _, s, _, _) = link_setup();
    let query = "?pr=https%3A%2F%2Fgithub.com%2Facme%2Fweb%2Fpull%2F7".to_owned();
    let service = s.clone();
    let search = query.clone();
    h.run(async move { service.resolve(&Target::Link(search)).await })
        .unwrap();
    let calls = Rc::new(Cell::new(0));
    let recorded = calls.clone();
    let (started, waiting) = futures::channel::oneshot::channel();
    let started = RefCell::new(Some(started));
    let fallback = h.server().state.borrow().extra_rpc.clone();
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(move |request| {
        (request.tag == "git.preparePullRequestThread")
            .then(|| {
                recorded.set(recorded.get() + 1);
                started.borrow_mut().take().unwrap().send(()).unwrap();
                t3_rpc::HandlerOutput::Unary(futures::future::pending().boxed_local())
            })
            .or_else(|| fallback.as_ref().and_then(|handler| handler(request)))
    }));
    let service = s.clone();
    h.run(async move {
        let operation = service.link_action(query.clone(), LinkAction::CheckoutPullRequest);
        let check = async {
            waiting.await.unwrap();
            let view = service.resolve(&Target::Link(query.clone())).await.unwrap();
            assert!(matches!(view, NewThreadView::LinkChoice { choices, .. }
                if choices[0].label.key == "Checking out..." && choices[0].disabled_reason.is_some()));
            assert!(matches!(service.link_action(query.clone(), LinkAction::CheckoutPullRequest).await,
                Err(ClientError::Cancelled)));
        };
        futures::pin_mut!(operation, check);
        if let futures::future::Either::Right((result, _)) = futures::future::select(check, operation).await {
            panic!("checkout completed before the pending reply: {result:?}");
        }
    });
    assert_eq!(calls.get(), 1);
    let view = h
        .run(async move {
            s.resolve(&Target::Link(
                "?pr=https%3A%2F%2Fgithub.com%2Facme%2Fweb%2Fpull%2F7".into(),
            ))
            .await
        })
        .unwrap();
    assert!(matches!(view, NewThreadView::LinkChoice { choices, .. }
        if choices[0].label.key == "Check out" && choices[0].disabled_reason.is_none()));
}

#[test]
fn add_project_reuses_a_project_and_creates_local_or_new_projects_from_real_projections() {
    use t3_client::navigation::{new_thread::Destination, project_flow::Step};
    for mode in ["existing", "local", "new"] {
        let (mut h, c, _, _, effects) = link_setup();
        let session = c.add_project();
        let service = session.clone();
        let view = h.run(async move { service.resolve().await });
        assert_eq!(view.step, Step::Sources);
        let service = session.clone();
        let selection = if mode == "new" { "new" } else { "local" };
        assert!(
            h.run(async move { service.select_item(selection).await })
                .unwrap()
                .is_none()
        );
        let path = match mode {
            "existing" => "/workspace/project/",
            "local" => "/workspace/fresh",
            _ => "Garden Example",
        };
        session.set_query(path.into()).unwrap();
        let service = session.clone();
        let view = h.run(async move { service.resolve().await });
        assert!(view.can_submit, "{view:?}");
        assert!(!view.can_pick_folder);
        let service = session.clone();
        let result = h
            .run(async move { service.submit(view.revision).await })
            .unwrap()
            .unwrap();
        if mode == "existing" {
            assert!(matches!(result, Destination::Thread(_)));
            assert!(effects.borrow().is_empty());
        } else {
            let Destination::Draft(id) = result else {
                panic!("created project must open a client draft")
            };
            let document = c.composer_drafts().document();
            let document = document.borrow();
            let project = document["draftThreadsByThreadKey"][&id]["projectId"]
                .as_str()
                .unwrap();
            assert!(
                c.store()
                    .shared_shell(&environment("env"))
                    .unwrap()
                    .projects
                    .iter()
                    .any(|p| p.id.0 == project)
            );
            assert_eq!(
                effects.borrow()[0].0,
                if mode == "new" {
                    "projects.createNew"
                } else {
                    "projects.mutate"
                }
            );
            if mode == "new" {
                assert!(
                    session
                        .notices()
                        .iter()
                        .any(|n| n.warning && n.description.contains("initial commit refused"))
                );
            }
        }
    }
}

#[test]
fn add_project_rejects_stale_or_denied_submissions_and_retains_input() {
    let (mut h, c, _, _, effects) = link_setup();
    let session = c.add_project();
    let service = session.clone();
    h.run(async move { service.resolve().await });
    let service = session.clone();
    h.run(async move { service.select_item("local").await })
        .unwrap();
    session.set_query("/workspace/first".into()).unwrap();
    let service = session.clone();
    let view = h.run(async move { service.resolve().await });
    session.set_query("/workspace/kept".into()).unwrap();
    let service = session.clone();
    assert!(matches!(
        h.run(async move { service.submit(view.revision).await }),
        Err(ClientError::Cancelled)
    ));
    c.client(&environment("env"))
        .unwrap()
        .session()
        .unwrap()
        .update_grants(t3_client::permissions::SessionGrants {
            valid: true,
            scopes: ["orchestration:read".into()].into(),
        });
    let service = session.clone();
    let view = h.run(async move { service.resolve().await });
    let service = session.clone();
    let result = h.run(async move { service.submit(view.revision).await });
    assert!(
        matches!(result, Err(ClientError::PermissionDenied { .. })),
        "{result:?}"
    );
    assert!(effects.borrow().is_empty());
    let service = session.clone();
    let view = h.run(async move { service.resolve().await });
    assert_eq!(view.query, "/workspace/kept");
    assert!(view.error.is_some());
}

#[test]
fn clone_step_stays_open_then_accepted_start_completes_and_retains_background_work() {
    use t3_client::navigation::project_flow::Step;
    let (mut h, c, _, _, effects) = link_setup();
    let session = c.add_project();
    let service = session.clone();
    h.run(async move { service.resolve().await });
    let service = session.clone();
    h.run(async move { service.select_item("source:url").await })
        .unwrap();
    session
        .set_query("https://github.com/acme/other.git".into())
        .unwrap();
    let service = session.clone();
    let view = h.run(async move { service.resolve().await });
    let mut completion = session.completed();
    let service = session.clone();
    assert!(
        h.run(async move { service.submit(view.revision).await })
            .unwrap()
            .is_none()
    );
    assert!(completion.next().now_or_never().is_none());
    let service = session.clone();
    let view = h.run(async move { service.resolve().await });
    assert_eq!(view.step, Step::CloneDestination);
    assert!(view.query.ends_with("other"));
    let service = session.clone();
    assert!(
        h.run(async move { service.submit(view.revision).await })
            .unwrap()
            .is_none()
    );
    assert_eq!(h.run(async move { completion.next().await }), Some(()));
    drop(session);
    h.settle();
    assert_eq!(
        effects
            .borrow()
            .iter()
            .filter(|(tag, _)| tag == "projectClone.start")
            .count(),
        1
    );
    let id = effects
        .borrow()
        .iter()
        .find(|(tag, _)| tag == "projectClone.start")
        .unwrap()
        .1["projectId"]
        .clone();
    assert!(
        c.composer_drafts().document().borrow()["draftThreadsByThreadKey"]
            .as_object()
            .unwrap()
            .values()
            .any(|d| d["projectId"] == id)
    );
}

#[test]
fn ignored_file_search_preserves_offscreen_selection_and_denied_reads_are_visible() {
    for denied in [false, true] {
        let (mut h, c, _, _, effects) = link_setup();
        if denied {
            c.client(&environment("env"))
                .unwrap()
                .session()
                .unwrap()
                .update_grants(t3_client::permissions::SessionGrants {
                    valid: true,
                    scopes: ["orchestration:read".into()].into(),
                });
        }
        let session = c.ignored_files(project("project"));
        let mut views = session.watch();
        let initial = h
            .run(async move {
                let view = views.next().await.unwrap();
                (view, views)
            })
            .0;
        assert!(if denied {
            initial.error.is_some()
        } else {
            initial.loading
        });
        let mut views = session.watch();
        let (view, mut views) = h.run(async move {
            loop {
                let view = views.next().await.unwrap();
                if !view.loading {
                    break (view, views);
                }
            }
        });
        if denied {
            assert!(view.error.is_some());
            assert!(
                effects
                    .borrow()
                    .iter()
                    .all(|(tag, _)| tag != "projects.listIgnoredFiles")
            );
            continue;
        }
        assert_eq!(view.selected_count, 1);
        assert_eq!(view.selected_bytes, 11);
        assert!(view.files[1].disabled);
        assert!(session.toggle_file("cache.bin", true).is_err());
        session.set_query("hidden".into()).unwrap();
        let (done, ready) = futures::channel::oneshot::channel();
        h.spawn(async move {
            while let Some(view) = views.next().await {
                if view
                    .files
                    .iter()
                    .any(|file| file.file.path.0 == "hidden.env")
                {
                    done.send(view).unwrap();
                    break;
                }
            }
        });
        h.settle();
        h.advance(250);
        let found = h.run(async move { ready.await.unwrap() });
        assert_eq!(found.selected_count, 1);
        session.toggle_file("hidden.env", true).unwrap();
        assert_eq!(
            session
                .selected()
                .iter()
                .map(|f| f.byte_length)
                .sum::<u64>(),
            44
        );
        session.set_query(String::new()).unwrap();
        let restored = session.snapshot();
        assert_eq!(restored.selected_count, 2);
        assert_eq!(restored.selected_bytes, 44);
        assert!(restored.files[0].selected);
        assert_eq!(
            effects
                .borrow()
                .iter()
                .filter(|(tag, _)| tag == "projects.listIgnoredFiles")
                .count(),
            2
        );
    }
}

#[test]
fn new_project_folder_names_match_the_typescript_path_cases() {
    use t3_client::navigation::project_flow::new_project_folder_name;
    for (name, expected) in [
        ("Pinball Stats", "pinball-stats"),
        ("  Café & Crème!  ", "cafe-creme"),
        ("../../etc", "etc"),
        ("🎱🎱", "project"),
        ("Con", "con-project"),
        ("LPT1", "lpt1-project"),
        ("console", "console"),
    ] {
        assert_eq!(new_project_folder_name(name), expected);
    }
    assert_eq!(
        new_project_folder_name(&format!("{} b", "a".repeat(63))),
        "a".repeat(63)
    );
}
