#![cfg(feature = "orchestration")]
#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use t3_mcp::{
    adapters::orchestration::OrchestrationAdapter,
    services::{OrchestratorService, ProjectService, ThreadService},
    *,
};
use t3_orchestration::{
    api::*,
    decision::engine::PureDecisionEngine,
    error,
    ports::thread_project::thread_management_service::{
        NoLegacyTranscript, ThreadManagementServices,
    },
    projections::store::ProjectionStore,
    runtime::{
        commit::EventSink,
        dispatch::{DispatchServices, Dispatcher, ProjectionCommandInputs},
        worker::OutboxSignals,
    },
    testing::{FakeSettingsService, ManualClock, SequenceIds, TEST_TIME, TestDatabase},
};
struct NoProviders;
#[async_trait]
impl ProviderRegistry for NoProviders {
    async fn adapter(
        &self,
        _: t3_contracts::provider_instance::ProviderInstanceId,
    ) -> error::Result<Arc<dyn t3_orchestration::provider::ProviderAdapter>> {
        Err(error::Error::Unsupported(
            "no provider process in this test",
        ))
    }
}
struct Recovery;
#[async_trait]
impl RecoveryService for Recovery {
    async fn recover(&self) -> error::Result<RecoverySummary> {
        Ok(RecoverySummary::default())
    }
    async fn reconcile(&self, _: Option<ThreadId>) -> error::Result<RecoverySummary> {
        Ok(RecoverySummary::default())
    }
    async fn prepare_for_shutdown(&self) -> error::Result<()> {
        Ok(())
    }
}
fn adapter() -> Arc<OrchestrationAdapter> {
    let database = TestDatabase::memory().unwrap();
    let reader = Arc::new(ProjectionStore::new(database.stores.clone()));
    let clock = Arc::new(ManualClock::new(TEST_TIME));
    let sink = Arc::new(EventSink::new(
        database.stores.clone(),
        reader.clone(),
        clock.clone(),
        OutboxSignals::default(),
    ));
    let dispatcher = Dispatcher::new(
        DispatchServices {
            database: database.stores.clone(),
            inputs: Arc::new(ProjectionCommandInputs {
                reader: reader.clone(),
                providers: Arc::new(NoProviders),
                clock: clock.clone(),
                ids: Arc::new(SequenceIds::new("mcp")),
            }),
            engine: Arc::new(PureDecisionEngine),
            committer: sink.clone(),
            recovery: Arc::new(Recovery),
            clock: clock.clone(),
        },
        None,
    );
    let store = database.stores.clone();
    Arc::new(OrchestrationAdapter {
        reader: reader.clone(),
        database: store,
        commands: Arc::new(t3_mcp::adapters::orchestration::LimitedCommands {
            services: Arc::new(ThreadManagementServices {
                reader,
                dispatcher: Arc::new(dispatcher.clone()),
                committer: sink.clone(),
                database: database.stores,
                settings: Arc::new(FakeSettingsService::default()),
                hydrator: Arc::new(NoLegacyTranscript),
                worktree_restore: Arc::new(
                    t3_orchestration::testing::FakeThreadWorktreeRestore::default(),
                ),
                signals: sink.signals(),
                clock,
            }),
            dispatcher,
            stop: None,
            attachments: None,
        }),
    })
}
async fn dispatch(adapter: &OrchestrationAdapter, value: Value) {
    // Setup acts as a full-access user, so no mode limit applies.
    adapter
        .commands
        .dispatcher(t3_orchestration::composition::CommandCaller {
            thread_id: None,
            limit: t3_orchestration::ports::decisions::dispatch_mode_limit::DispatchModes {
                runtime_mode: t3_contracts::provider_policy::RuntimeMode::FullAccess,
                interaction_mode: t3_contracts::provider_policy::ProviderInteractionMode::Default,
            },
            link_fence: None,
        })
        .dispatch(serde_json::from_value(value).unwrap())
        .await
        .unwrap();
}
async fn create(adapter: &OrchestrationAdapter, id: &str) {
    dispatch(adapter,json!({"type":"thread.create","commandId":format!("create:{id}"),"threadId":id,"projectId":"project","title":"Before","modelSelection":{"instanceId":"codex","model":"a"},"runtimeMode":"approval-required","interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"})).await;
}
async fn context(adapter: Arc<OrchestrationAdapter>, name: &str, input: &Value) -> CallContext {
    let scope=serde_json::from_value(json!({"environmentId":"env","requestNamespace":"stable-session","issuedAt":0,"capabilities":["orchestration"],"client":{"sessionId":"client","label":"client","access":"approval-required"}})).unwrap();
    access::authorize(
        adapter,
        scope,
        access::tool_rule(name, input).unwrap(),
        name,
        input,
        false,
    )
    .await
    .unwrap()
}
#[tokio::test]
async fn metadata_ids_replay_and_dispatch_rechecks_the_target_mode_in_sqlite() {
    let adapter = adapter();
    create(&adapter, "caller").await;
    create(&adapter, "target").await;
    let input =
        json!({"threadId":"target","action":"rename","title":"Renamed","clientRequestId":"stable"});
    let admitted = context(adapter.clone(), "t3_thread_update", &input).await;
    let first = adapter
        .t3_thread_update(
            admitted.clone(),
            decode_input("t3_thread_update", input.clone()).unwrap(),
        )
        .await
        .unwrap();
    let repeated = adapter
        .t3_thread_update(admitted, decode_input("t3_thread_update", input).unwrap())
        .await
        .unwrap();
    assert_eq!(first.command_id, repeated.command_id);
    assert_eq!(first.sequence, repeated.sequence);
    assert_eq!(first.title, "Renamed");
    let stop = json!({"threadId":"target","stop":true,"clientRequestId":"stop-stable"});
    let admitted = context(adapter.clone(), "t3_thread_interrupt", &stop).await;
    let input = || serde_json::from_value(stop.clone()).unwrap();
    let stopped = adapter
        .t3_thread_interrupt(admitted.clone(), input())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&stopped).unwrap()["status"],
        "no_active_run"
    );
    assert_eq!(
        stopped,
        adapter
            .t3_thread_interrupt(admitted, input())
            .await
            .unwrap()
    );
    let input = json!({"threadId":"target","action":"pin"});
    let admitted = context(adapter.clone(), "t3_thread_organize", &input).await;
    dispatch(&adapter,json!({"type":"thread.runtime-mode.set","commandId":"elevate","threadId":"target","runtimeMode":"full-access"})).await;
    let error = adapter
        .t3_thread_organize(admitted, decode_input("t3_thread_organize", input).unwrap())
        .await
        .unwrap_err();
    assert!(
        matches!(error,McpError::Failure {ref code,..} if code=="runtime_mode_escalation_denied"),
        "{error}"
    );
    assert!(adapter.shell(ThreadId("target".into())).await.unwrap()["pinnedAt"].is_null());
}

