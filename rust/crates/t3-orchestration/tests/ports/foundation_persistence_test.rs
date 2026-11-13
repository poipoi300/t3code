use t3_contracts::base_schemas::*;
use t3_orchestration::{
    effects::*,
    error::Result,
    stores::{
        StoreDatabase,
        event_store::{EventFilter, EventStore},
        outbox, projects,
        receipts::{self, AggregateId, CommandReceipt, ReceiptStatus},
    },
    testing::{TEST_TIME, TestDatabase, detached_event, project_event},
};
fn pending(id: &str, thread: &str, request: EffectRequest) -> PendingEffect {
    PendingEffect {
        id: id.into(),
        command_id: CommandId("command".into()),
        thread_id: ThreadId(thread.into()),
        request,
        available_at: None,
    }
}
fn cleanup(id: &str, thread: &str) -> PendingEffect {
    pending(id, thread, EffectRequest::CleanupTerminals)
}
fn claim(worker: &str) -> ClaimOptions {
    ClaimOptions {
        worker_id: worker.into(),
        ..Default::default()
    }
}
fn receipt() -> CommandReceipt {
    CommandReceipt {
        command_id: CommandId("command".into()),
        aggregate: AggregateId::Thread(ThreadId("thread".into())),
        command_type: "message.dispatch".into(),
        accepted_at: TEST_TIME.into(),
        result_sequence: 1,
        status: ReceiptStatus::Accepted,
        error: None,
    }
}
#[tokio::test]
async fn receipts_insert_once_upsert_explicitly_and_separate_project_from_thread() -> Result<()> {
    let f = TestDatabase::memory()?;
    f.stores
        .transaction(|tx| {
            let r = receipt();
            assert!(receipts::insert_if_absent(tx, &r)?);
            let mut rejected = r.clone();
            rejected.status = ReceiptStatus::Rejected;
            rejected.error = Some("rejected".into());
            assert!(!receipts::insert_if_absent(tx, &rejected)?);
            assert_eq!(receipts::get_thread(tx, &r.command_id)?, Some(r.clone()));
            assert!(receipts::get_project(tx, &r.command_id)?.is_none());
            rejected.aggregate = AggregateId::Project(ProjectId("project".into()));
            receipts::upsert(tx, &rejected)?;
            assert!(receipts::get_thread(tx, &r.command_id)?.is_none());
            assert_eq!(receipts::get_project(tx, &r.command_id)?, Some(rejected));
            Ok(())
        })
        .await
}
#[tokio::test]
async fn transaction_rolls_back_events_projection_receipt_and_outbox_together() -> Result<()> {
    let f = TestDatabase::memory()?;
    let outcome: Result<()> = f
        .stores
        .transaction(|tx| {
            EventStore::new().append(tx, None, &[detached_event("event", "thread")?])?;
            projects::apply(tx, &project_event("project-event", "project")?)?;
            receipts::insert_if_absent(tx, &receipt())?;
            outbox::enqueue(tx, &[cleanup("effect", "thread")], TEST_TIME)?;
            t3_orchestration::stores::positions::allocate(
                tx,
                &ThreadId("thread".into()),
                &TurnItemId("item".into()),
                None,
                None,
                Default::default(),
            )?;
            tx.execute(
                "INSERT INTO projection_projects(project_id) VALUES ('invalid')",
                [],
            )?;
            Ok(())
        })
        .await;
    assert!(outcome.is_err());
    f.stores
        .read(|db| {
            for table in [
                "orchestration_events",
                "projection_projects",
                "orchestration_command_receipts",
                "orchestration_v2_effect_outbox",
                "orchestration_v2_turn_item_positions",
            ] {
                assert_eq!(
                    db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))?,
                    0
                );
            }
            Ok(())
        })
        .await
}

