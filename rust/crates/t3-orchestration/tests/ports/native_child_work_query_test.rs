use super::provider_event_ingestor_test::{Harness, fixture};
use async_trait::async_trait;
use serde_json::json;
use std::sync::{Arc, Mutex};
use t3_contracts::orchestration_v2::OrchestrationV2NativeChildWork;
use t3_orchestration::{api::*, error::Result, ports::runs_ingestion::native_child_work_query::*};
#[derive(Default)]
struct Backend(Mutex<Vec<ChildWorkInput>>);
#[async_trait]
impl NativeChildWorkBackend for Backend {
    async fn read(&self, input: ChildWorkInput) -> Result<Vec<OrchestrationV2NativeChildWork>> {
        self.0.lock().unwrap().push(input);
        Ok(vec![])
    }
}
#[tokio::test]
async fn only_sibling_tasks_from_the_same_native_session_share_child_records() -> Result<()> {
    for same_session in [false, true] {
        let h = Harness::new().await?;
        let mut child = fixture("OrchestrationV2AppThread");
        child["id"] = "child".into();
        child["creationSource"] = "provider".into();
        child["lineage"] = json!({"rootThreadId":"thread:test","parentThreadId":"thread:test","relationshipToParent":"subagent"});
        child["forkedFrom"] = json!({"type":"node","nodeId":"task:current"});
        h.write("thread.created", child).await?;
        for (index, task_id, run_id, provider_id, native_session) in [
            (
                0,
                "task:current",
                "run:current",
                "pt:current",
                "native:current",
            ),
            (
                1,
                "task:sibling",
                "run:sibling",
                "pt:sibling",
                if same_session {
                    "native:current"
                } else {
                    "native:other"
                },
            ),
        ] {
            let mut task = fixture("OrchestrationV2Subagent");
            task["id"] = task_id.into();
            task["threadId"] = "thread:test".into();
            task["childThreadId"] = if index == 0 {
                json!("child")
            } else {
                json!(null)
            };
            task["runId"] = run_id.into();
            task["driver"] = "antigravity".into();
            task["origin"] = "provider_native".into();
            task["providerInstanceId"] = "antigravity".into();
            task["nativeTaskRef"] =
                json!({"nativeId":task_id,"driver":"antigravity","strength":"strong"});
            h.write("subagent.updated", task).await?;
            let mut run = fixture("OrchestrationV2Run");
            run["id"] = run_id.into();
            run["threadId"] = "thread:test".into();
            run["ordinal"] = json!(index + 1);
            run["providerThreadId"] = provider_id.into();
            run["providerInstanceId"] = "antigravity".into();
            h.write("run.updated", run).await?;
            let mut provider = fixture("OrchestrationV2ProviderThread");
            provider["id"] = provider_id.into();
            provider["appThreadId"] = "thread:test".into();
            provider["driver"] = "antigravity".into();
            provider["providerInstanceId"] = "antigravity".into();
            provider["nativeThreadRef"] =
                json!({"driver":"antigravity","nativeId":native_session,"strength":"strong"});
            h.write("provider-thread.updated", provider).await?;
        }
        let backend = Arc::new(Backend::default());
        let query = NativeChildWorkQuery {
            projections: h.reader.clone(),
            backend: backend.clone(),
        };
        assert!(query.get(ThreadId("child".into())).await?.is_empty());
        let calls = backend.0.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].root_session_id, "native:current");
        assert_eq!(calls[0].siblings.len(), usize::from(same_session));
        if same_session {
            assert_eq!(calls[0].siblings[0].native_task_id, "task:sibling");
        }
    }
    Ok(())
}
