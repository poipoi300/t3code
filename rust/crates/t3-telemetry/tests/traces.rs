use serde_json::json;
use std::time::Duration;
use t3_telemetry::{
    datetime,
    diagnostics::{TraceAggregator, TraceDiagnostics, rotated_paths},
    logging::{TraceOptions, TraceSink},
};

fn record(name: &str, start: u64, duration: u64, exit: &str) -> serde_json::Value {
    json!({"type":"effect-span","name":name,"traceId":"trace","spanId":"span","startTimeUnixNano":(start*1_000_000).to_string(),"endTimeUnixNano":((start+duration)*1_000_000).to_string(),"durationMs":duration,"exit":{"_tag":exit,"cause":"Provider crashed"},"attributes":{},"events":[]})
}
#[test]
fn aggregates_browser_and_effect_failures_interruptions_logs_and_malformed_records() {
    let mut a = TraceAggregator::new(1000);
    a.add_line(&record("server.getConfig", 1000, 50, "Success").to_string());
    a.add_line("not-json");
    a.add_line("[]");
    let mut failure = record("orchestration.dispatch", 2000, 1500, "Failure");
    failure["events"] = json!([{"name":"provider failed","timeUnixNano":"3400000000","attributes":{"effect.logLevel":"Error"}}]);
    a.add_line(&failure.to_string());
    a.add_line(&record("orchestration.dispatch", 4000, 250, "Failure").to_string());
    a.add_line(&record("git.status", 5000, 25, "Interrupted").to_string());
    let mut browser = record("browser.request", 6000, 10, "Success");
    browser.as_object_mut().expect("object").remove("exit");
    browser["status"] = json!({"code":"2","message":"Browser request failed"});
    a.add_line(&browser.to_string());
    let result = a
        .finish(std::path::Path::new("test.ndjson"), &[], 10000, None, false)
        .expect("finish");
    assert_eq!(result.record_count, 5);
    assert_eq!(result.parse_error_count, 2);
    assert_eq!(result.failure_count, 3);
    assert_eq!(result.interruption_count, 1);
    assert_eq!(result.slow_span_count, 1);
    assert_eq!(result.common_failures[0].count, 2);
    assert_eq!(result.top_spans_by_count[0].count, 2);
    assert_eq!(
        result.latest_warning_and_error_logs[0].message.as_str(),
        "provider failed"
    );
    assert_eq!(
        result.first_span_at,
        t3_contracts::resource_telemetry::WireOption::Some(datetime(1000))
    );
}

#[tokio::test]
async fn missing_partial_files_and_rotations_return_contract_failures() {
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("server.trace.ndjson");
    let service = TraceDiagnostics::new(path.clone(), 2);
    let missing = service.read_at(1).await.expect("missing");
    assert_eq!(missing.record_count, 0);
    assert!(matches!(
        missing.error,
        t3_contracts::resource_telemetry::WireOption::Some(_)
    ));
    std::fs::write(&path, record("new", 1, 2, "Success").to_string()).expect("write");
    std::fs::create_dir(rotated_paths(&path, 2)[0].clone()).expect("directory read failure");
    std::fs::write(
        rotated_paths(&path, 2)[1].clone(),
        record("old", 0, 1, "Failure").to_string(),
    )
    .expect("backup");
    let partial = service.read_at(2).await.expect("partial");
    assert_eq!(partial.record_count, 2);
    assert_eq!(
        partial.partial_failure,
        t3_contracts::resource_telemetry::WireOption::Some(true)
    );
}

#[test]
fn sink_batches_bounds_rotates_and_redacts_before_disk() {
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("server.trace.ndjson");
    std::fs::write(format!("{}.4", path.display()), "old").expect("old");
    let sink = TraceSink::new(TraceOptions {
        file_path: path.clone(),
        max_bytes: 600,
        max_files: 2,
        batch_window: Duration::from_secs(1),
    })
    .expect("sink");
    assert!(!std::path::Path::new(&format!("{}.4", path.display())).exists());
    for _ in 0..9 {
        sink.push(record("test", 0, 10, "Success"));
        sink.flush_sync().expect("flush");
    }
    assert!(path.exists());
    assert!(std::path::Path::new(&format!("{}.1", path.display())).exists());
    assert!(std::path::Path::new(&format!("{}.2", path.display())).exists());
    assert!(!std::path::Path::new(&format!("{}.3", path.display())).exists());
    sink.push(json!({"prompt":"PRIVATE","url":"wss://x/ws?wsTicket=TICKET&dpop=PROOF","authorization":"Bearer SECRET"}));
    sink.flush_sync().expect("flush");
    let text = std::fs::read_to_string(&path).expect("read");
    for secret in ["PRIVATE", "TICKET", "PROOF", "SECRET"] {
        assert!(!text.contains(secret));
    }
}