#[tokio::test]
async fn management_rename_refuses_runtime_and_interaction_raises_after_admission() {
    for kind in ["runtime", "interaction"] {
        let adapter = adapter();
        create(&adapter, "caller").await;
        create(&adapter, "target").await;
        if kind == "interaction" {
            dispatch(&adapter,json!({"type":"thread.interaction-mode.set","commandId":"plan","threadId":"target","interactionMode":"plan"})).await;
        }
        let input = json!({"threadId":"target","action":"rename","title":"Forbidden"});
        let admitted = context(adapter.clone(), "t3_thread_update", &input).await;
        let admitted = if kind == "interaction" {
            let mut scope = admitted.scope;
            scope.client.as_mut().unwrap().narrowed_to = Some(Modes {
                interaction_mode: t3_contracts::provider_policy::ProviderInteractionMode::Plan,
                ..admitted.limits
            });
            access::authorize(
                adapter.clone(),
                scope,
                access::tool_rule("t3_thread_update", &input).unwrap(),
                "t3_thread_update",
                &input,
                false,
            )
            .await
            .unwrap()
        } else {
            admitted
        };
        let change = if kind == "runtime" {
            json!({"type":"thread.runtime-mode.set","commandId":"elevate","threadId":"target","runtimeMode":"full-access"})
        } else {
            json!({"type":"thread.interaction-mode.set","commandId":"elevate","threadId":"target","interactionMode":"default"})
        };
        dispatch(&adapter, change).await;
        assert!(
            adapter
                .t3_thread_update(admitted, decode_input("t3_thread_update", input).unwrap())
                .await
                .is_err(),
            "{kind}"
        );
        assert_eq!(
            adapter.shell(ThreadId("target".into())).await.unwrap()["title"],
            "Before",
            "{kind}"
        );
    }
}

