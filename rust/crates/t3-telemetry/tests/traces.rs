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
}