#[test]
fn cli_credentials_and_prompt_arguments_are_redacted() {
    let redacted = t3_telemetry::privacy::redact_text(
        "provider --api-key TOP_SECRET --prompt \"PROMPT_CONTENT with spaces\" --model safe",
    );
    assert!(!redacted.contains("TOP_SECRET"));
    assert!(!redacted.contains("PROMPT_CONTENT"));
    assert!(redacted.contains("--model safe"));
    let url = t3_telemetry::privacy::redact_text(
        "failed request to https://operator:PRIVATE_PASSWORD@collector.example/v1/traces",
    );
    assert!(!url.contains("PRIVATE_PASSWORD"));
    assert!(!url.contains("operator:"));
    assert!(url.contains("collector.example/v1/traces"));
    let command = t3_telemetry::privacy::redact_arguments([
        "provider",
        "--prompt",
        "a multi word PRIVATE_PROMPT\nwith a second line",
        "--api-key=PRIVATE_KEY with spaces",
        "--model",
        "safe",
    ]);
    assert!(!command.contains("PRIVATE"));
    assert!(!command.contains("second line"));
    assert!(!command.contains("multi word"));
    assert!(command.contains("--model safe"));
    for (text, secret) in [
        (
            "open https://host.test/pair#token=PAIRING_TOKEN",
            "PAIRING_TOKEN",
        ),
        (
            "https://host.test/x#a=1&access_token=FRAGMENT_TOKEN",
            "FRAGMENT_TOKEN",
        ),
        ("https://host.test/x;token=MATRIX_TOKEN", "MATRIX_TOKEN"),
        (
            "https://host.test/callback?code=OAUTH_CODE&state=s",
            "OAUTH_CODE",
        ),
        (
            "https://host.test/callback?state=OAUTH_STATE&code=c",
            "OAUTH_STATE",
        ),
        (
            "https://host.test/callback#code=FRAGMENT_CODE",
            "FRAGMENT_CODE",
        ),
    ] {
        let redacted = t3_telemetry::privacy::redact_text(text);
        assert!(!redacted.contains(secret), "{redacted}");
        assert!(redacted.contains("host.test"), "{redacted}");
    }
    assert_eq!(
        t3_telemetry::privacy::redact_text("process exited with code=1 state=idle"),
        "process exited with code=1 state=idle"
    );
}

fn local_logging(dir: &std::path::Path) -> t3_telemetry::logging::LoggingOptions {
    t3_telemetry::logging::LoggingOptions {
        trace: TraceOptions {
            file_path: dir.join("server.trace.ndjson"),
            max_bytes: 100_000,
            max_files: 2,
            batch_window: Duration::from_secs(60),
        },
        log_level: "Info".into(),
        trace_min_level: "Info".into(),
        trace_timing_enabled: true,
        telemetry: t3_config::config::TelemetryConfig {
            disabled: false,
            warnings: vec![],
            resource_attributes: Default::default(),
            traces: None,
            metrics: None,
            logs: None,
            default_export: t3_config::config::SignalExport {
                protocol: "http/json".into(),
                headers: None,
                export_interval_ms: 10_000,
            },
        },
        version: "test".into(),
        release_channel: "dev".into(),
        host_name: "test-host".into(),
        mode: t3_config::config::RuntimeMode::Web,
        console: false,
    }
}