#[tokio::test]
async fn dropping_a_transaction_future_does_not_cancel_an_already_started_sqlite_commit()
-> Result<()> {
    let f = TestDatabase::memory()?;
    let stores = f.stores.clone();
    let (started, wait_started) = tokio::sync::oneshot::channel();
    let (release, wait_release) = std::sync::mpsc::channel();
    let task = tokio::spawn(async move {
        stores
            .transaction(move |tx| {
                EventStore::new().append(
                    tx,
                    Some(&CommandId("command".into())),
                    &[detached_event("committed", "thread")?],
                )?;
                projects::apply(tx, &project_event("project", "project")?)?;
                receipts::insert_if_absent(tx, &receipt())?;
                outbox::enqueue(tx, &[cleanup("effect", "thread")], TEST_TIME)?;
                let _ = started.send(());
                let _ = wait_release.recv();
                Ok(())
            })
            .await
    });
    wait_started.await.expect("transaction started");
    task.abort();
    assert!(task.await.expect_err("caller cancelled").is_cancelled());
    let _ = release.send(());
    f.stores
        .transaction(|tx| {
            for table in [
                "orchestration_events",
                "projection_projects",
                "orchestration_command_receipts",
                "orchestration_v2_effect_outbox",
            ] {
                assert_eq!(
                    tx.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                        .get::<_, i64>(0))?,
                    1
                );
            }
            Ok(())
        })
        .await
}
#[tokio::test]
async fn enqueue_is_idempotent_and_rows_preserve_nulls_json_and_owner_guards() -> Result<()> {
    let f = TestDatabase::memory()?;
    f.stores
        .transaction(|tx| {
            let effect = cleanup("effect", "thread");
            outbox::enqueue(tx, std::slice::from_ref(&effect), TEST_TIME)?;
            outbox::enqueue(
                tx,
                &[PendingEffect {
                    thread_id: ThreadId("other".into()),
                    request: EffectRequest::CleanupPreviews,
                    ..effect
                }],
                "2026-09-04T00:00:00.000Z",
            )?;
            let row = outbox::get(tx, "effect")?.expect("effect");
            assert_eq!(row.request, EffectRequest::CleanupTerminals);
            assert_eq!(row.thread_id, ThreadId("thread".into()));
            assert_eq!(row.created_at, TEST_TIME);
            assert_eq!(row.attempt_count, 0);
            assert_eq!(row.lease_owner, None);
            assert_eq!(row.completed_at, None);
            let raw: String = tx.query_row(
                "SELECT payload_json FROM orchestration_v2_effect_outbox",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(raw, r#"{"type":"terminal.cleanup"}"#);
            let row = outbox::claim_next(tx, &claim("worker"), TEST_TIME)?.expect("claimed");
            assert_eq!(row.attempt_count, 1);
            assert_eq!(row.lease_owner.as_deref(), Some("worker"));
            assert_eq!(
                tx.query_row(
                    "SELECT lease_expires_at FROM orchestration_v2_effect_outbox",
                    [],
                    |row| row.get::<_, Option<String>>(0)
                )?,
                None
            );
            assert!(!outbox::succeed(tx, "effect", "wrong", TEST_TIME)?);
            assert!(!outbox::retry(
                tx, "effect", "wrong", "error", TEST_TIME, TEST_TIME
            )?);
            assert!(!outbox::fail(tx, "effect", "wrong", "error", TEST_TIME)?);
            assert!(outbox::succeed(tx, "effect", "worker", TEST_TIME)?);
            assert!(!outbox::succeed(tx, "effect", "worker", TEST_TIME)?);
            assert_eq!(
                outbox::list_by_command(tx, &CommandId("command".into()))?.len(),
                1
            );
            Ok(())
        })
        .await
}
#[tokio::test]
async fn delayed_retry_blocks_later_rowids_within_lane_but_not_other_threads_or_titles()
-> Result<()> {
    let f = TestDatabase::memory()?;
    f.stores
        .transaction(|tx| {
            outbox::enqueue(
                tx,
                &[
                    PendingEffect {
                        available_at: Some("2026-09-02T00:00:00.000Z".into()),
                        ..cleanup("z-first", "thread")
                    },
                    cleanup("a-second", "thread"),
                    cleanup("other", "other"),
                    pending(
                        "title",
                        "thread",
                        EffectRequest::GenerateTitle {
                            kind: TitleKind::Regenerate,
                        },
                    ),
                ],
                TEST_TIME,
            )?;
            assert_eq!(
                outbox::claim_next(tx, &claim("worker"), TEST_TIME)?
                    .expect("first")
                    .id,
                "z-first"
            );
            assert!(outbox::retry(
                tx,
                "z-first",
                "worker",
                "retry",
                TEST_TIME,
                "2026-09-04T00:00:00.000Z"
            )?);
            let mut ids = Vec::new();
            while let Some(row) = outbox::claim_next(tx, &claim("worker"), TEST_TIME)? {
                ids.push(row.id);
            }
            ids.sort();
            assert_eq!(ids, ["other", "title"]);
            assert_eq!(
                outbox::next_claimable_at(tx, None)?.as_deref(),
                Some("2026-09-04T00:00:00.000Z")
            );
            assert_eq!(
                outbox::claim_next(tx, &claim("worker"), "2026-09-04T00:00:00.000Z")?
                    .expect("retry")
                    .id,
                "z-first"
            );
            assert!(
                outbox::claim_next(tx, &claim("worker"), "2099-01-01T00:00:00.000Z")?.is_none()
            );
            Ok(())
        })
        .await
}
#[tokio::test]
async fn titles_respect_global_cap_and_do_not_block_critical_work() -> Result<()> {
    let f = TestDatabase::memory()?;
    f.stores
        .transaction(|tx| {
            for (id, thread) in [("title-a", "a"), ("title-b", "b")] {
                outbox::enqueue(
                    tx,
                    &[pending(
                        id,
                        thread,
                        EffectRequest::GenerateTitle {
                            kind: TitleKind::Regenerate,
                        },
                    )],
                    TEST_TIME,
                )?;
            }
            let options = ClaimOptions {
                max_running_title_generations: Some(1),
                ..claim("worker")
            };
            assert_eq!(
                outbox::claim_next(tx, &options, TEST_TIME)?
                    .expect("title")
                    .id,
                "title-a"
            );
            assert!(outbox::next_claimable_at(tx, Some(1))?.is_none());
            assert!(outbox::claim_next(tx, &options, TEST_TIME)?.is_none());
            outbox::enqueue(tx, &[cleanup("turn", "a")], TEST_TIME)?;
            assert_eq!(
                outbox::claim_next(tx, &options, TEST_TIME)?
                    .expect("critical")
                    .id,
                "turn"
            );
            Ok(())
        })
        .await
}
#[tokio::test]
async fn recovery_claim_skips_continuations_and_their_predecessor_blocking() -> Result<()> {
    let f = TestDatabase::memory()?;
    f.stores
        .transaction(|tx| {
            outbox::enqueue(
                tx,
                &[
                    pending(
                        "continue",
                        "thread",
                        EffectRequest::Continue {
                            source_run_id: RunId("run".into()),
                        },
                    ),
                    cleanup("later", "thread"),
                ],
                TEST_TIME,
            )?;
            let options = ClaimOptions {
                exclude_restart_continuations: true,
                ..claim("worker")
            };
            assert_eq!(
                outbox::claim_next(tx, &options, TEST_TIME)?
                    .expect("later")
                    .id,
                "later"
            );
            assert_eq!(
                outbox::get(tx, "continue")?.expect("pending").status,
                EffectStatus::Pending
            );
            Ok(())
        })
        .await
}
#[tokio::test]
async fn cancelled_lease_cannot_settle_and_unblocks_successors() -> Result<()> {
    let f = TestDatabase::memory()?;
    f.stores
        .transaction(|tx| {
            outbox::enqueue(
                tx,
                &[
                    cleanup("first", "thread"),
                    pending("second", "thread", EffectRequest::CleanupPreviews),
                ],
                TEST_TIME,
            )?;
            outbox::claim_next(tx, &claim("worker"), TEST_TIME)?;
            assert_eq!(
                outbox::cancel_unsettled(
                    tx,
                    &ThreadId("thread".into()),
                    &["terminal.cleanup"],
                    "stop",
                    TEST_TIME
                )?,
                ["first"]
            );
            assert!(!outbox::succeed(tx, "first", "worker", TEST_TIME)?);
            assert_eq!(
                outbox::claim_next(tx, &claim("worker"), TEST_TIME)?
                    .expect("second")
                    .id,
                "second"
            );
            Ok(())
        })
        .await
}
#[tokio::test]
async fn restart_retires_pending_process_work_and_requeues_safe_running_work() -> Result<()> {
    let f = TestDatabase::temporary()?;
    let directory = f.directory.expect("directory");
    let path = directory.path().join("statev2.sqlite");
    f.stores
        .transaction(|tx| {
            outbox::enqueue(
                tx,
                &[
                    PendingEffect {
                        available_at: Some("2026-09-02T00:00:00.000Z".into()),
                        ..cleanup("safe", "a")
                    },
                    pending(
                        "bound",
                        "b",
                        EffectRequest::Start {
                            run_id: RunId("run".into()),
                        },
                    ),
                ],
                TEST_TIME,
            )?;
            assert!(outbox::claim_next(tx, &claim("worker"), TEST_TIME)?.is_some());
            Ok(())
        })
        .await?;
    f.stores.0.close().await?;
    let reopened = StoreDatabase(t3_persistence::Database::open(path)?);
    reopened
        .transaction(|tx| {
            let result = outbox::reconcile_after_process_loss(tx, TEST_TIME)?;
            assert_eq!(
                result,
                Reconciliation {
                    requeued: 1,
                    cancelled: 1
                }
            );
            assert_eq!(
                outbox::get(tx, "bound")?.expect("bound").status,
                EffectStatus::Cancelled
            );
            let safe = outbox::get(tx, "safe")?.expect("safe");
            assert_eq!(safe.status, EffectStatus::Pending);
            assert!(safe.lease_owner.is_none());
            Ok(())
        })
        .await?;
    reopened.0.close().await?;
    Ok(())
}
#[tokio::test]
async fn pruning_batches_only_old_success_and_cancelled_while_retaining_failed_rows() -> Result<()>
{
    let f = TestDatabase::memory()?;
    f.stores
        .transaction(|tx| {
            for i in 0..503 {
                outbox::enqueue(
                    tx,
                    &[cleanup(&format!("e-{i:03}"), &format!("thread-{i}"))],
                    TEST_TIME,
                )?;
            }
            for i in 0..503 {
                let row = outbox::claim_next(tx, &claim("worker"), TEST_TIME)?.expect("row");
                if i == 502 {
                    outbox::fail(tx, &row.id, "worker", "failure", TEST_TIME)?;
                } else {
                    outbox::succeed(tx, &row.id, "worker", TEST_TIME)?;
                }
            }
            assert_eq!(
                outbox::prune_settled_batch(tx, "2026-09-10T00:00:00.000Z")?,
                500
            );
            assert_eq!(
                outbox::prune_settled_batch(tx, "2026-09-10T00:00:00.000Z")?,
                2
            );
            assert_eq!(
                outbox::list_by_command(tx, &CommandId("command".into()))?.len(),
                1
            );
            Ok(())
        })
        .await
}
#[tokio::test]
async fn duplicate_event_id_failure_rolls_back_whole_batch_in_transaction() -> Result<()> {
    let f = TestDatabase::memory()?;
    let result: Result<()> = f
        .stores
        .transaction(|tx| {
            let event = detached_event("duplicate", "thread")?;
            EventStore::new().append(tx, None, &[event.clone(), event])?;
            Ok(())
        })
        .await;
    assert!(result.is_err());
    f.stores
        .read(|db| {
            assert!(
                EventStore::new()
                    .read(db, &EventFilter::default())?
                    .is_empty()
            );
            Ok(())
        })
        .await
}

#[tokio::test]
async fn each_outbox_lane_serializes_and_blocked_deadlines_do_not_wake_claimers() -> Result<()> {
    let f = TestDatabase::memory()?;
    f.stores
        .transaction(|tx| {
            let title = |id| {
                pending(
                    id,
                    "thread",
                    EffectRequest::GenerateTitle {
                        kind: TitleKind::Regenerate,
                    },
                )
            };
            outbox::enqueue(
                tx,
                &[
                    title("a-title"),
                    cleanup("b-critical", "thread"),
                    title("c-title"),
                    cleanup("d-critical", "thread"),
                    PendingEffect {
                        available_at: Some("2026-09-04T00:00:00.000Z".into()),
                        ..cleanup("future", "future")
                    },
                ],
                TEST_TIME,
            )?;
            assert_eq!(
                outbox::claim_next(tx, &claim("title"), TEST_TIME)?
                    .expect("title")
                    .id,
                "a-title"
            );
            assert_eq!(
                outbox::claim_next(tx, &claim("critical"), TEST_TIME)?
                    .expect("critical")
                    .id,
                "b-critical"
            );
            assert!(outbox::claim_next(tx, &claim("blocked"), TEST_TIME)?.is_none());
            assert_eq!(
                outbox::next_claimable_at(tx, None)?.as_deref(),
                Some("2026-09-04T00:00:00.000Z")
            );
            assert!(outbox::succeed(tx, "b-critical", "critical", TEST_TIME)?);
            assert_eq!(
                outbox::next_claimable_at(tx, None)?.as_deref(),
                Some(TEST_TIME)
            );
            assert_eq!(
                outbox::claim_next(tx, &claim("critical"), TEST_TIME)?
                    .expect("next critical")
                    .id,
                "d-critical"
            );
            assert!(outbox::succeed(tx, "d-critical", "critical", TEST_TIME)?);
            assert!(outbox::succeed(tx, "a-title", "title", TEST_TIME)?);
            assert_eq!(
                outbox::claim_next(tx, &claim("title"), TEST_TIME)?
                    .expect("next title")
                    .id,
                "c-title"
            );
            assert!(outbox::succeed(tx, "c-title", "title", TEST_TIME)?);
            Ok(())
        })
        .await
}

#[tokio::test]
async fn pruning_releases_each_batch_and_preserves_cutoff_recent_failed_and_unsettled_rows()
-> Result<()> {
    let f = TestDatabase::temporary()?;
    f.stores.transaction(|tx| {
        for (prefix, count, status, completed) in [
            ("succeeded-old:", 1201, "succeeded", Some("2026-09-02T23:59:00.000Z")),
            ("cancelled-old:", 2, "cancelled", Some("2026-09-02T23:59:00.000Z")),
            ("succeeded-recent:", 1, "succeeded", Some("2026-09-03T00:01:00.000Z")),
            ("at-cutoff:", 1, "succeeded", Some(TEST_TIME)),
            ("failed-old:", 1, "failed", Some("2026-09-02T23:59:00.000Z")),
            ("pending:", 1, "pending", None),
            ("running:", 1, "running", None),
        ] {
            tx.execute("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<?2) INSERT INTO orchestration_v2_effect_outbox(effect_id,command_id,thread_id,effect_type,payload_json,status,available_at,created_at,updated_at,completed_at) SELECT ?1||i,'command:prune','thread:prune','terminal.cleanup','{\"type\":\"terminal.cleanup\"}',?3,?4,?4,?4,?5 FROM n", t3_persistence::rusqlite::params![prefix,count,status,TEST_TIME,completed])?;
        }
        Ok(())
    }).await?;
    assert_eq!(outbox::prune_settled(&f.stores, TEST_TIME).await?, 1203);
    f.stores
        .read(|db| {
            let ids = db
                .prepare("SELECT effect_id FROM orchestration_v2_effect_outbox ORDER BY effect_id")?
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            assert_eq!(
                ids,
                [
                    "at-cutoff:1",
                    "failed-old:1",
                    "pending:1",
                    "running:1",
                    "succeeded-recent:1"
                ]
            );
            Ok(())
        })
        .await?;
    assert_eq!(outbox::prune_settled(&f.stores, TEST_TIME).await?, 0);
    Ok(())
}

#[tokio::test]
async fn process_loss_classifies_every_effect_without_reclaiming_settled_or_pending_safe_work()
-> Result<()> {
    let f = TestDatabase::memory()?;
    f.stores.transaction(|tx| {
        let requests: Vec<EffectRequest> = serde_json::from_value(serde_json::json!([
            {"type":"provider-runtime.continue","sourceRunId":"run"},
            {"type":"provider-session.detach","providerSessionId":"session"},
            {"type":"provider-session.apply-model-selection","providerSessionId":"session","providerThreadId":"provider-thread","modelSelection":{"instanceId":"codex","model":"model"}},
            {"type":"provider-turn.start","runId":"run"},
            {"type":"provider-turn.interrupt","providerSessionId":"session","providerThreadId":"provider-thread","providerTurnId":"turn"},
            {"type":"provider-turn.steer","providerSessionId":"session","providerThreadId":"provider-thread","providerTurnId":"turn","messageId":"message"},
            {"type":"provider-turn.restart","providerSessionId":"session","providerThreadId":"provider-thread","providerTurnId":"turn","interruptedAttemptId":"attempt","runId":"run"},
            {"type":"runtime-request.respond","providerSessionId":"session","requestId":"request"},
            {"type":"provider-thread.rollback","providerThreadId":"provider-thread","checkpointId":"checkpoint","scopeId":"scope"},
            {"type":"checkpoint.capture","runId":"run","scopeId":"scope"},
            {"type":"scheduled-tasks.cleanup"},{"type":"terminal.cleanup"},{"type":"preview.cleanup"},
            {"type":"attachment.cleanup","attachmentIds":["attachment"]},
            {"type":"thread-title.generate","kind":{"type":"regenerate"}},
            {"type":"delegated-tasks.stop"}
        ]))?;
        let future = "2026-09-04T00:00:00.000Z";
        let process_bound: Vec<&str> = store_oracle()["processLoss"]["processBound"]
            .as_array()
            .expect("process-bound types")
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect();
        for (index, request) in requests.iter().enumerate() {
            for status in ["pending", "running", "succeeded", "failed", "cancelled"] {
                let id = format!("{index}:{status}");
                outbox::enqueue(tx, &[PendingEffect { available_at: Some(future.into()), ..pending(&id, &id, request.clone()) }], TEST_TIME)?;
                tx.execute("UPDATE orchestration_v2_effect_outbox SET status=?1,lease_owner=CASE WHEN ?1='running' THEN 'crashed' ELSE NULL END,attempt_count=CASE WHEN ?1='running' THEN 1 ELSE 0 END,completed_at=CASE WHEN ?1 IN ('succeeded','failed','cancelled') THEN ?2 ELSE NULL END WHERE effect_id=?3", t3_persistence::rusqlite::params![status,TEST_TIME,id])?;
            }
        }
        assert_eq!(outbox::reconcile_after_process_loss(tx, TEST_TIME)?, Reconciliation { cancelled: 12, requeued: 10 });
        for (index, request) in requests.iter().enumerate() {
            for status in ["pending", "running", "succeeded", "failed", "cancelled"] {
                let row = outbox::get(tx, &format!("{index}:{status}"))?.expect("effect");
                assert_eq!(&row.request, request);
                let was_unsettled = matches!(status, "pending" | "running");
                if process_bound.contains(&request.effect_type()) && was_unsettled {
                    assert_eq!(row.status, EffectStatus::Cancelled);
                    assert_eq!(row.completed_at.as_deref(), Some(TEST_TIME));
                    assert_eq!(row.last_error.as_deref(), Some("Cancelled because the server process ended before the effect completed."));
                } else if status == "running" {
                    assert_eq!(row.status, EffectStatus::Pending);
                    assert_eq!(row.available_at, TEST_TIME);
                    assert_eq!(row.attempt_count, 1);
                    assert_eq!(row.last_error.as_deref(), Some("Requeued after the previous server process ended."));
                } else {
                    assert_eq!(serde_json::to_value(row.status)?, status);
                    assert_eq!(row.available_at, future);
                }
                assert!(row.lease_owner.is_none());
            }
        }
        let before = outbox::get(tx, "0:running")?.expect("safe");
        assert_eq!(outbox::claim_next(tx, &ClaimOptions { exclude_restart_continuations: true, ..claim("recovery") }, TEST_TIME)?.expect("safe cleanup").attempt_count, 2);
        assert_eq!(outbox::get(tx, "0:running")?, Some(before));
        Ok(())
    }).await
}

// ---------------------------------------------------------------------------------------------
// Real races: two connections in autocommit mode (as the TypeScript store claims), released
// together by a barrier, many rounds. The control shows the harness detects a double claim.
// ---------------------------------------------------------------------------------------------

fn raw_connection(path: &std::path::Path) -> Result<t3_persistence::rusqlite::Connection> {
    let connection = t3_persistence::rusqlite::Connection::open(path)?;
    connection.busy_timeout(std::time::Duration::from_secs(10))?;
    Ok(connection)
}

fn race<T: Send + 'static>(
    path: &std::path::Path,
    contender: impl Fn(&t3_persistence::rusqlite::Connection, usize, &std::sync::Barrier) -> Result<T>
    + Send
    + Sync
    + 'static,
) -> Result<[T; 2]> {
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let contender = std::sync::Arc::new(contender);
    let handles: Vec<_> = (0..2)
        .map(|index| {
            let (barrier, contender, path) = (barrier.clone(), contender.clone(), path.to_owned());
            std::thread::spawn(move || -> Result<T> {
                let connection = raw_connection(&path)?;
                contender(&connection, index, &barrier)
            })
        })
        .collect();
    let mut results = handles
        .into_iter()
        .map(|handle| handle.join().expect("contender thread"))
        .collect::<Result<Vec<T>>>()?
        .into_iter();
    Ok([
        results.next().expect("first"),
        results.next().expect("second"),
    ])
}

fn database_path(f: &TestDatabase) -> std::path::PathBuf {
    f.directory
        .as_ref()
        .expect("temporary")
        .path()
        .join("statev2.sqlite")
}

#[tokio::test]
async fn racing_autocommit_claims_on_two_connections_claim_each_effect_once() -> Result<()> {
    let f = TestDatabase::temporary()?;
    let path = database_path(&f);
    // Control: a select-then-update claim, interleaved by the barrier, claims the same row twice.
    raw_connection(&path)?.execute_batch(
        "INSERT INTO orchestration_v2_effect_outbox (effect_id, command_id, thread_id, effect_type, \
         payload_json, status, attempt_count, available_at, created_at, updated_at) VALUES \
         ('control', 'command', 'control', 'terminal.cleanup', '{\"type\":\"terminal.cleanup\"}', \
         'pending', 0, '2026-09-03T00:00:00.000Z', '2026-09-03T00:00:00.000Z', '2026-09-03T00:00:00.000Z')",
    )?;
    let naive = race(&path, |db, index, barrier| {
        let id: String = db.query_row(
            "SELECT effect_id FROM orchestration_v2_effect_outbox WHERE status = 'pending' LIMIT 1",
            [],
            |row| row.get(0),
        )?;
        barrier.wait();
        db.execute(
            "UPDATE orchestration_v2_effect_outbox SET status = 'running', lease_owner = ?1 WHERE effect_id = ?2",
            t3_persistence::rusqlite::params![format!("naive:{index}"), id],
        )?;
        Ok(id)
    })?;
    assert_eq!(
        naive[0], naive[1],
        "the harness must interleave a two-step claim"
    );
    for round in 0..40 {
        let id = format!("effect:{round}");
        f.stores
            .transaction({
                let id = id.clone();
                move |tx| outbox::enqueue(tx, &[cleanup(&id, &id)], TEST_TIME)
            })
            .await?;
        let claimed = race(&path, |db, index, barrier| {
            barrier.wait();
            Ok(
                outbox::claim_next(db, &claim(&format!("worker:{index}")), TEST_TIME)?
                    .map(|row| row.id),
            )
        })?;
        let winners: Vec<_> = claimed.iter().flatten().collect();
        assert_eq!(winners, [&id], "round {round}: {claimed:?}");
    }
    Ok(())
}

#[tokio::test]
async fn racing_receipt_reservations_have_one_winner_and_survive_event_pruning() -> Result<()> {
    let f = TestDatabase::temporary()?;
    let path = database_path(&f);
    for round in 0..40 {
        let inserted = race(&path, move |db, index, barrier| {
            let mut competing = receipt();
            competing.command_id = CommandId(format!("command:{round}"));
            if index == 1 {
                competing.aggregate = AggregateId::Project(ProjectId("project".into()));
            }
            barrier.wait();
            receipts::insert_if_absent(db, &competing)
        })?;
        assert_ne!(inserted[0], inserted[1], "round {round}");
        let winner = f
            .stores
            .read(move |db| receipts::get(db, &CommandId(format!("command:{round}"))))
            .await?
            .expect("receipt");
        assert_eq!(
            winner.aggregate.kind(),
            if inserted[0] { "thread" } else { "project" }
        );
    }
    f.stores
        .transaction(|tx| {
            let store = EventStore::new();
            let command = CommandId("command:0".into());
            store.append(tx, Some(&command), &[detached_event("pruned", "thread")?])?;
            tx.execute("DELETE FROM orchestration_events", [])?;
            let mut retry = receipt();
            retry.command_id = command.clone();
            let before = receipts::get(tx, &command)?;
            assert!(!receipts::insert_if_absent(tx, &retry)?);
            assert_eq!(receipts::get(tx, &command)?, before);
            Ok(())
        })
        .await
}

/// `claim_next` runs inside the worker's transaction. An undecodable row keeps its claim (running,
/// leased, as the TypeScript autocommit leaves it) instead of rolling back and being selected on
/// every poll, so other threads proceed and only the poisoned lane stays blocked.
#[tokio::test]
async fn an_undecodable_claim_blocks_only_its_own_lane() -> Result<()> {
    let f = TestDatabase::memory()?;
    f.stores
        .transaction(|tx| {
            tx.execute_batch(
                "INSERT INTO orchestration_v2_effect_outbox (effect_id, command_id, thread_id, \
                 effect_type, payload_json, status, attempt_count, available_at, created_at, \
                 updated_at) VALUES ('poisoned', 'command', 'poisoned-thread', 'future.effect', \
                 '{\"type\":\"future.effect\"}', 'pending', 0, '2026-09-02T00:00:00.000Z', \
                 '2026-09-02T00:00:00.000Z', '2026-09-02T00:00:00.000Z')",
            )?;
            outbox::enqueue(
                tx,
                &[
                    cleanup("poisoned-thread:later", "poisoned-thread"),
                    cleanup("a:1", "a"),
                    cleanup("b:1", "b"),
                    cleanup("a:2", "a"),
                ],
                TEST_TIME,
            )
        })
        .await?;
    let claim_as_worker = |worker: &'static str| {
        f.stores
            .transaction(move |tx| outbox::claim_next(tx, &claim(worker), TEST_TIME))
    };
    assert_eq!(claim_as_worker("worker").await?.expect("healthy").id, "a:1");
    let poisoned = f
        .stores
        .read(|db| {
            Ok(db.query_row(
                "SELECT status, lease_owner, attempt_count, last_error FROM orchestration_v2_effect_outbox WHERE effect_id = 'poisoned'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, i64>(2)?, row.get::<_, Option<String>>(3)?)),
            )?)
        })
        .await?;
    assert_eq!(poisoned, ("running".into(), Some("worker".into()), 1, None));
    assert_eq!(
        claim_as_worker("worker").await?.expect("other lane").id,
        "b:1"
    );
    assert!(
        claim_as_worker("worker").await?.is_none(),
        "a:2 waits for a:1, the poisoned lane waits"
    );
    f.stores
        .transaction(|tx| outbox::succeed(tx, "a:1", "worker", TEST_TIME))
        .await?;
    assert_eq!(
        claim_as_worker("worker").await?.expect("next in lane").id,
        "a:2"
    );
    let report = f
        .stores
        .transaction(|tx| {
            tx.execute(
                "UPDATE orchestration_v2_effect_outbox SET status = 'pending', lease_owner = NULL WHERE effect_id = 'poisoned'",
                [],
            )?;
            outbox::claim_next_reporting(tx, &claim("reporter"), TEST_TIME)
        })
        .await?;
    assert!(report.effect.is_none());
    assert_eq!(report.undecodable.len(), 1);
    assert_eq!(report.undecodable[0].effect_id, "poisoned");
    assert_eq!(
        report.undecodable[0].thread_id,
        ThreadId("poisoned-thread".into())
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Outbox and receipt oracles: the TypeScript operation log replayed through the Rust stores.
// Timestamps are given to Rust in another spelling of the same instant, so the canonical text in
// the snapshots also proves the store canonicalises them as `DateTime.formatIso` does.
// ---------------------------------------------------------------------------------------------

use crate::persistence_orchestration_event_store_test::{
    assert_rows, assert_same_json, snapshot, store_oracle,
};
use serde_json::{Value, json};

fn respell(at: &str) -> String {
    at.replace(".000Z", "Z")
}

fn effect_json(row: &EffectRow) -> Result<Value> {
    Ok(json!({
        "id": row.id,
        "commandId": row.command_id,
        "threadId": row.thread_id,
        "request": serde_json::to_value(&row.request)?,
        "status": serde_json::to_value(row.status)?,
        "attemptCount": row.attempt_count,
        "availableAt": row.available_at,
        "leaseOwner": row.lease_owner,
        "createdAt": row.created_at,
        "updatedAt": row.updated_at,
        "completedAt": row.completed_at,
        "lastError": row.last_error,
    }))
}

fn optional_effect_json(row: Option<&EffectRow>) -> Result<Value> {
    row.map(effect_json)
        .transpose()
        .map(|row| row.unwrap_or(Value::Null))
}

fn replay_outbox(
    tx: &t3_persistence::rusqlite::Connection,
    ops: &Value,
    results: &Value,
) -> Result<()> {
    let mut claims: Vec<(String, String)> = Vec::new();
    let text = |value: &Value| value.as_str().expect("string").to_owned();
    for (index, (op, expected)) in ops
        .as_array()
        .expect("ops")
        .iter()
        .zip(results.as_array().expect("results"))
        .enumerate()
    {
        let what = format!("outbox op {index} ({})", op["op"]);
        let at = op["at"].as_str().map(respell).unwrap_or_default();
        match op["op"].as_str().expect("op") {
            "enqueue" => {
                let effects = op["effects"]
                    .as_array()
                    .expect("effects")
                    .iter()
                    .map(|effect| {
                        Ok(PendingEffect {
                            id: text(&effect["id"]),
                            command_id: CommandId(text(&effect["commandId"])),
                            thread_id: ThreadId(text(&effect["threadId"])),
                            request: serde_json::from_value(effect["request"].clone())?,
                            available_at: effect["availableAt"].as_str().map(respell),
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                outbox::enqueue(tx, &effects, &at)?;
            }
            "snapshot" => assert_rows(
                &snapshot(tx, "orchestration_v2_effect_outbox")?,
                expected,
                &what,
            ),
            "insert" => tx.execute_batch(op["sql"].as_str().expect("sql"))?,
            "claim" => {
                let worker = text(&op["workerId"]);
                let options = ClaimOptions {
                    worker_id: worker.clone(),
                    exclude_restart_continuations: op["excludeRestartContinuations"]
                        .as_bool()
                        .unwrap_or(false),
                    max_running_title_generations: op["maxRunningTitleGenerations"].as_i64(),
                };
                let claimed = outbox::claim_next(tx, &options, &at)?;
                claims.push((
                    claimed
                        .as_ref()
                        .map(|row| row.id.clone())
                        .unwrap_or_default(),
                    worker,
                ));
                assert_same_json(&optional_effect_json(claimed.as_ref())?, expected, &what);
            }
            "claimDecodable" => {
                let claimed = outbox::claim_next_reporting(
                    tx,
                    &claim(op["workerId"].as_str().expect("worker")),
                    &at,
                )?;
                assert_eq!(
                    Some(claimed.undecodable.len() as u64),
                    expected["undecodable"].as_u64(),
                    "{what}"
                );
                assert_same_json(
                    &optional_effect_json(claimed.effect.as_ref())?,
                    &expected["effect"],
                    &what,
                );
            }
            "nextClaimableAt" => assert_eq!(
                outbox::next_claimable_at(tx, op["maxRunningTitleGenerations"].as_i64())?
                    .as_deref(),
                expected.as_str(),
                "{what}"
            ),
            "succeed" | "fail" | "retry" => {
                let (id, worker) = &claims[op["claim"].as_u64().expect("claim") as usize];
                let error = op["error"].as_str().unwrap_or_default();
                let settled = match op["op"].as_str() {
                    Some("succeed") => outbox::succeed(tx, id, worker, &at)?,
                    Some("fail") => outbox::fail(tx, id, worker, error, &at)?,
                    _ => outbox::retry(
                        tx,
                        id,
                        worker,
                        error,
                        &at,
                        &respell(op["availableAt"].as_str().expect("availableAt")),
                    )?,
                };
                assert_eq!(Some(settled), expected.as_bool(), "{what}");
            }
            "cancel" => {
                let types: Vec<&str> = op["effectTypes"]
                    .as_array()
                    .expect("types")
                    .iter()
                    .filter_map(Value::as_str)
                    .collect();
                let cancelled = outbox::cancel_unsettled(
                    tx,
                    &ThreadId(text(&op["threadId"])),
                    &types,
                    op["reason"].as_str().expect("reason"),
                    &at,
                )?;
                assert_eq!(json!(cancelled), *expected, "{what}");
            }
            "get" => {
                let found = op["effectIds"]
                    .as_array()
                    .expect("ids")
                    .iter()
                    .map(|id| {
                        optional_effect_json(outbox::get(tx, id.as_str().expect("id"))?.as_ref())
                    })
                    .collect::<Result<Vec<_>>>()?;
                assert_same_json(&Value::Array(found), expected, &what);
            }
            "listByCommand" => {
                let found = outbox::list_by_command(tx, &CommandId(text(&op["commandId"])))?
                    .iter()
                    .map(effect_json)
                    .collect::<Result<Vec<_>>>()?;
                assert_same_json(&Value::Array(found), expected, &what);
            }
            "prune" => assert_eq!(
                Some(outbox::prune_settled_batch(
                    tx,
                    &respell(op["cutoff"].as_str().expect("cutoff"))
                )? as u64),
                expected.as_u64(),
                "{what}"
            ),
            "reconcile" => {
                let reconciled = outbox::reconcile_after_process_loss(tx, &at)?;
                assert_eq!(
                    json!({"requeued": reconciled.requeued, "cancelled": reconciled.cancelled}),
                    *expected,
                    "{what}"
                );
            }
            other => panic!("unknown outbox op {other}"),
        }
    }
    Ok(())
}

#[tokio::test]
async fn outbox_operations_match_the_typescript_outbox_row_for_row() -> Result<()> {
    let f = TestDatabase::memory()?;
    f.stores
        .transaction(|tx| {
            let outbox = &store_oracle()["outbox"];
            replay_outbox(tx, &outbox["ops"], &outbox["results"])
        })
        .await
}

#[tokio::test]
async fn process_loss_reconciliation_matches_the_typescript_outbox() -> Result<()> {
    let f = TestDatabase::memory()?;
    f.stores
        .transaction(|tx| {
            let loss = &store_oracle()["processLoss"];
            replay_outbox(tx, &loss["ops"], &loss["results"])
        })
        .await
}

#[test]
fn process_loss_classification_is_the_typescript_classification() -> Result<()> {
    let loss = &store_oracle()["processLoss"];
    let list = |key: &str| -> Vec<&str> {
        loss[key]
            .as_array()
            .expect("types")
            .iter()
            .filter_map(Value::as_str)
            .collect()
    };
    let (bound, safe) = (list("processBound"), list("replaySafe"));
    assert_eq!(outbox::PROCESS_LOSS.len(), bound.len() + safe.len());
    for (effect_type, class) in outbox::PROCESS_LOSS {
        let expected = if bound.contains(&effect_type) {
            outbox::AfterProcessLoss::Cancel
        } else {
            assert!(
                safe.contains(&effect_type),
                "{effect_type} is in neither TypeScript list"
            );
            outbox::AfterProcessLoss::Requeue
        };
        assert_eq!(class, expected, "{effect_type}");
    }
    let mut seen = std::collections::BTreeSet::new();
    for op in loss["ops"].as_array().expect("ops") {
        for effect in op["effects"].as_array().into_iter().flatten() {
            let request: EffectRequest = serde_json::from_value(effect["request"].clone())?;
            let class = outbox::after_process_loss(&request);
            let listed = outbox::PROCESS_LOSS
                .iter()
                .find(|(effect_type, _)| *effect_type == request.effect_type())
                .map(|(_, class)| *class);
            assert_eq!(Some(class), listed, "{}", request.effect_type());
            assert_eq!(
                request.process_bound(),
                class == outbox::AfterProcessLoss::Cancel
            );
            seen.insert(request.effect_type());
        }
    }
    assert_eq!(seen.len(), 16, "every effect variant is exercised");
    Ok(())
}

fn receipt_from(value: &Value) -> Result<CommandReceipt> {
    let text = |key: &str| value[key].as_str().map(str::to_owned);
    Ok(CommandReceipt {
        command_id: CommandId(text("commandId").expect("commandId")),
        aggregate: match text("threadId") {
            Some(thread) => AggregateId::Thread(ThreadId(thread)),
            None => AggregateId::Project(ProjectId(text("projectId").expect("projectId"))),
        },
        command_type: text("commandType").expect("commandType"),
        accepted_at: respell(&text("acceptedAt").expect("acceptedAt")),
        result_sequence: value["resultSequence"].as_i64().expect("resultSequence"),
        status: serde_json::from_value(value["status"].clone())?,
        error: text("error"),
    })
}

fn receipt_json(receipt: Option<CommandReceipt>) -> Value {
    receipt.map_or(Value::Null, |receipt| {
        let mut value = json!({
            "commandId": receipt.command_id,
            "commandType": receipt.command_type,
            "acceptedAt": receipt.accepted_at,
            "resultSequence": receipt.result_sequence,
            "status": receipt.status,
            "error": receipt.error,
        });
        let (key, id) = match &receipt.aggregate {
            AggregateId::Thread(id) => ("threadId", id.0.clone()),
            AggregateId::Project(id) => ("projectId", id.0.clone()),
        };
        value[key] = id.into();
        value
    })
}

#[tokio::test]
async fn receipt_operations_match_the_typescript_receipt_store() -> Result<()> {
    let f = TestDatabase::memory()?;
    f.stores
        .transaction(|tx| {
            let oracle = &store_oracle()["receipts"];
            for (index, (op, expected)) in oracle["ops"]
                .as_array()
                .expect("ops")
                .iter()
                .zip(oracle["results"].as_array().expect("results"))
                .enumerate()
            {
                let what = format!("receipt op {index} ({})", op["op"]);
                let command = CommandId(op["commandId"].as_str().unwrap_or_default().to_owned());
                match op["op"].as_str().expect("op") {
                    "insertIfAbsent" => assert_eq!(
                        Some(receipts::insert_if_absent(
                            tx,
                            &receipt_from(&op["receipt"])?
                        )?),
                        expected.as_bool(),
                        "{what}"
                    ),
                    "upsert" => receipts::upsert(tx, &receipt_from(&op["receipt"])?)?,
                    "get" => assert_same_json(
                        &receipt_json(receipts::get_thread(tx, &command)?),
                        expected,
                        &what,
                    ),
                    "getProject" => assert_same_json(
                        &receipt_json(receipts::get_project(tx, &command)?),
                        expected,
                        &what,
                    ),
                    "snapshot" => assert_rows(
                        &snapshot(tx, "orchestration_command_receipts")?,
                        expected,
                        &what,
                    ),
                    other => panic!("unknown receipt op {other}"),
                }
            }
            Ok(())
        })
        .await
}

/// TypeScript records keep insertion order; `t3_contracts::provider_policy::ProviderUserInputAnswers`
/// is a `BTreeMap`, so the stored payload (and any event payload holding such a record) has sorted keys.
#[tokio::test]
#[ignore = "needs t3-contracts to make ProviderUserInputAnswers an insertion-ordered map"]
async fn user_input_answers_keep_their_insertion_order() -> Result<()> {
    let f = TestDatabase::memory()?;
    f.stores
        .transaction(|tx| {
            let request: EffectRequest = serde_json::from_value(json!({
                "type": "runtime-request.respond", "providerSessionId": "session", "requestId": "request",
                "answers": {"question": "yes", "count": 2}
            }))?;
            outbox::enqueue(tx, &[pending("respond", "thread", request)], TEST_TIME)?;
            let payload: String = tx.query_row(
                "SELECT payload_json FROM orchestration_v2_effect_outbox",
                [],
                |row| row.get(0),
            )?;
            assert!(payload.ends_with(r#""answers":{"question":"yes","count":2}}"#), "{payload}");
            Ok(())
        })
        .await
}
