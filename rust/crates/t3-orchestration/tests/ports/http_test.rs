use super::bounded_snapshot_transport_test::{seed, service};
use t3_contracts::{
    auth::AuthEnvironmentScope, base_schemas::ThreadId,
    environment_http::EnvironmentRequestInvalidReason,
};
use t3_orchestration::{ports::transport::http::*, testing::TestDatabase};

#[tokio::test]
async fn transcript_authorizes_and_maps_missing_and_oversized_history() {
    use std::sync::Arc;
    use t3_orchestration::{
        ports::thread_project::thread_management_service::{
            NoLegacyTranscript, ThreadManagementService, ThreadManagementServices,
        },
        runtime::worker::OutboxSignals,
        testing::{FakeCommandDispatcher, FakeSettingsService, ManualClock, TEST_TIME},
    };
    let fixture = TestDatabase::memory().unwrap();
    let (service, sink) = service(&fixture);
    seed(&fixture, &sink).await.unwrap();
    let item = super::thread_stream_test::command("export-command", 1);
    super::bounded_snapshot_transport_test::commit(&sink,vec![serde_json::from_value(serde_json::json!({"id":"export-row","threadId":"thread","type":"turn-item.updated","occurredAt":TEST_TIME,"payload":item})).unwrap()]).await.unwrap();
    let management = ThreadManagementService::new(ThreadManagementServices {
        reader: service.projections.clone(),
        committer: service.events.clone(),
        database: fixture.stores.clone(),
        dispatcher: Arc::new(FakeCommandDispatcher::default()),
        settings: Arc::new(FakeSettingsService::default()),
        hydrator: Arc::new(NoLegacyTranscript),
        signals: OutboxSignals::default(),
        clock: Arc::new(ManualClock::new(TEST_TIME)),
    });
    let http = OrchestrationHttpRead { service: &service };
    let environment = t3_contracts::base_schemas::EnvironmentId("env".into());
    let thread = ThreadId("thread".into());
    assert_eq!(
        http.thread_transcript(
            &[],
            &management,
            thread.clone(),
            environment.clone(),
            100000
        )
        .await
        .unwrap_err()
        .status(),
        403
    );
    let scopes = [AuthEnvironmentScope::OrchestrationRead];
    assert_eq!(
        http.thread_transcript(
            &scopes,
            &management,
            ThreadId("missing".into()),
            environment.clone(),
            100000
        )
        .await
        .unwrap_err()
        .status(),
        404
    );
    let error = http
        .thread_transcript(&scopes, &management, thread.clone(), environment.clone(), 1)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        HttpReadError::Invalid(EnvironmentRequestInvalidReason::ThreadTranscriptTooLarge)
    ));
    let transcript = http
        .thread_transcript(&scopes, &management, thread, environment, 100000)
        .await
        .unwrap();
    assert_eq!(transcript.title, "Test");
    assert_eq!(transcript.thread_id.0, "thread");
    assert_eq!(transcript.items.len(), 1);
    assert_eq!(
        serde_json::to_value(&transcript.items[0]).unwrap()["item"]["output"],
        "private output"
    );
    management.shutdown().await;
}
#[test]
fn resume_query_matches_javascript_safe_integer_validation() {
    for (raw, expected) in [
        (None, None),
        (Some(""), Some(0)),
        (Some(" 12 "), Some(12)),
        (Some("1e3"), Some(1000)),
        (Some("0x10"), Some(16)),
        (Some("0b11"), Some(3)),
        (Some("0o10"), Some(8)),
        (Some("-0"), Some(0)),
        (Some("-1"), None),
        (Some("1.2"), None),
        (Some("Infinity"), None),
        (Some("NaN"), None),
        (Some("9007199254740992"), None),
        (Some("not-a-cursor"), None),
    ] {
        assert_eq!(parse_resume_from(raw), expected, "{raw:?}");
    }
}
#[tokio::test]
async fn http_reads_authorize_before_access_and_distinguish_invalid_cursor_from_missing_thread() {
    let fixture = TestDatabase::memory().unwrap();
    let (service, _sink) = service(&fixture);
    let http = OrchestrationHttpRead { service: &service };
    assert_eq!(
        http.thread_snapshot(&[], ThreadId("missing".into()))
            .await
            .unwrap_err()
            .status(),
        403
    );
    assert_eq!(
        http.thread_snapshot(
            &[AuthEnvironmentScope::OrchestrationOperate],
            ThreadId("missing".into())
        )
        .await
        .unwrap_err()
        .status(),
        403
    );
    assert_eq!(
        http.thread_snapshot(
            &[AuthEnvironmentScope::OrchestrationRead],
            ThreadId("missing".into())
        )
        .await
        .unwrap_err()
        .status(),
        404
    );
    let error = http
        .history_page(
            &[AuthEnvironmentScope::OrchestrationRead],
            ThreadId("missing".into()),
            HistoryPageQuery {
                cursor: "invalid",
                through_entry_id: None,
                view: None,
                defer_work: None,
                compact_ids: None,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        HttpReadError::Invalid(EnvironmentRequestInvalidReason::InvalidHistoryCursor)
    ));
    assert_eq!(error.status(), 400);
}
#[tokio::test]
async fn a_resumable_http_shell_does_not_read_a_snapshot_and_full_thread_reads_share_the_cursor() {
    let fixture = TestDatabase::memory().unwrap();
    let (service, sink) = service(&fixture);
    let sequence = seed(&fixture, &sink).await.unwrap();
    let http = OrchestrationHttpRead { service: &service };
    let cursor = sequence.to_string();
    let answer = http
        .shell_snapshot(
            &[AuthEnvironmentScope::OrchestrationRead],
            ShellSnapshotQuery {
                resume_from: Some(&cursor),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(matches!(answer, ShellSnapshotAnswer::Resumable(_)));
    let snapshot = http
        .thread_snapshot(
            &[AuthEnvironmentScope::OrchestrationRead],
            ThreadId("thread".into()),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.snapshot_sequence, sequence);
    assert!(snapshot.history_cursor.is_none());
}
