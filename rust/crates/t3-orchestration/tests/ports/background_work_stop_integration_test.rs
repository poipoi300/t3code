use super::{
    provider_event_ingestor_test::item, remote_delegated_task_test::live_dispatcher_with_inputs,
    run_execution_service_test::root_state,
};
use serde_json::json;
use t3_orchestration::{api::*, effects::EffectRequest, error::Result, stores::outbox};
#[tokio::test]
async fn stop_targets_background_work_on_an_earlier_provider_thread_while_a_newer_run_is_starting()
-> Result<()> {
    for kind in ["thread.stop", "run.interrupt"] {
        let (h, dispatcher, _) =
            live_dispatcher_with_inputs(|base| std::sync::Arc::new(StopProbeInputs(base))).await?;
        let mut earlier = root_state();
        earlier.run.status = OrchestrationV2RunStatus::Completed;
        earlier.provider_thread.status = ProviderThreadStatus::Idle;
        earlier.provider_thread.pending_background_tasks = Some(Some(serde_json::from_value(
            json!([{"kind":"command","taskId":"shell:old","awaited":true}]),
        )?));
        h.write("run.updated", t3_contracts::stored::to_value(&earlier.run)?)
            .await?;
        h.write(
            "provider-thread.updated",
            t3_contracts::stored::to_value(&earlier.provider_thread)?,
        )
        .await?;
        let mut work = item("command_execution", "background:old");
        work["input"] = "sleep 150".into();
        work["output"] = "".into();
        h.write("turn-item.updated", work).await?;
        let mut newer = t3_contracts::stored::to_value(&earlier.run)?;
        newer["id"] = "run:new".into();
        newer["ordinal"] = 2.into();
        newer["status"] = "starting".into();
        newer["providerThreadId"] = "pt:new".into();
        newer["rootNodeId"] = "node:new".into();
        newer["activeAttemptId"] = "attempt:new".into();
        newer["completedAt"] = serde_json::Value::Null;
        h.write("run.created", newer).await?;
        let mut provider = t3_contracts::stored::to_value(&earlier.provider_thread)?;
        provider["id"] = "pt:new".into();
        provider["providerSessionId"] = "session:new".into();
        provider["lastRunOrdinal"] = 2.into();
        provider["pendingBackgroundTasks"] = json!([]);
        h.write("provider-thread.updated", provider).await?;
        let mut node = t3_contracts::stored::to_value(&earlier.root_node)?;
        node["id"] = "node:new".into();
        node["runId"] = "run:new".into();
        node["providerThreadId"] = "pt:new".into();
        node["status"] = "pending".into();
        h.write("node.updated", node).await?;
        let mut attempt = t3_contracts::stored::to_value(&earlier.attempt)?;
        attempt["id"] = "attempt:new".into();
        attempt["runId"] = "run:new".into();
        attempt["rootNodeId"] = "node:new".into();
        attempt["providerThreadId"] = "pt:new".into();
        attempt["providerTurnId"] = serde_json::Value::Null;
        attempt["status"] = "pending".into();
        h.write("run-attempt.created", attempt).await?;
        let mut command = json!({"type":kind,"commandId":"stop","threadId":"thread:test"});
        if kind == "run.interrupt" {
            command["runId"] = "run:new".into();
        }
        dispatcher
            .dispatch(serde_json::from_value(command)?)
            .await?;
        assert_eq!(
            h.reader
                .run(ThreadId("thread:test".into()), RunId("run:new".into()))
                .await?
                .unwrap()
                .status,
            OrchestrationV2RunStatus::Interrupted
        );
        let effects = h
            .database
            .stores
            .read(|db| outbox::list_by_command(db, &CommandId("stop".into())))
            .await?;
        assert!(effects.iter().any(|effect|matches!(&effect.request,EffectRequest::Interrupt{provider_thread_id,..} if provider_thread_id.0=="pt:test")),"{kind}: {effects:?}");
    }
    Ok(())
}

// seam-shim: supplies the native pending-background-work probe at CommandInputs preparation.
struct StopProbeInputs(t3_orchestration::runtime::dispatch::ProjectionCommandInputs);
#[async_trait::async_trait]
impl t3_orchestration::runtime::dispatch::CommandInputs for StopProbeInputs {
    fn reader(&self) -> Option<&dyn ProjectionReader> {
        Some(self.0.reader.as_ref())
    }
    async fn lock_keys(&self, command: &OrchestrationV2ServerCommand) -> Result<Vec<ThreadId>> {
        self.0.lock_keys(command).await
    }
    async fn prepare(
        &self,
        command: &OrchestrationV2ServerCommand,
    ) -> Result<t3_orchestration::runtime::dispatch::PreparedCommand> {
        let mut prepared = self.0.prepare(command).await?;
        prepared.context.inputs.background_work = vec![
            (ProviderThreadId("pt:test".into()), true),
            (ProviderThreadId("pt:new".into()), false),
        ];
        prepared.context.inputs.live_sessions = vec![
            (ProviderSessionId("session:test".into()), true),
            (ProviderSessionId("session:new".into()), false),
        ];
        Ok(prepared)
    }
}
