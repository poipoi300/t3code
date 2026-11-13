use crate::checkpoint_service_test::support::*;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use t3_contracts::provider_instance::ProviderInstanceId;
use t3_orchestration::provider::ProviderAdapter;
use t3_orchestration::{
    api::*,
    decision::engine::PureDecisionEngine,
    error::{Error, Result},
    ports::checkpoints_forks::checkpoint_restore_safety::expanded_id,
    runtime::dispatch::{DispatchServices, Dispatcher, ProjectionCommandInputs},
};
struct NoProviders;
#[async_trait]
impl ProviderRegistry for NoProviders {
    async fn adapter(&self, _: ProviderInstanceId) -> Result<Arc<dyn ProviderAdapter>> {
        Err(Error::Unsupported("test does not start providers"))
    }
}
struct Recovery;
#[async_trait]
impl RecoveryService for Recovery {
    async fn recover(&self) -> Result<RecoverySummary> {
        Ok(RecoverySummary::default())
    }
    async fn reconcile(&self, _: Option<ThreadId>) -> Result<RecoverySummary> {
        Ok(RecoverySummary::default())
    }
    async fn prepare_for_shutdown(&self) -> Result<()> {
        Ok(())
    }
}
fn dispatcher(h: &Harness) -> Dispatcher {
    Dispatcher::new(
        DispatchServices {
            database: h.db.stores.clone(),
            inputs: Arc::new(ProjectionCommandInputs {
                reader: h.reader.clone(),
                providers: Arc::new(NoProviders),
                clock: h.clock.clone(),
                ids: h.ids.clone(),
            }),
            engine: Arc::new(PureDecisionEngine),
            committer: h.sink.clone(),
            recovery: Arc::new(Recovery),
            clock: h.clock.clone(),
        },
        None,
    )
}
async fn fork(h: &Harness, target: &str, point: serde_json::Value, side_chat: bool) -> Result<()> {
    fork_from(h, "thread:test", target, point, side_chat).await
}
async fn fork_from(
    h: &Harness,
    source: &str,
    target: &str,
    point: serde_json::Value,
    side_chat: bool,
) -> Result<()> {
    let dispatcher = dispatcher(h);
    dispatcher.dispatch(serde_json::from_value(json!({"type":"thread.fork","commandId":format!("fork:{target}"),"sourceThreadId":source,"targetThreadId":target,"sourcePoint":point,"sideChat":side_chat,"createdBy":"user","creationSource":"mcp"}))?).await?;
    Ok(())
}
#[tokio::test]
async fn persisted_fork_keeps_parent_watch_and_inherits_only_pull_request_context() -> Result<()> {
    let h = Harness::new().await?;
    let mut parent = thread("thread:test", None);
    parent.pull_requests = Some(Some(vec![serde_json::from_value(
        json!({"host":"github.com","repository":"example/repo","number":1,"url":"https://github.com/example/repo/pull/1","source":"agent","linkedAt":time(),"snapshot":null,"stack":null,"watch":{"startedAt":time(),"headSha":"head","failedChecks":[],"passed":false,"passedChecks":[],"remarksThrough":time(),"remarkIds":[],"conflicting":false,"wakes":0}}),
    )?]));
    h.seed("thread.created", &parent).await?;
    h.seed("run.created", run(1, "completed")).await?;
    let before = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .expect("parent");
    fork(
        &h,
        "thread:fork",
        json!({"type":"run","runId":"run:1"}),
        false,
    )
    .await?;
    let child = h
        .reader
        .projection(ThreadId("thread:fork".into()))
        .await?
        .expect("fork");
    assert!(child.runs.is_empty());
    assert!(child.provider_threads.is_empty());
    assert_eq!(child.context_transfers.len(), 1);
    assert_eq!(
        child.context_transfers[0].status,
        ContextTransferStatus::Pending
    );
    let mut expected = parent.pull_requests.clone().flatten().expect("links");
    expected[0].watch = None;
    assert_eq!(
        child.thread.pull_requests.flatten().expect("fork links"),
        expected
    );
    let after = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .expect("parent");
    assert_eq!(after.thread, before.thread);
    assert_eq!(after.runs, before.runs);
    Ok(())
}
#[tokio::test]
async fn persisted_stopped_forks_keep_exact_context_boundary_without_reusing_usage() -> Result<()> {
    for status in ["failed", "interrupted", "cancelled"] {
        let h = Harness::new().await?;
        let source = thread("thread:test", None);
        let source_run = run(1, status);
        let source_provider = provider();
        h.seed("run.created", &source_run).await?;
        h.seed("provider-thread.updated", &source_provider).await?;
        h.seed("thread.created", &source).await?;
        fork(
            &h,
            "thread:fork",
            json!({"type":"run","runId":"run:1"}),
            false,
        )
        .await?;
        let child = h
            .reader
            .projection(ThreadId("thread:fork".into()))
            .await?
            .expect("fork");
        let transfer = &child.context_transfers[0];
        let point = serde_json::to_value(&transfer.source_point)?;
        assert_eq!(point["threadId"], "thread:test");
        assert_eq!(point["runId"], "run:1");
        assert_eq!(
            point["providerThreadRef"],
            serde_json::to_value(&source_provider.native_thread_ref)?
        );
        assert!(transfer.source_token_usage.is_none());
        assert!(transfer.resolution.is_none());
        assert!(transfer.consumed_at.is_none());
        assert_eq!(
            expanded_id(&transfer.source_thread_id)?,
            ThreadId("thread:test".into())
        );
        assert_eq!(
            expanded_id(&transfer.target_thread_id)?,
            ThreadId("thread:fork".into())
        );
        assert_eq!(
            serde_json::to_value(
                h.reader
                    .run(ThreadId("thread:test".into()), RunId("run:1".into()))
                    .await?
                    .expect("source")
                    .status
            )?,
            json!(status)
        );
    }
    Ok(())
}
#[tokio::test]
async fn explicit_checkpoint_fork_resolves_its_persisted_source_run() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let h = Harness::new().await?;
    let backend = Arc::new(Backend::default());
    let scope = h.seed_run(temp.path(), 1, "completed").await?;
    let checkpoint = backend
        .service()
        .capture(
            capture_input(scope, 1),
            tokio_util::sync::CancellationToken::new(),
        )
        .await?;
    h.seed("checkpoint.captured", &checkpoint).await?;
    let mut source = run(1, "completed");
    source.checkpoint_id = Some(checkpoint.id.clone());
    h.seed("run.updated", source).await?;
    fork(
        &h,
        "thread:checkpoint",
        json!({"type":"checkpoint","checkpointId":checkpoint.id}),
        false,
    )
    .await?;
    let child = h
        .reader
        .projection(ThreadId("thread:checkpoint".into()))
        .await?
        .expect("fork");
    let point = serde_json::to_value(&child.context_transfers[0].source_point)?;
    assert_eq!(point["runId"], "run:1");
    assert_eq!(point["checkpointId"], serde_json::to_value(checkpoint.id)?);
    Ok(())
}
#[tokio::test]
async fn side_chat_waits_for_failed_worktree_then_opens_after_a_later_root_run() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let h = Harness::new().await?;
    h.seed_run(temp.path(), 1, "failed").await?;
    let mut failed = run(1, "failed");
    failed.workspace_preparation = Some(Some(serde_json::from_value(
        json!({"type":"worktree","baseRef":"main"}),
    )?));
    h.seed("run.updated", failed).await?;
    let error = fork(
        &h,
        "thread:blocked-side",
        json!({"type":"latest_stable"}),
        true,
    )
    .await
    .expect_err("no prepared workspace");
    assert!(error.to_string().contains("has no worktree yet"));
    assert!(
        h.reader
            .thread(ThreadId("thread:blocked-side".into()))
            .await?
            .is_none()
    );
    let scope = h.seed_run(temp.path(), 2, "completed").await?;
    let backend = Arc::new(Backend::default());
    let checkpoint = backend
        .service()
        .capture(
            capture_input(scope, 2),
            tokio_util::sync::CancellationToken::new(),
        )
        .await?;
    h.seed("checkpoint.captured", &checkpoint).await?;
    let mut stable = run(2, "completed");
    stable.checkpoint_id = Some(checkpoint.id);
    h.seed("run.updated", stable).await?;
    fork(&h, "thread:side", json!({"type":"latest_stable"}), true).await?;
    let child = h
        .reader
        .projection(ThreadId("thread:side".into()))
        .await?
        .expect("side chat");
    assert_eq!(child.thread.lineage.side_chat, Some(Some(true)));
    assert_eq!(
        serde_json::to_value(&child.context_transfers[0].source_point)?["runId"],
        "run:2"
    );
    Ok(())
}
async fn seed_history_run(
    h: &Harness,
    thread: &str,
    id: &str,
    ordinal: u64,
    status: &str,
    text: &str,
) -> Result<()> {
    let mut source = serde_json::to_value(run(ordinal, status))?;
    source["id"] = json!(id);
    source["threadId"] = json!(thread);
    source["userMessageId"] = json!(format!("message:{id}"));
    h.seed("run.created", source).await?;
    h.seed("turn-item.updated",json!({"id":format!("item:{id}"),"threadId":thread,"runId":id,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":ordinal,"status":"completed","title":null,"startedAt":time(),"completedAt":time(),"updatedAt":time(),"type":"user_message","createdBy":"user","creationSource":"web","inputIntent":"turn_start","messageId":format!("message:{id}"),"text":text,"attachments":[]})).await
}
#[tokio::test]
async fn portable_fork_history_bounds_each_ancestor_and_excludes_rolled_back_runs() -> Result<()> {
    use t3_orchestration::ports::runs_ingestion::context_handoff_service::read_fork_history_items;
    let h = Harness::new().await?;
    seed_history_run(
        &h,
        "thread:test",
        "ancestor:1",
        1,
        "rolled_back",
        "ROLLED_BACK_MARKER",
    )
    .await?;
    seed_history_run(
        &h,
        "thread:test",
        "ancestor:2",
        2,
        "completed",
        "ANCESTOR_BOUNDARY",
    )
    .await?;
    seed_history_run(
        &h,
        "thread:test",
        "ancestor:3",
        3,
        "completed",
        "PAST_ANCESTOR_BOUNDARY",
    )
    .await?;
    fork(
        &h,
        "thread:child",
        json!({"type":"run","runId":"ancestor:2"}),
        false,
    )
    .await?;
    seed_history_run(
        &h,
        "thread:child",
        "child:1",
        1,
        "interrupted",
        "CHILD_BOUNDARY",
    )
    .await?;
    seed_history_run(
        &h,
        "thread:child",
        "child:2",
        2,
        "completed",
        "PAST_CHILD_BOUNDARY",
    )
    .await?;
    fork_from(
        &h,
        "thread:child",
        "thread:grandchild",
        json!({"type":"run","runId":"child:1"}),
        false,
    )
    .await?;
    let grandchild = h
        .reader
        .thread(ThreadId("thread:grandchild".into()))
        .await?
        .expect("grandchild");
    let history = read_fork_history_items(h.reader.as_ref(), &grandchild, 0).await?;
    let texts = history
        .iter()
        .map(|item| {
            serde_json::to_value(item).map(|value| value["text"].as_str().expect("text").to_owned())
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    assert_eq!(texts, vec!["ANCESTOR_BOUNDARY", "CHILD_BOUNDARY"]);
    Ok(())
}
#[test]
fn side_chat_digest_keeps_recent_activity_with_a_utf16_budget_and_parent_recovery() {
    use t3_orchestration::ports::runs_ingestion::context_handoff_service::make_side_chat_context;
    let lines = (0..100)
        .map(|index| format!("- item-{index}: {}", "\u{1f600}".repeat(200)))
        .collect::<Vec<_>>();
    let context = make_side_chat_context(&ThreadId("thread:parent".into()), &lines);
    assert!(context.encode_utf16().count() <= 8000);
    assert!(context.contains("side chat"));
    assert!(context.contains("do not continue or redo"));
    assert!(context.contains("Earlier items truncated."));
    assert!(!context.contains("item-0:"));
    assert!(context.contains("item-99:"));
    assert!(context.contains("t3_thread_read on thread thread:parent"));
}

#[tokio::test]
async fn merge_back_pending_context_is_scoped_to_its_target_and_consumption_releases_it()
-> Result<()> {
    let h = Harness::new().await?;
    seed_history_run(&h, "thread:test", "parent:1", 1, "completed", "parent").await?;
    for child in ["first", "second"] {
        fork(
            &h,
            &format!("thread:{child}"),
            json!({"type":"run","runId":"parent:1"}),
            true,
        )
        .await?;
        seed_history_run(
            &h,
            &format!("thread:{child}"),
            &format!("{child}:1"),
            1,
            "completed",
            child,
        )
        .await?;
    }
    let merge = |id: &str, source: &str, target: &str, run: &str| {
        serde_json::from_value(
            json!({"type":"thread.merge_back","commandId":id,"sourceThreadId":source,"targetThreadId":target,"sourcePoint":{"type":"run","runId":run},"createdBy":"user","creationSource":"web"}),
        )
    };
    let commands = dispatcher(&h);
    commands
        .dispatch(merge(
            "merge:first",
            "thread:first",
            "thread:test",
            "first:1",
        )?)
        .await?;
    let rejected = commands
        .dispatch(merge(
            "merge:second-rejected",
            "thread:second",
            "thread:test",
            "second:1",
        )?)
        .await
        .expect_err("pending first fork");
    assert!(
        rejected
            .to_string()
            .contains("pending context from another side chat or fork")
    );
    commands
        .dispatch(merge(
            "merge:first-again",
            "thread:first",
            "thread:test",
            "first:1",
        )?)
        .await?;
    fork_from(
        &h,
        "thread:first",
        "thread:nested",
        json!({"type":"run","runId":"first:1"}),
        false,
    )
    .await?;
    seed_history_run(&h, "thread:nested", "nested:1", 1, "completed", "nested").await?;
    commands
        .dispatch(merge(
            "merge:nested",
            "thread:nested",
            "thread:first",
            "nested:1",
        )?)
        .await?;
    let main = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .expect("main");
    let merges = main
        .context_transfers
        .iter()
        .filter(|transfer| transfer.r#type == OrchestrationV2ContextTransferType::MergeBack)
        .collect::<Vec<_>>();
    assert_eq!(merges.len(), 2);
    assert_eq!(merges[0].status, ContextTransferStatus::Superseded);
    assert_eq!(merges[1].status, ContextTransferStatus::Pending);
    let mut consumed = (*merges[1]).clone();
    consumed.status = ContextTransferStatus::Consumed;
    consumed.consumed_at = Some(time());
    h.seed("context-transfer.updated", consumed).await?;
    commands
        .dispatch(merge(
            "merge:second-after-consumption",
            "thread:second",
            "thread:test",
            "second:1",
        )?)
        .await?;
    let main = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .expect("main");
    let pending = main
        .context_transfers
        .iter()
        .filter(|transfer| {
            transfer.r#type == OrchestrationV2ContextTransferType::MergeBack
                && transfer.status == ContextTransferStatus::Pending
        })
        .collect::<Vec<_>>();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        expanded_id(&pending[0].source_thread_id)?,
        ThreadId("thread:second".into())
    );
    Ok(())
}

#[tokio::test]
async fn closing_side_chats_deletes_only_unused_chats_and_deleting_parent_closes_the_rest()
-> Result<()> {
    let h = Harness::new().await?;
    seed_history_run(&h, "thread:test", "parent:1", 1, "completed", "parent").await?;
    for name in ["unused", "used"] {
        fork(
            &h,
            &format!("thread:{name}"),
            json!({"type":"run","runId":"parent:1"}),
            true,
        )
        .await?;
    }
    seed_history_run(&h, "thread:used", "used:1", 1, "completed", "a question").await?;
    let commands = dispatcher(&h);
    for name in ["unused", "used"] {
        commands.dispatch(serde_json::from_value(json!({"type":"thread.delete","commandId":format!("close:{name}"),"threadId":format!("thread:{name}"),"onlyIfUnused":true}))?).await?;
    }
    assert!(
        h.reader
            .thread(ThreadId("thread:unused".into()))
            .await?
            .expect("unused")
            .deleted_at
            .is_some()
    );
    assert!(
        h.reader
            .thread(ThreadId("thread:used".into()))
            .await?
            .expect("used")
            .deleted_at
            .is_none()
    );
    commands
        .dispatch(serde_json::from_value(
            json!({"type":"thread.delete","commandId":"delete:parent","threadId":"thread:test"}),
        )?)
        .await?;
    assert!(
        h.reader
            .thread(ThreadId("thread:used".into()))
            .await?
            .expect("used")
            .deleted_at
            .is_some()
    );
    let rejected = fork(&h, "thread:orphan", json!({"type":"latest_stable"}), true)
        .await
        .expect_err("deleted parent");
    assert!(rejected.to_string().contains("was deleted"));
    assert!(
        h.reader
            .thread(ThreadId("thread:orphan".into()))
            .await?
            .is_none()
    );
    Ok(())
}
