#![allow(clippy::unwrap_used)]
mod support;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use support::*;
use t3_contracts::{
    base_schemas::{ProjectId, ThreadId},
    orchestration_v2::OrchestrationV2ServerCommand,
};
use t3_orchestration::{
    api::{
        Clock, CommandDispatcher, CommitGuard, CommitRequest, Decision, EventCommitter,
        ProjectionReader,
    },
    stores::{StoreDatabase, receipts::CommandReceipt},
};
use t3_project::{
    managed::ManagedProjectFolders,
    promotion::{MoveThreadTasks, ScratchThreadPromotion},
};
use tokio::sync::{Semaphore, watch};

struct UnavailableCommands;
#[async_trait]
impl CommandDispatcher for UnavailableCommands {
    async fn dispatch(
        &self,
        _: OrchestrationV2ServerCommand,
    ) -> t3_orchestration::error::Result<CommandReceipt> {
        Err(t3_orchestration::error::Error::Unsupported(
            "fixture commands unavailable",
        ))
    }
}
struct GatedTaskStore {
    entered: watch::Sender<bool>,
    release: Semaphore,
    database: StoreDatabase,
}
#[async_trait]
impl MoveThreadTasks for GatedTaskStore {
    async fn move_thread_tasks(&self, _: Vec<ThreadId>, _: ProjectId) -> t3_project::Result<()> {
        self.entered.send_replace(true);
        self.release.acquire().await.unwrap().forget();
        self.database
            .read(|db| {
                db.query_row(
                    "SELECT unavailable_scheduler_column FROM projection_projects",
                    [],
                    |_| Ok(()),
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }
}
#[tokio::test]
async fn disconnected_promotion_is_joined_before_shutdown_even_if_task_store_fails() {
    let f = Fixture::new();
    let folders = ManagedProjectFolders::new(
        f.root("base"),
        f.service.clone(),
        Arc::new(FixtureIo),
        git_environment(f.temp.path()),
    );
    let scratch = folders.ensure_scratch().await.unwrap();
    let thread_id = ThreadId("promotion-thread".into());
    let folder = folders
        .folder_for_thread(&scratch.project_id, &thread_id, "Promoted")
        .await
        .unwrap()
        .unwrap();
    let at = f.clock.now();
    let event = serde_json::from_value(json!({
        "id":"promotion:thread-created","type":"thread.created","threadId":thread_id,"providerInstanceId":"codex","occurredAt":at,
        "payload":{"createdBy":"user","creationSource":"web","id":thread_id,"projectId":scratch.project_id,"title":"Promoted",
        "providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"fixture"},
        "runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":folder,
        "activeProviderThreadId":null,"lineage":{"parentThreadId":null,"relationshipToParent":null,"rootThreadId":thread_id},
        "forkedFrom":null,"createdAt":at,"updatedAt":at,"archivedAt":null,
        "settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null}
    })).unwrap();
    f.sink
        .commit(CommitRequest {
            receipt: None,
            decision: Decision {
                events: vec![event],
                ..Default::default()
            },
            guard: CommitGuard::None,
        })
        .await
        .unwrap();
    let (entered, mut started) = watch::channel(false);
    let tasks = Arc::new(GatedTaskStore {
        entered,
        release: Semaphore::new(0),
        database: f.database.clone(),
    });
    let promotion = ScratchThreadPromotion::new(
        f.service.clone(),
        folders.clone(),
        f.projections.clone(),
        Arc::new(UnavailableCommands),
        tasks.clone(),
    );
    let input = || serde_json::from_value(json!({"threadId":thread_id})).unwrap();
    let caller = tokio::spawn({
        let promotion = promotion.clone();
        let input = input();
        async move { promotion.create_from_thread(input).await }
    });
    started.changed().await.unwrap();
    let promoted = f
        .service
        .get_by_workspace_root(&folder.to_string_lossy(), false)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(promoted.id, scratch.project_id);
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let mut shutdown = Box::pin(promotion.shutdown());
    assert!(futures_util::poll!(shutdown.as_mut()).is_pending());
    assert!(matches!(
        promotion.create_from_thread(input()).await,
        Err(t3_project::Error::Unavailable(_))
    ));
    tasks.release.add_permits(1);
    shutdown.await;
    assert_eq!(
        f.projections
            .shell(thread_id)
            .await
            .unwrap()
            .unwrap()
            .project_id,
        scratch.project_id
    );
    assert_eq!(
        f.service
            .get_by_id(&promoted.id, false)
            .await
            .unwrap()
            .unwrap()
            .title
            .0,
        "Promoted"
    );
    folders.shutdown().await;
    f.shutdown().await;
}
