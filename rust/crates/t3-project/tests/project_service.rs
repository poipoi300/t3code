#![allow(clippy::unwrap_used)]
mod support;
use serde_json::json;
use support::*;
use t3_contracts::{base_schemas::ProjectId, project::ProjectMutation};
use t3_project::Error;

#[tokio::test]
async fn refuses_worktree_containers_on_create_and_root_change_without_committing() {
    let f = Fixture::new();
    let source = f.root("container-source");
    let git = t3_git::Git::new(&source).with_environment(git_environment(f.temp.path()));
    git.execute(&["init", "-b", "develop"], Default::default())
        .await
        .unwrap();
    std::fs::write(source.join("README.md"), "fixture").unwrap();
    git.execute(&["add", "README.md"], Default::default())
        .await
        .unwrap();
    git.execute(&["commit", "-m", "fixture"], Default::default())
        .await
        .unwrap();
    let container = f.root("container");
    git.execute(
        &[
            "clone",
            "--bare",
            &source.to_string_lossy(),
            &container.join(".bare").to_string_lossy(),
        ],
        Default::default(),
    )
    .await
    .unwrap();
    std::fs::write(container.join(".git"), "gitdir: ./.bare\n").unwrap();
    let git = t3_git::Git::new(&container).with_environment(git_environment(f.temp.path()));
    git.execute(
        &["worktree", "add", "develop", "develop"],
        Default::default(),
    )
    .await
    .unwrap();
    for bare in ["true", "false"] {
        git.execute(&["config", "core.bare", bare], Default::default())
            .await
            .unwrap();
        let error = f
            .service
            .create(f.create(
                &format!("container-{bare}"),
                &format!("create-{bare}"),
                &container,
            ))
            .await;
        assert!(
            matches!(error, Err(Error::Invalid(_))),
            "container core.bare={bare} was accepted"
        );
        assert_eq!(
            error.unwrap_err().to_string(),
            format!(
                "{} holds a repository's worktrees but is not a checkout itself. Add one of its worktrees as the project instead.",
                container.display()
            )
        );
        assert!(f.service.snapshot().await.unwrap().projects.is_empty());
        assert!(
            f.service
                .bootstrap_automatic(f.create(
                    "automatic",
                    &format!("automatic-{bare}"),
                    &container
                ))
                .await
                .unwrap()
                .is_none()
        );
    }
    let worktree = container.join("develop");
    let project = f
        .service
        .create(f.create("checkout", "create-checkout", &worktree))
        .await
        .unwrap();
    let update = serde_json::from_value(json!({"type":"project.update","commandId":"move-container","projectId":project.id,"workspaceRoot":container})).unwrap();
    assert!(matches!(
        f.service.update(update).await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(
        f.service
            .get_by_id(&project.id, false)
            .await
            .unwrap()
            .unwrap()
            .workspace_root,
        project.workspace_root
    );
    let ordinary = f.root("ordinary-auto");
    assert!(
        matches!(
            f.service
                .bootstrap_automatic(f.create("checkout", "duplicate-auto", &ordinary))
                .await,
            Err(Error::Invalid(_))
        ),
        "unrelated invariant errors must propagate"
    );
    let (automatic, created) = f
        .service
        .bootstrap_automatic(f.create("ordinary", "ordinary-auto", &ordinary))
        .await
        .unwrap()
        .unwrap();
    assert!(created);
    let (reused, created) = f
        .service
        .bootstrap_automatic(f.create("other-id", "reuse-auto", &ordinary))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reused.id, automatic.id);
    assert!(!created);
    let legacy = f.root("legacy-container");
    let legacy_project = f
        .service
        .create(f.create("legacy", "legacy-create", &legacy))
        .await
        .unwrap();
    git.execute(
        &[
            "clone",
            "--bare",
            &source.to_string_lossy(),
            &legacy.join(".bare").to_string_lossy(),
        ],
        Default::default(),
    )
    .await
    .unwrap();
    std::fs::write(legacy.join(".git"), "gitdir: ./.bare\n").unwrap();
    let legacy_git = f.service.git().at(&legacy);
    legacy_git
        .execute(
            &["worktree", "add", "checkout", "develop"],
            Default::default(),
        )
        .await
        .unwrap();
    assert!(legacy_git.is_worktree_container().await);
    let rename = serde_json::from_value(json!({"type":"project.update", "commandId":"legacy-rename", "projectId":legacy_project.id, "workspaceRoot":legacy_project.workspace_root, "title":"Renamed legacy project"})).unwrap();
    assert_eq!(
        f.service.update(rename).await.unwrap().title.0,
        "Renamed legacy project"
    );
    f.shutdown().await;
}

#[tokio::test]
async fn lifecycle_uses_receipts_and_keeps_user_directories() {
    let f = Fixture::new();
    let root = f.root("repo");
    std::fs::write(root.join("keep.txt"), "user data").unwrap();
    let created = f
        .service
        .create(f.create("p", "create", &root))
        .await
        .unwrap();
    assert_eq!(created.title.0, "p");
    let again = f
        .service
        .create(f.create("p", "create", &root))
        .await
        .unwrap();
    assert_eq!(again.id, created.id);
    let updated=f.service.mutate(serde_json::from_value(json!({"type":"project.update","projectId":"p","commandId":"update","title":"Renamed","pinned":true,"archived":true})).unwrap()).await.unwrap();
    assert_eq!(updated.title.0, "Renamed");
    assert_eq!(updated.pinned_at, Some(Some(f.clock.now())));
    assert_eq!(f.service.snapshot().await.unwrap().projects.len(), 1);
    let deleted = f
        .service
        .mutate(ProjectMutation::Delete(f.delete("p", "delete", false)))
        .await
        .unwrap();
    assert!(deleted.deleted_at.is_some());
    assert!(f.service.snapshot().await.unwrap().projects.is_empty());
    assert_eq!(
        std::fs::read_to_string(root.join("keep.txt")).unwrap(),
        "user data"
    );
    assert_eq!(
        f.service
            .delete(f.delete("p", "delete", false))
            .await
            .unwrap()
            .deleted_at,
        deleted.deleted_at
    );
    assert!(matches!(
        f.service.delete(f.delete("p", "new-delete", false)).await,
        Err(Error::NotFound(_))
    ));
    let counts=f.database.read(|db|Ok((db.query_row("SELECT COUNT(*) FROM orchestration_events WHERE aggregate_kind='project'",[],|r|r.get::<_,i64>(0))?,db.query_row("SELECT COUNT(*) FROM orchestration_command_receipts WHERE aggregate_kind='project'",[],|r|r.get::<_,i64>(0))?))).await.unwrap();
    assert_eq!(counts, (3, 4));
    f.shutdown().await;
}
use t3_orchestration::api::Clock;

#[tokio::test]
async fn rejected_conflict_is_replayed_after_owner_is_deleted() {
    let f = Fixture::new();
    let root = f.root("repo");
    f.service
        .create(f.create("a", "first", &root))
        .await
        .unwrap();
    assert!(matches!(
        f.service.create(f.create("b", "conflict", &root)).await,
        Err(Error::Conflict { .. })
    ));
    f.service
        .delete(f.delete("a", "delete", false))
        .await
        .unwrap();
    assert!(matches!(
        f.service.create(f.create("b", "conflict", &root)).await,
        Err(Error::Conflict { .. })
    ));
    assert!(
        f.service
            .get_by_id(&ProjectId("b".into()), true)
            .await
            .unwrap()
            .is_none()
    );
    let b = f
        .service
        .create(f.create("b", "fresh", &root))
        .await
        .unwrap();
    assert_eq!(b.id.0, "b");
    f.shutdown().await;
}
#[tokio::test]
async fn concurrent_workspace_claims_and_receipt_identity_are_enforced() {
    let f = Fixture::new();
    let root = f.root("repo");
    let (a, b) = tokio::join!(
        f.service.create(f.create("a", "a", &root)),
        f.service.create(f.create("b", "b", &root))
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(
        a.as_ref().err().or(b.as_ref().err()),
        Some(Error::Conflict { .. })
    ));
    let wrong = f.service.create(f.create("c", "a", &f.root("other"))).await;
    assert!(wrong.is_err());
    assert_eq!(f.service.snapshot().await.unwrap().projects.len(), 1);
    f.shutdown().await;
}
#[tokio::test]
async fn event_and_receipt_roll_back_if_projection_write_fails() {
    let f = Fixture::new();
    let root = f.root("repo");
    f.database.transaction(|tx|{tx.execute_batch("CREATE TRIGGER reject_project BEFORE INSERT ON projection_projects BEGIN SELECT RAISE(ABORT,'fixture failure'); END;")?;Ok(())}).await.unwrap();
    assert!(
        f.service
            .create(f.create("p", "create", &root))
            .await
            .is_err()
    );
    let counts = f
        .database
        .read(|db| {
            Ok((
                db.query_row("SELECT COUNT(*) FROM orchestration_events", [], |r| {
                    r.get::<_, i64>(0)
                })?,
                db.query_row(
                    "SELECT COUNT(*) FROM orchestration_command_receipts",
                    [],
                    |r| r.get::<_, i64>(0),
                )?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(counts, (0, 0));
    f.database
        .transaction(|tx| {
            tx.execute_batch("DROP TRIGGER reject_project")?;
            Ok(())
        })
        .await
        .unwrap();
    f.service
        .create(f.create("p", "create", &root))
        .await
        .unwrap();
    f.shutdown().await;
}
#[tokio::test]
async fn bootstrap_and_page_preserve_creation_order() {
    let f = Fixture::new();
    let root = f.root("a");
    assert!(
        f.service
            .bootstrap(f.create("a", "a", &root))
            .await
            .unwrap()
            .1
    );
    assert!(
        !f.service
            .bootstrap(f.create("b", "b", &root))
            .await
            .unwrap()
            .1
    );
    f.clock.set("2026-09-04T12:01:00.000Z");
    f.service
        .create(f.create("c", "c", &f.root("c")))
        .await
        .unwrap();
    let (page, next) = f.service.list_page(0, 1).await.unwrap();
    assert_eq!(
        page.iter().map(|p| p.id.0.as_str()).collect::<Vec<_>>(),
        vec!["a"]
    );
    assert_eq!(next, Some(1));
    assert_eq!(f.service.list_page(1, 1).await.unwrap().0[0].id.0, "c");
    f.shutdown().await;
}
#[tokio::test]
async fn rejects_invalid_script_ids_and_remembers_the_rejection() {
    let f = Fixture::new();
    f.service
        .create(f.create("p", "create", &f.root("repo")))
        .await
        .unwrap();
    let invalid = json!({"type":"project.update","commandId":"bad","projectId":"p","scripts":[{"id":"BAD!","name":"test","command":"echo test","icon":"play","runOnWorktreeCreate":false}]});
    assert!(matches!(
        f.service
            .mutate(serde_json::from_value(invalid).unwrap())
            .await,
        Err(Error::Invalid(_))
    ));
    let corrected = json!({"type":"project.update","commandId":"bad","projectId":"p","scripts":[]});
    assert!(matches!(
        f.service
            .mutate(serde_json::from_value(corrected).unwrap())
            .await,
        Err(Error::Invalid(_))
    ));
    f.shutdown().await;
}
#[tokio::test]
async fn shutdown_refuses_late_mutations() {
    let f = Fixture::new();
    f.service.shutdown().await;
    assert!(matches!(
        f.service
            .mutate(ProjectMutation::Create(f.create(
                "p",
                "create",
                &f.root("repo")
            )))
            .await,
        Err(Error::Unavailable(_))
    ));
    f.shutdown().await;
}

async fn seed_thread(f: &Fixture, id: &str, at: &str, archived: bool) {
    use t3_orchestration::api::{CommitGuard, CommitRequest, Decision, EventCommitter};
    let event=serde_json::from_value(json!({
        "id":format!("created:{id}"),"type":"thread.created","threadId":id,
        "providerInstanceId":"codex","occurredAt":at,
        "payload":{"createdBy":"user","creationSource":"web","id":id,"projectId":"p","title":id,
        "providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"fixture"},
        "runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,
        "activeProviderThreadId":null,"lineage":{"parentThreadId":null,"relationshipToParent":null,"rootThreadId":id},
        "forkedFrom":null,"createdAt":at,"updatedAt":at,"archivedAt":if archived{Some(at)}else{None},
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
}
#[tokio::test]
async fn deletion_refuses_nonempty_and_retries_a_partial_cascade_newest_first() {
    use t3_contracts::base_schemas::ThreadId;
    use t3_orchestration::api::ProjectionReader;
    let f = Fixture::new();
    let root = f.root("repo");
    f.service
        .create(f.create("p", "create", &root))
        .await
        .unwrap();
    seed_thread(&f, "old", "2026-01-01T00:00:00.000Z", false).await;
    seed_thread(&f, "new", "2026-01-02T00:00:00.000Z", true).await;
    assert!(matches!(
        f.service.delete(f.delete("p", "refused", false)).await,
        Err(Error::NotEmpty(_))
    ));
    assert!(
        f.projections
            .thread(ThreadId("new".into()))
            .await
            .unwrap()
            .unwrap()
            .deleted_at
            .is_none()
    );
    f.database.transaction(|tx|{tx.execute_batch("CREATE TRIGGER reject_old_delete BEFORE INSERT ON orchestration_events WHEN NEW.event_type='thread.deleted' AND NEW.stream_id='old' BEGIN SELECT RAISE(ABORT,'fixture cascade failure'); END;")?;Ok(())}).await.unwrap();
    assert!(
        f.service
            .delete(f.delete("p", "cascade", true))
            .await
            .is_err()
    );
    assert!(
        f.projections
            .thread(ThreadId("new".into()))
            .await
            .unwrap()
            .unwrap()
            .deleted_at
            .is_some()
    );
    assert!(
        f.projections
            .thread(ThreadId("old".into()))
            .await
            .unwrap()
            .unwrap()
            .deleted_at
            .is_none()
    );
    assert!(
        f.service
            .get_by_id(&ProjectId("p".into()), false)
            .await
            .unwrap()
            .is_some()
    );
    f.database
        .transaction(|tx| {
            tx.execute_batch("DROP TRIGGER reject_old_delete")?;
            Ok(())
        })
        .await
        .unwrap();
    f.service
        .delete(f.delete("p", "cascade", true))
        .await
        .unwrap();
    f.service
        .delete(f.delete("p", "cascade", true))
        .await
        .unwrap();
    let deleted=f.database.read(|db|Ok(db.prepare("SELECT stream_id FROM orchestration_events WHERE event_type='thread.deleted' ORDER BY sequence")?.query_map([],|r|r.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?)).await.unwrap();
    assert_eq!(deleted, vec!["new", "old"]);
    assert!(root.exists());
    f.shutdown().await;
}
