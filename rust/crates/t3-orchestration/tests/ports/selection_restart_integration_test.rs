use super::{
    provider_event_ingestor_test::Harness,
    provider_turn_start_service_test::{finish, policy, seed, service},
};
use async_trait::async_trait;
use std::sync::Arc;
use t3_contracts::provider_instance::ProviderInstanceId;
use t3_orchestration::{
    api::*,
    error::{Error, Result},
    provider::*,
    testing::FakeProviderAdapter,
};
use tokio_util::sync::CancellationToken;
pub(crate) struct PolicySessions(pub(crate) Arc<FakeProviderAdapter>);
#[async_trait]
impl SessionDirectory for PolicySessions {
    async fn get(&self, _id: ProviderSessionId) -> Result<Option<Arc<dyn SessionRuntime>>> {
        Ok(Some(self.0.runtime.clone()))
    }
    async fn attach(
        &self,
        _instance: ProviderInstanceId,
        _input: OpenSession,
    ) -> Result<Arc<dyn SessionRuntime>> {
        Err(Error::Unsupported("configured startup lost policy options"))
    }
    async fn attach_with_policy(
        &self,
        _instance: ProviderInstanceId,
        input: OpenSession,
        options: RuntimePolicyOptions,
    ) -> Result<Arc<dyn SessionRuntime>> {
        Ok(self
            .0
            .open_session_with_policy(input, options, CancellationToken::new())
            .await?)
    }
    async fn detach(&self, _id: ProviderSessionId, _revoke: bool) -> Result<()> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}
#[tokio::test]
async fn configured_reasoning_effort_survives_session_open_thread_ensure_or_resume_and_turn_start()
-> Result<()> {
    for resume in [false, true] {
        let h = Harness::new().await?;
        let mut state = seed(&h).await?;
        let (adapter, _calls) = h.fake();
        let adapter = Arc::new(adapter);
        state.provider_thread.native_thread_ref = Some(serde_json::from_value(
            serde_json::json!({"driver":"codex","nativeId":"native:policy","strength":"strong"}),
        )?);
        if resume {
            h.write(
                "provider-thread.updated",
                t3_contracts::stored::to_value(&state.provider_thread)?,
            )
            .await?;
            adapter
                .runtime
                .resume_thread_with_policy
                .push(Ok(state.provider_thread.clone()));
        } else {
            adapter
                .runtime
                .ensure_thread_with_policy
                .push(Ok(state.provider_thread.clone()));
        }
        adapter
            .open_session_with_policy
            .push(Ok(adapter.runtime.clone()));
        adapter.runtime.start_turn_with_policy.push(Ok(()));
        let start = service(&h, Arc::new(PolicySessions(adapter.clone())));
        let mut runtime_policy = policy();
        runtime_policy.cwd = Some(std::path::PathBuf::from("C:/isolated/policy-worktree"));
        let handle = start
            .start_configured(
                ThreadId("thread:test".into()),
                RunId("run:test".into()),
                ConfiguredRuntimePolicy {
                    policy: runtime_policy.clone(),
                    options: RuntimePolicyOptions {
                        reasoning_effort: Some("high".into()),
                    },
                },
                false,
                CancellationToken::new(),
            )
            .await?
            .unwrap();
        let (open, options, _) = adapter.open_session_with_policy.next_call().await.unwrap();
        assert_eq!(open.runtime_policy.cwd, runtime_policy.cwd);
        assert_eq!(options.reasoning_effort.as_deref(), Some("high"));
        let options = if resume {
            adapter
                .runtime
                .resume_thread_with_policy
                .next_call()
                .await
                .unwrap()
                .1
        } else {
            adapter
                .runtime
                .ensure_thread_with_policy
                .next_call()
                .await
                .unwrap()
                .1
        };
        assert_eq!(options.reasoning_effort.as_deref(), Some("high"));
        let (turn, options) = adapter
            .runtime
            .start_turn_with_policy
            .next_call()
            .await
            .unwrap();
        assert_eq!(options.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(turn.runtime_policy.cwd, runtime_policy.cwd);
        finish(adapter.as_ref(), &h, handle).await?;
        start.execution.shutdown().await?;
    }
    Ok(())
}
