#![allow(clippy::unwrap_used)]
use futures::{
    FutureExt, StreamExt,
    channel::{mpsc, oneshot},
    executor::block_on,
    future::LocalBoxFuture,
    stream::LocalBoxStream,
};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
};
use t3_client::{
    ClientError,
    rpc::{RpcEndpoint, RpcMethod},
    services::{filesystem::*, models::*, resources::*, runtime::Services, sessions::*},
    testing::{FakePlatform, RpcScript, ScriptedRpc},
    types::{EnvironmentId, Generation},
};
use t3_contracts::{orchestration_v2::*, rpc::WsReviewGetDiffPreviewRpc};

fn env() -> EnvironmentId {
    EnvironmentId("environment".into())
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}
fn shell() -> OrchestrationV2ThreadShell {
    decode(
        json!({"id":"thread","projectId":"project","title":"Agent: task","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-test"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"lineage":{"rootThreadId":"thread","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"activeProviderThreadId":null,"latestRunId":"run","activeRunId":"run","status":"running","pendingRuntimeRequest":null,"latestVisibleMessage":null,"latestUserMessageAt":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"createdAt":"2026-11-13T00:00:00Z","updatedAt":"2026-11-13T00:02:00Z","archivedAt":null,"settledOverride":null,"settledAt":null,"deletedAt":null,"createdBy":"user","creationSource":"web"}),
    )
}

#[test]
fn shell_models_keep_runtime_activity_distinct_from_the_latest_run_and_explicit_nulls() {
    let mut source = shell();
    source.latest_run_requested_at = Some(Some(decode(json!("2026-11-13T00:00:00Z"))));
    source.latest_run_started_at = Some(Some(decode(json!("2026-11-13T00:00:10Z"))));
    let presented = present_thread_shell(env(), Rc::new(source.clone()), None);
    assert!(thread_runtime_is_active(presented.runtime.as_ref()));
    assert!(!thread_runtime_can_archive(presented.runtime.as_ref()));
    assert_eq!(
        resolve_thread_working_started_at(
            presented.latest_run.as_ref(),
            presented.runtime.as_ref()
        ),
        Some("2026-11-13T00:00:10Z")
    );
    source.activity_run_started_at = Some(None);
    source.status = OrchestrationV2ShellThreadStatus::Completed;
    let presented = present_thread_shell(env(), Rc::new(source.clone()), None);
    assert_eq!(
        presented.latest_run.as_ref().unwrap().completed_at,
        Some(source.updated_at.clone())
    );
    assert_eq!(
        resolve_thread_working_started_at(
            presented.latest_run.as_ref(),
            presented.runtime.as_ref()
        ),
        None
    );
    source.latest_run_completed_at = Some(None);
    source.activity_run_started_at = Some(Some(decode(json!("2026-11-13T00:01:00Z"))));
    let presented = present_thread_shell(env(), Rc::new(source), None);
    assert!(
        presented
            .latest_run
            .as_ref()
            .unwrap()
            .completed_at
            .is_none()
    );
    assert_eq!(
        resolve_thread_working_started_at(
            presented.latest_run.as_ref(),
            presented.runtime.as_ref()
        ),
        Some("2026-11-13T00:01:00Z")
    );
    for (elapsed, label) in [
        (f64::NAN, "0s"),
        (-2.0, "0s"),
        (59_999.0, "59s"),
        (60_000.0, "1m"),
        (3_660_000.0, "1h 1m"),
    ] {
        assert_eq!(format_working_duration_label(elapsed), label);
    }
}

#[test]
fn shell_models_park_awaited_work_preserve_failure_and_distinguish_questions_from_auth_refresh() {
    let mut source = shell();
    source.pending_background_tasks = Some(Some(decode(
        json!([{"kind":"command","taskId":"work","awaited":true}]),
    )));
    source.provider_instance_history = Some(Some(decode(json!([
        "claudeAgent",
        "codex",
        "cursor",
        "grok"
    ]))));
    let presented = present_thread_shell(
        env(),
        Rc::new(source.clone()),
        Some(PendingSubagentRequest::Approval),
    );
    assert_eq!(
        presented.runtime.as_ref().unwrap().status,
        OrchestrationV2ShellThreadStatus::Idle
    );
    assert_eq!(
        resolve_thread_provider_stack(&presented),
        decode::<Vec<t3_contracts::provider_instance::ProviderInstanceId>>(json!([
            "cursor", "grok", "codex"
        ]))
    );
    source.status = OrchestrationV2ShellThreadStatus::Failed;
    for (kind, approvals, questions) in [
        ("user_input", false, true),
        ("auth_refresh", false, false),
        ("command", true, false),
    ] {
        source.pending_runtime_request = Some(decode(
            json!({"id":"request","kind":kind,"createdAt":"2026-11-13T00:00:00Z"}),
        ));
        let presented = present_thread_shell(env(), Rc::new(source.clone()), None);
        assert_eq!(presented.has_pending_approvals, approvals);
        assert_eq!(presented.has_pending_user_input, questions);
        assert_eq!(presented.pending_user_input_at.is_some(), questions);
        assert_eq!(
            presented.runtime.as_ref().unwrap().status,
            OrchestrationV2ShellThreadStatus::Failed
        );
    }
}

fn preview(incomplete: bool, at: &str) -> Value {
    json!({"cwd":"/repo","generatedAt":at,"sources":[],"incomplete":incomplete})
}
#[test]
fn review_tracked_preview_is_provisional_and_only_the_final_answer_runs_the_hook() {
    block_on(async {
        let services = Services::default();
        let rpc = ScriptedRpc::default();
        let platform = FakePlatform::default();
        let input = decode::<t3_contracts::review::ReviewDiffPreviewInput>(
            json!({"cwd":"/repo","sourceKind":"working-tree"}),
        );
        let mut tracked = json!(input);
        tracked["trackedOnly"] = json!(true);
        rpc.scripts.borrow_mut().extend([
            RpcScript {
                method: "review.getDiffPreview".into(),
                input: tracked,
                replies: vec![Ok(preview(true, "2026-11-13T00:00:00Z"))],
            },
            RpcScript {
                method: "review.getDiffPreview".into(),
                input: json!(input),
                replies: vec![Ok(preview(false, "2026-11-13T00:00:01Z"))],
            },
        ]);
        let resource = services
            .query::<WsReviewGetDiffPreviewRpc>(env(), &input)
            .unwrap();
        let answers = Cell::new(0);
        services
            .review
            .preview(&services, env(), &rpc, &platform, input.clone(), |answer| {
                assert_eq!(answer.incomplete, Some(false));
                assert_eq!(
                    resource.snapshot().value.as_ref().unwrap().incomplete,
                    Some(true)
                );
                answers.set(answers.get() + 1);
            })
            .await
            .unwrap();
        assert_eq!(answers.get(), 1);
        assert!(!resource.snapshot().loading);
        rpc.scripts.borrow_mut().push_back(RpcScript {
            method: "review.getDiffPreview".into(),
            input: json!(input),
            replies: vec![Ok(preview(false, "2026-11-13T00:00:02Z"))],
        });
        services
            .review
            .preview(&services, env(), &rpc, &platform, input, |_| {})
            .await
            .unwrap();
        assert!(rpc.scripts.borrow().is_empty());
    });
}

#[test]
fn metadata_maintenance_only_renews_misses_while_a_path_stays_mounted() {
    let service = FileMetadataService::default();
    service.begin_session(env(), Generation(1));
    let known = service.enqueue(env(), "/known", None, 0);
    let missing = service.enqueue(env(), "/missing", None, 0);
    let _known_reader = known.observe(0);
    let _missing_reader = missing.observe(0);
    let batch = service.next_batch(&env()).unwrap();
    assert_eq!(batch.paths, vec!["/known", "/missing"]);
    service.complete(batch, vec![Some(decode(json!({"kind":"file"}))), None], 0);
    service.collect(60_000);
    assert_eq!(service.next_batch(&env()).unwrap().paths, vec!["/missing"]);
}

#[test]
fn established_session_count_changes_for_new_endpoints_even_when_generation_is_reused() {
    let mut seen = EstablishedSessions::default();
    let rpc = Rc::new(PendingRpc::default());
    let session = connection(rpc.clone(), 1).session.unwrap();
    assert_eq!(
        seen.observe(QueryConnectionPhase::Connected, Some(session.clone())),
        1
    );
    for phase in [
        QueryConnectionPhase::Connecting,
        QueryConnectionPhase::Backoff,
        QueryConnectionPhase::Available,
    ] {
        assert_eq!(seen.observe(phase, None), 1);
    }
    assert_eq!(
        seen.observe(QueryConnectionPhase::Connected, Some(session)),
        1
    );
    assert_eq!(
        seen.observe(
            QueryConnectionPhase::Connected,
            connection(Rc::new(PendingRpc::default()), 1).session
        ),
        2
    );
}

#[test]
fn config_shared_views_reuse_unchanged_settings_sections_and_provider_lists() {
    use t3_client::services::config::*;
    use t3_contracts::server::*;
    let mut config: ServerConfig = serde_json::from_str(
        include_str!("../../../fixtures/samples/ServerConfig.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    config.settings.provider_instances.insert(
        decode(json!("codex")),
        decode(json!({"driver":"codex","config":{}})),
    );
    let first = SharedServerConfig::update(Rc::new(config.clone()), None);
    config.settings.hide_tagged_threads = !config.settings.hide_tagged_threads;
    let next = SharedServerConfig::update(Rc::new(config.clone()), Some(&first));
    assert!(!Rc::ptr_eq(&first.settings, &next.settings));
    assert!(Rc::ptr_eq(&first.favorites, &next.favorites));
    assert!(Rc::ptr_eq(
        &first.provider_instances[&decode(json!("codex"))],
        &next.provider_instances[&decode(json!("codex"))]
    ));
    assert!(Rc::ptr_eq(&first.providers, &next.providers));
    if let Some(provider) = config.providers.0.first_mut() {
        provider.skills.push(decode(
            json!({"name":"review","path":"/skills/review","enabled":true}),
        ));
        let changed = SharedServerConfig::update(Rc::new(config), Some(&next));
        assert!(Rc::ptr_eq(
            &next.providers[0].models,
            &changed.providers[0].models
        ));
        assert!(!Rc::ptr_eq(
            &next.providers[0].skills,
            &changed.providers[0].skills
        ));
    }
}

struct Number;
impl RpcMethod for Number {
    type Input = u64;
    type Output = u64;
    const NAME: &'static str = "number";
}
#[derive(Default)]
struct PendingRpc {
    calls: Cell<usize>,
    replies: RefCell<VecDeque<oneshot::Receiver<Value>>>,
}
impl PendingRpc {
    fn enqueue(&self) -> oneshot::Sender<Value> {
        let (tx, rx) = oneshot::channel();
        self.replies.borrow_mut().push_back(rx);
        tx
    }
}
impl RpcEndpoint for PendingRpc {
    fn destination(&self) -> Option<EnvironmentId> {
        Some(env())
    }
    fn call<M: RpcMethod>(
        &self,
        _: M::Input,
    ) -> LocalBoxFuture<'_, Result<M::Output, ClientError>> {
        self.calls.set(self.calls.get() + 1);
        let reply = self.replies.borrow_mut().pop_front().unwrap();
        async move {
            serde_json::from_value(reply.await.map_err(|_| ClientError::Disconnected)?)
                .map_err(Into::into)
        }
        .boxed_local()
    }
    fn subscribe<M: RpcMethod>(
        &self,
        _: M::Input,
    ) -> LocalBoxStream<'static, Result<M::Output, ClientError>> {
        futures::stream::pending::<Value>()
            .map(|value| serde_json::from_value::<M::Output>(value).map_err(Into::into))
            .boxed_local()
    }
}
fn connection(rpc: Rc<PendingRpc>, generation: u64) -> QueryConnection<PendingRpc> {
    QueryConnection {
        phase: QueryConnectionPhase::Connected,
        failure: None,
        session: Some(ServiceSession {
            identity: t3_client::environment::SessionIdentity {
                environment: env(),
                generation: Generation(generation),
                account: None,
                dpop_thumbprint: None,
            },
            endpoint: rpc,
        }),
    }
}

#[test]
fn mounted_queries_restore_offline_then_refresh_invalidate_renew_and_cancel_on_disposal() {
    let platform = FakePlatform::default();
    block_on(
        QueryPersistence {
            platform: &platform,
        }
        .save(env(), "number".into(), &4u64),
    )
    .unwrap();
    let resource = Resource::new(
        ResourcePolicy {
            refresh_ms: Some(50),
            ..Default::default()
        },
        Generation(0),
    );
    let (tx, rx) = mpsc::unbounded();
    tx.unbounded_send(QueryConnection {
        phase: QueryConnectionPhase::Offline,
        failure: None,
        session: None,
    })
    .unwrap();
    let answers = Cell::new(0);
    let mut driver = Box::pin(drive_query::<Number, PendingRpc, _>(
        &resource,
        &platform,
        env(),
        rx.boxed_local(),
        0,
        Some("number"),
        |_| {
            answers.set(answers.get() + 1);
        },
    ));
    assert!(driver.as_mut().now_or_never().is_none());
    assert_eq!(resource.snapshot().value.as_deref(), Some(&4));
    assert!(resource.snapshot().error.is_none());
    assert_eq!(answers.get(), 0);
    let rpc = Rc::new(PendingRpc::default());
    let reply = rpc.enqueue();
    tx.unbounded_send(connection(rpc.clone(), 1)).unwrap();
    assert!(driver.as_mut().now_or_never().is_none());
    assert!(resource.snapshot().loading);
    reply.send(json!(5)).unwrap();
    assert!(driver.as_mut().now_or_never().is_none());
    assert_eq!(resource.snapshot().value.as_deref(), Some(&5));
    assert_eq!(answers.get(), 1);
    let reply = rpc.enqueue();
    platform.advance(50);
    assert!(driver.as_mut().now_or_never().is_none());
    reply.send(json!(6)).unwrap();
    assert!(driver.as_mut().now_or_never().is_none());
    assert_eq!(resource.snapshot().value.as_deref(), Some(&6));
    let reply = rpc.enqueue();
    resource.invalidate();
    assert!(driver.as_mut().now_or_never().is_none());
    reply.send(json!(7)).unwrap();
    assert!(driver.as_mut().now_or_never().is_none());
    assert_eq!(resource.snapshot().value.as_deref(), Some(&7));
    platform.advance(3_000);
    drop(driver);
    assert_eq!(platform.pending_timers(), 0);
    assert!(!resource.has_readers());
    assert!(!resource.expired(3_050 + resource.policy.idle_ttl_ms - 1));
    assert!(resource.expired(3_050 + resource.policy.idle_ttl_ms));
}

#[test]
fn replacing_a_query_cancels_the_old_rpc_before_publishing_and_keeps_the_old_answer_readable() {
    let platform = FakePlatform::default();
    let resource = Resource::new(ResourcePolicy::default(), Generation(0));
    let token = resource.begin();
    resource.finish(token, 1, 0, false);
    let (tx, rx) = mpsc::unbounded();
    let old = Rc::new(PendingRpc::default());
    let old_reply = old.enqueue();
    tx.unbounded_send(connection(old, 1)).unwrap();
    let mut driver = Box::pin(drive_query::<Number, _, _>(
        &resource,
        &platform,
        env(),
        rx.boxed_local(),
        0,
        None,
        |_| {},
    ));
    assert!(driver.as_mut().now_or_never().is_none());
    let new = Rc::new(PendingRpc::default());
    let reply = new.enqueue();
    tx.unbounded_send(connection(new, 2)).unwrap();
    assert!(driver.as_mut().now_or_never().is_none());
    assert!(old_reply.send(json!(2)).is_err());
    assert_eq!(resource.snapshot().value.as_deref(), Some(&1));
    reply.send(json!(3)).unwrap();
    assert!(driver.as_mut().now_or_never().is_none());
    assert_eq!(resource.snapshot().value.as_deref(), Some(&3));
    drop(driver);
    assert!(!resource.snapshot().loading);
}

#[test]
fn asset_requests_limit_each_environment_and_release_cancelled_slots() {
    use t3_client::services::assets::*;
    let service = AssetService::default();
    let rpc = PendingRpc::default();
    let replies: Vec<_> = (0..18).map(|_| rpc.enqueue()).collect();
    let attachment = || {
        decode::<t3_contracts::assets::AssetResource>(
            json!({"_tag":"attachment","attachmentId":"image"}),
        )
    };
    let mut requests: Vec<_> = (0..18)
        .map(|_| Box::pin(service.mint(env(), &rpc, attachment())))
        .collect();
    for request in &mut requests {
        assert!(request.as_mut().now_or_never().is_none());
    }
    assert_eq!(rpc.calls.get(), ASSET_URL_MAX_IN_FLIGHT);
    drop(requests);
    assert!(replies.iter().take(6).all(oneshot::Sender::is_canceled));
    let mut replacement = Box::pin(service.mint(env(), &rpc, attachment()));
    assert!(replacement.as_mut().now_or_never().is_none());
    assert_eq!(rpc.calls.get(), ASSET_URL_MAX_IN_FLIGHT + 1);
    let other = PendingRpc::default();
    let _reply = other.enqueue();
    let mut independent =
        Box::pin(service.mint(EnvironmentId("other".into()), &other, attachment()));
    assert!(independent.as_mut().now_or_never().is_none());
    assert_eq!(other.calls.get(), 1);
}

#[test]
fn cancelled_attachment_mints_are_deleted_and_transfer_failures_preserve_the_id() {
    use t3_client::services::assets::*;
    block_on(async {
        let rpc = ScriptedRpc::default();
        let platform = FakePlatform::default();
        let upload = decode::<t3_contracts::assets::AttachmentCreateUploadUrlInput>(
            json!({"type":"file","name":"file.txt","mimeType":"text/plain","sizeBytes":1}),
        );
        rpc.scripts.borrow_mut().extend([
            RpcScript {
                method: "attachments.createUploadUrl".into(),
                input: json!(upload),
                replies: vec![Ok(
                    json!({"attachmentId":"fresh","relativeUrl":"/upload","expiresAt":1}),
                )],
            },
            RpcScript {
                method: "attachments.delete".into(),
                input: json!({"attachmentId":"fresh"}),
                replies: vec![Ok(Value::Null)],
            },
        ]);
        assert!(
            matches!(attachment_upload_cycle(&rpc,&platform,"https://remote.test",upload,vec![1],"text/plain",|_|false).await,AttachmentUploadResult::Cancelled{attachment_id} if attachment_id=="fresh")
        );
        assert!(rpc.scripts.borrow().is_empty());
        assert!(platform.http_requests().is_empty());
    });
}

#[test]
fn closing_a_resource_prevents_a_driver_from_resurrecting_it_after_removal() {
    let platform = FakePlatform::default();
    let resource = Resource::new(ResourcePolicy::default(), Generation(1));
    let rpc = PendingRpc::default();
    resource.close();
    assert!(matches!(
        block_on(resource.execute::<Number, _, _>(&rpc, &platform, 0)),
        Err(ClientError::Cancelled)
    ));
    assert_eq!(rpc.calls.get(), 0);
    assert!(!resource.snapshot().loading);
}

#[test]
fn metadata_driver_batches_mounted_paths_renews_misses_and_releases_its_timer_after_unmount() {
    let service = FileMetadataService::default();
    let platform = FakePlatform::default();
    let rpc = Rc::new(PendingRpc::default());
    let reply = rpc.enqueue();
    let (tx, rx) = mpsc::unbounded();
    tx.unbounded_send(connection(rpc.clone(), 1).session)
        .unwrap();
    let mut driver = Box::pin(service.drive(env(), &platform, rx.boxed_local()));
    assert!(driver.as_mut().now_or_never().is_none());
    let reader = service.observe(env(), "/missing", None, 0);
    assert!(driver.as_mut().now_or_never().is_none());
    assert_eq!(rpc.calls.get(), 1);
    reply.send(json!({"entries":[null]})).unwrap();
    assert!(driver.as_mut().now_or_never().is_none());
    assert_eq!(reader.resource.snapshot().value.as_deref(), Some(&None));
    assert_eq!(platform.pending_timers(), 1);
    let reply = rpc.enqueue();
    platform.advance(60_000);
    assert!(driver.as_mut().now_or_never().is_none());
    assert_eq!(rpc.calls.get(), 2);
    reply.send(json!({"entries":[{"kind":"file"}]})).unwrap();
    assert!(driver.as_mut().now_or_never().is_none());
    assert!(reader.resource.snapshot().value.as_ref().unwrap().is_some());
    assert_eq!(platform.pending_timers(), 0);
    drop(reader);
    assert!(driver.as_mut().now_or_never().is_none());
    drop(driver);
    assert_eq!(platform.pending_timers(), 0);
}