#[tokio::test]
async fn local_spans_are_batched_off_the_ending_thread_and_keep_usage_counters() {
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("server.trace.ndjson");
    let (dispatch, handle) = t3_telemetry::logging::build(local_logging(dir.path()))
        .await
        .expect("logging");
    tracing::dispatcher::with_default(&dispatch, || {
        let _span = tracing::info_span!(
            "provider.turn",
            inputTokens = 12_i64,
            total_tokens = 30_i64,
            tokenCount = "PRIVATE_TOKEN",
            outputTokens = "NOT_A_NUMBER"
        )
        .entered();
    });
    // Ending the span only queues it in the processor; the sink receives it on export.
    assert_eq!(handle.sink.buffered(), 0);
    handle.force_flush().await.expect("flush");
    let trace = std::fs::read_to_string(&path).expect("trace");
    let span: serde_json::Value = trace
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .find(|record: &serde_json::Value| record["name"] == "provider.turn")
        .expect("span exported");
    assert_eq!(span["attributes"]["inputTokens"], 12);
    assert_eq!(span["attributes"]["total_tokens"], 30);
    assert_eq!(span["attributes"]["tokenCount"], "<redacted>");
    assert_eq!(span["attributes"]["outputTokens"], "<redacted>");
    let sink = TraceSink::new(TraceOptions {
        file_path: dir.path().join("records.ndjson"),
        max_bytes: 100_000,
        max_files: 1,
        batch_window: Duration::from_secs(1),
    })
    .expect("sink");
    sink.push(json!({"usage":{"inputTokens":7,"cachedInputTokens":"PRIVATE","secretToken":3}}));
    sink.flush_sync().expect("flush");
    let record: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(dir.path().join("records.ndjson"))
            .expect("records")
            .trim(),
    )
    .expect("record");
    assert_eq!(
        record["usage"],
        json!({"inputTokens":7,"cachedInputTokens":"<redacted>","secretToken":"<redacted>"})
    );
    handle.shutdown().await.expect("shutdown");
}

#[test]
fn prefixed_keys_the_last_user_info_at_and_other_auth_schemes_are_redacted() {
    for (text, secret) in [
        ("https://host.test/token?client_secret=abc", "abc"),
        ("https://host.test/token?private_token=abc", "abc"),
        ("https://host.test/token?auth_token=abc", "abc"),
        ("https://user:p@ss@collector.example/", "ss@"),
        ("https://user:p@ss@collector.example/", "p@"),
        ("Authorization: Basic dXNlcjpwYXNz", "dXNlcjpwYXNz"),
        ("x-api-key: sk-ant-abc", "sk-ant-abc"),
    ] {
        let redacted = t3_telemetry::privacy::redact_text(text);
        assert!(!redacted.contains(secret), "{text} -> {redacted}");
    }
    assert!(
        t3_telemetry::privacy::redact_text("https://user:p@ss@collector.example/")
            .contains("collector.example/")
    );
    assert_eq!(
        t3_telemetry::privacy::redact_text("https://host.test/a?page=2&sort=name"),
        "https://host.test/a?page=2&sort=name"
    );
}

fn quiet_spans(dispatch: &tracing::Dispatch, count: usize) {
    tracing::dispatcher::with_default(dispatch, || {
        for index in 0..count {
            let _span = tracing::info_span!("burst.span", index).entered();
        }
    });
}

#[tokio::test]
async fn a_burst_of_local_spans_within_one_window_is_kept() {
    let dir = tempfile::tempdir().expect("temp");
    let mut options = local_logging(dir.path());
    // Large enough that the sink's own byte bound is not what drops records.
    options.trace.max_bytes = 16 * 1024 * 1024;
    let (dispatch, handle) = t3_telemetry::logging::build(options)
        .await
        .expect("logging");
    quiet_spans(&dispatch, 20_000);
    handle.force_flush().await.expect("flush");
    let trace = std::fs::read_to_string(dir.path().join("server.trace.ndjson")).expect("trace");
    assert_eq!(trace.matches("\"burst.span\"").count(), 20_000);
    assert_eq!(handle.sink.dropped(), 0);
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn dropping_the_handle_without_shutdown_keeps_the_last_window() {
    let dir = tempfile::tempdir().expect("temp");
    let (dispatch, handle) = t3_telemetry::logging::build(local_logging(dir.path()))
        .await
        .expect("logging");
    quiet_spans(&dispatch, 3);
    drop(handle);
    let trace = std::fs::read_to_string(dir.path().join("server.trace.ndjson")).unwrap_or_default();
    assert_eq!(trace.matches("\"burst.span\"").count(), 3, "{trace}");
}

/// What `server.getTraceDiagnostics` does while the server runs: flush the sink, then read the
/// file. Spans still queued in the local processor must be on disk by then, and after shutdown.
#[tokio::test]
async fn a_sink_flush_and_shutdown_write_spans_still_queued() {
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("server.trace.ndjson");
    let (dispatch, handle) = t3_telemetry::logging::build(local_logging(dir.path()))
        .await
        .expect("logging");
    quiet_spans(&dispatch, 2);
    handle.sink.flush().await.expect("sink flush");
    let live = std::fs::read_to_string(&path).expect("trace file after a sink flush");
    assert_eq!(live.matches("\"burst.span\"").count(), 2);
    quiet_spans(&dispatch, 1);
    handle.shutdown().await.expect("shutdown");
    let after = std::fs::read_to_string(&path).expect("trace file after shutdown");
    assert_eq!(after.matches("\"burst.span\"").count(), 3);
}