#[tokio::test]
async fn native_project_rows_are_mapped_to_public_project_contracts() {
    let adapter = adapter();
    create(&adapter, "caller").await;
    adapter.database.transaction(|db| {
        let row=serde_json::from_value(json!({"projectId":"project","title":"Project","hasCustomTitle":true,"workspaceRoot":"C:/workspace","defaultModelSelection":null,"defaultThreadEnvMode":null,"autoPull":false,"faviconPath":null,"projectIcon":null,"scripts":[],"pinnedAt":null,"archivedAt":null,"createdAt":TEST_TIME,"updatedAt":TEST_TIME,"deletedAt":null}))?;
        t3_orchestration::stores::projects::upsert(db,&row)
    }).await.unwrap();
    let input = json!({"projectId":"project"});
    let admitted = context(adapter.clone(), "t3_project_read", &input).await;
    let project = adapter
        .t3_project_read(
            admitted.clone(),
            decode_input("t3_project_read", input).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(project["id"], "project");
    assert!(project.get("hasCustomTitle").is_none());
    serde_json::from_value::<t3_contracts::project::Project>(project.clone()).unwrap();
    let page = adapter
        .t3_project_list(
            admitted,
            decode_input("t3_project_list", json!({})).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page, json!({"projects":[project],"nextCursor":null}));
}

/// The composed runtime refuses under the command lock with a typed `CallerRefused` (review L1):
/// the tool answers TypeScript's code and `escalationDenied` text, not `orchestration_error`.
#[tokio::test]
async fn a_typed_refusal_under_the_command_lock_keeps_its_code() {
    use t3_mcp::adapters::orchestration::CallerCommandSource;
    use t3_orchestration::{
        api::CommandDispatcher, composition::CommandCaller, error::CallerRefusal,
        ports::decisions::dispatch_mode_limit::ModeLimit,
    };
    struct Refusing(Arc<dyn CallerCommandSource>);
    struct RefusingDispatcher;
    #[async_trait]
    impl CommandDispatcher for RefusingDispatcher {
        async fn dispatch(
            &self,
            command: t3_contracts::orchestration_v2::OrchestrationV2ServerCommand,
        ) -> error::Result<t3_orchestration::stores::receipts::CommandReceipt> {
            let value = serde_json::to_value(&command).unwrap();
            Err(error::Error::CallerRefused {
                command_id: serde_json::from_value(value["commandId"].clone()).unwrap(),
                refusal: CallerRefusal::AboveModeLimit {
                    thread_id: ThreadId("target".into()),
                    mode: ModeLimit::Runtime,
                    runtime_mode: "full-access".into(),
                    interaction_mode: "default".into(),
                },
            })
        }
    }
    impl CallerCommandSource for Refusing {
        fn threads(
            &self,
            caller: CommandCaller,
        ) -> t3_orchestration::ports::thread_project::thread_management_service::ThreadManagementService
        {
            self.0.threads(caller)
        }
        fn dispatcher(&self, _: CommandCaller) -> Arc<dyn CommandDispatcher> {
            Arc::new(RefusingDispatcher)
        }
        fn intake(
            &self,
            caller: CommandCaller,
        ) -> Option<t3_orchestration::ports::runs_ingestion::thread_message_intake::ThreadMessageIntake>
        {
            self.0.intake(caller)
        }
        fn stop(
            &self,
        ) -> Option<&t3_orchestration::ports::thread_project::thread_management_service::ThreadStopService>
        {
            None
        }
    }
    let base = adapter();
    create(&base, "caller").await;
    create(&base, "target").await;
    let adapter = Arc::new(OrchestrationAdapter {
        reader: base.reader.clone(),
        database: base.database.clone(),
        commands: Arc::new(Refusing(base.commands.clone())),
    });
    let input = json!({"threadId":"target","action":"pin"});
    let admitted = context(adapter.clone(), "t3_thread_organize", &input).await;
    let error = adapter
        .t3_thread_organize(admitted, decode_input("t3_thread_organize", input).unwrap())
        .await
        .unwrap_err();
    match error {
        McpError::Failure { code, message } => {
            assert_eq!(code, "runtime_mode_escalation_denied");
            assert_eq!(
                message,
                "Thread target now runs in full-access/default mode, above this caller's. Its user changed it while this call ran."
            );
        }
        other => panic!("{other}"),
    }
}
