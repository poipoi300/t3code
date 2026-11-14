#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use t3_contracts::base_schemas::ThreadId;
use t3_mcp::{adapters::ServiceAdapters, *};

struct Store;
#[async_trait]
impl AccessStore for Store {
    async fn thread_shell(&self, _: &ThreadId) -> Result<Option<ThreadShell>> {
        Ok(None)
    }
}

#[cfg(feature = "scheduler")]
mod scheduler {
    use super::*;
    use t3_mcp::{
        adapters::scheduler::SchedulerAdapter,
        services::{OrchestratorService, ThreadService},
    };
    struct Execution;
    #[async_trait]
    impl t3_scheduler::Orchestration for Execution {
        async fn dispatch(
            &self,
            _: t3_scheduler::DispatchRequest,
        ) -> t3_scheduler::Result<t3_scheduler::DispatchReceipt> {
            Ok(t3_scheduler::DispatchReceipt {
                thread_id: ThreadId("scheduled-thread".into()),
            })
        }
    }
    #[tokio::test]
    async fn native_schedule_updates_runs_and_deletes_preserve_mode_limits() {
        let database = tokio::task::spawn_blocking(|| t3_persistence::Database::open(":memory:"))
            .await
            .unwrap()
            .unwrap();
        let clock = Arc::new(t3_scheduler::TokioClock::new(
            "2026-01-01T00:00:00Z".parse().unwrap(),
        ));
        let service = Arc::new(
            t3_scheduler::ScheduledTaskService::new(
                database,
                clock,
                Arc::new(Execution),
                Default::default(),
            )
            .await
            .unwrap(),
        );
        let task=service.upsert(serde_json::from_value(json!({"id":"task","title":"Before","prompt":"Review work","enabled":true,"schedule":{"type":"interval","everyMs":3600000},"projectId":"project","workspaceStrategy":{"type":"root"},"modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap()).await.unwrap();
        let adapter = SchedulerAdapter {
            service: service.clone(),
            #[cfg(feature = "orchestration")]
            reader: None,
        };
        let mut limited = context("limited").await;
        limited.limits.runtime_mode = t3_contracts::provider_policy::RuntimeMode::ApprovalRequired;
        let error = adapter
            .delete_scheduled_task(
                limited,
                serde_json::from_value(json!({"scheduledTaskId":"task"})).unwrap(),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(error,McpError::Failure {ref code,..} if code=="runtime_mode_escalation_denied")
        );
        assert_eq!(service.get(&task.id).await.unwrap().title.0, "Before");
        let saved = adapter
            .schedule_task(
                context("owner").await,
                serde_json::from_value(json!({"scheduledTaskId":"task","title":"After"})).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(saved.title, "After");
        let listed = adapter
            .list_scheduled_tasks(
                context("owner").await,
                serde_json::from_value(json!({"projectId":"project"})).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed.tasks.len(), 1);
        assert_eq!(listed.tasks[0].title, "After");
        let run = adapter
            .run_scheduled_task_now(
                context("owner").await,
                decode_input("run_scheduled_task_now", json!({"taskId":"task"})).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(run["threadId"], "scheduled-thread");
        assert_eq!(service.get(&task.id).await.unwrap().run_count, 1);
        assert!(
            adapter
                .delete_scheduled_task(
                    context("owner").await,
                    serde_json::from_value(json!({"scheduledTaskId":"task"})).unwrap()
                )
                .await
                .unwrap()
                .deleted
        );
        assert!(service.get(&task.id).await.is_err());
        service.shutdown().await;
    }
}
async fn context(owner: &str) -> CallContext {
    let scope=serde_json::from_value(json!({"environmentId":"environment","requestNamespace":owner,"issuedAt":0,"capabilities":["orchestration"],"client":{"sessionId":owner,"label":owner,"access":"full-access"}})).unwrap();
    access::authorize(
        Arc::new(Store),
        scope,
        AccessRule::Reads,
        "probe",
        &json!({}),
        false,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn all_unbound_tools_report_the_named_missing_service_as_structured_content() {
    let mut services = Services::default();
    Arc::new(ServiceAdapters::default()).install(&mut services);
    let context = context("owner").await;
    let witnesses: Value = serde_json::from_str(include_str!("../fixtures/decoders.json")).unwrap();
    let bindings: Value = serde_json::from_str(include_str!("../fixtures/bindings.json")).unwrap();
    let mut checked = 0;
    for (name, cases) in witnesses.as_object().unwrap() {
        if name == "send_to_user" {
            continue;
        }
        let input = if name == "preview_act" {
            json!({"action":"press","key":"Enter"})
        } else {
            cases[0]["input"].clone()
        };
        let input = decode_input(name, input).unwrap_or_else(|error| panic!("{name}: {error}"));
        let error = services
            .dispatch(name, context.clone(), input)
            .await
            .unwrap_err();
        assert!(
            matches!(&error,McpError::NotAvailable{tool,method} if tool==name && method==bindings[name]["expectedServiceMethod"].as_str().unwrap()),
            "{name}: {error}"
        );
        let result = output::failure(name, error);
        assert_eq!(result["isError"], true);
        assert_eq!(result["structuredContent"]["available"], false);
        assert_eq!(result["structuredContent"]["tool"], *name);
        assert!(
            result["structuredContent"]["error"]["message"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        );
        checked += 1;
    }
    assert_eq!(checked, catalog::catalog().tools.len() - 1);
}

#[cfg(feature = "workspace")]
mod attachments {
    use super::*;
    use std::{future::Future, path::Path, pin::Pin};
    use t3_mcp::services::AttachmentService;
    use t3_mcp::{
        adapters::attachments::AttachmentAdapter,
        session::{AliveMark, Clock},
    };
    use t3_workspace::{
        assets::AssetSigner,
        attachments::{AttachmentStore, DocumentText},
    };
    struct FixedClock;
    impl Clock for FixedClock {
        fn now(&self) -> AliveMark {
            AliveMark {
                wall_ms: 1_000,
                monotonic_ms: 0,
            }
        }
    }
    struct Documents;
    impl DocumentText for Documents {
        fn ensure<'a>(&'a self, _: &'a Path) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            Box::pin(async {})
        }
    }

    #[tokio::test]
    async fn native_uploads_are_signed_and_only_the_preparing_caller_can_discard() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(AttachmentStore::new(
            temp.path().into(),
            AssetSigner::new(vec![7; 32]).unwrap(),
            Arc::new(Documents),
        ));
        let adapter = AttachmentAdapter::new(store.clone(), Arc::new(FixedClock));
        let input=decode_input("t3_attachment_prepare_upload",json!({"upload":{"type":"file","name":"notes.txt","mimeType":"text/plain","sizeBytes":5}})).unwrap();
        let issued = adapter
            .t3_attachment_prepare_upload(context("alice").await, input)
            .await
            .unwrap();
        let token = issued
            .relative_url
            .0
            .strip_prefix("/api/attachments/upload/")
            .unwrap();
        let claims = store.validate_upload(token, 1_000).unwrap();
        let path = store
            .upload(token, 1_000, std::io::Cursor::new(b"hello"))
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"hello");
        let input = serde_json::from_value(json!({"attachmentId":issued.attachment_id})).unwrap();
        let refused = adapter
            .t3_attachment_discard(context("bob").await, input)
            .await
            .unwrap_err();
        assert!(matches!(refused,McpError::Failure {ref code,..} if code=="invalid_request"));
        assert!(path.exists());
        assert_eq!(claims.attachment_id, issued.attachment_id.0);
        let input =
            || serde_json::from_value(json!({"attachmentId":issued.attachment_id})).unwrap();
        assert_eq!(
            adapter
                .t3_attachment_discard(context("alice").await, input())
                .await
                .unwrap(),
            json!({})
        );
        assert!(!path.exists());
        assert!(
            adapter
                .t3_attachment_discard(context("alice").await, input())
                .await
                .is_err()
        );
    }
}
