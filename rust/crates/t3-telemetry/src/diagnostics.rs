use crate::{Result, datetime, privacy};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use t3_contracts::server::ServerTraceDiagnosticsResult;
use tokio::io::AsyncBufReadExt;

pub struct TraceDiagnostics {
    pub trace_file_path: PathBuf,
    pub max_files: usize,
    pub slow_span_threshold_ms: u64,
}

pub fn rotated_paths(path: &Path, max_files: usize) -> Vec<PathBuf> {
    (1..=max_files)
        .rev()
        .map(|i| PathBuf::from(format!("{}.{i}", path.display())))
        .chain(std::iter::once(path.to_path_buf()))
        .collect()
}

impl TraceDiagnostics {
    pub fn new(trace_file_path: PathBuf, max_files: usize) -> Self {
        Self {
            trace_file_path,
            max_files,
            slow_span_threshold_ms: 1000,
        }
    }
    pub async fn read(&self) -> Result<ServerTraceDiagnosticsResult> {
        self.read_at(crate::unix_millis()).await
    }
    pub async fn read_at(&self, now_ms: u64) -> Result<ServerTraceDiagnosticsResult> {
        let paths = rotated_paths(&self.trace_file_path, self.max_files);
        let mut aggregator = TraceAggregator::new(self.slow_span_threshold_ms);
        let mut found = false;
        let mut failure = None;
        for path in &paths {
            let file = match tokio::fs::File::open(path).await {
                Ok(file) => file,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    failure = Some(
                        json!({"kind":"trace-file-read-failed","message":format!("Failed to read local trace file '{}'.",path.display())}),
                    );
                    continue;
                }
            };
            let mut reader = tokio::io::BufReader::new(file);
            let mut line = Vec::new();
            let mut success = true;
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line).await {
                    Ok(0) => break,
                    Ok(_) => aggregator.add_line(&String::from_utf8_lossy(&line)),
                    Err(_) => {
                        success = false;
                        failure = Some(
                            json!({"kind":"trace-file-read-failed","message":format!("Failed to read local trace file '{}'.",path.display())}),
                        );
                        break;
                    }
                }
            }
            found |= success;
        }
        if !found {
            aggregator = TraceAggregator::new(self.slow_span_threshold_ms);
            if failure.is_none() {
                failure = Some(
                    json!({"kind":"trace-file-not-found","message":"No local trace files were found."}),
                );
            }
        }
        aggregator.finish(
            &self.trace_file_path,
            &paths,
            now_ms,
            failure.clone(),
            found && failure.is_some(),
        )
    }
}

#[derive(Default)]
struct SpanSummary {
    count: u64,
    failures: u64,
    total: f64,
    max: f64,
}
pub struct TraceAggregator {
    threshold: u64,
    records: u64,
    parse_errors: u64,
    failures: u64,
    interruptions: u64,
    slow: u64,
    first: Option<u64>,
    last: Option<u64>,
    spans: BTreeMap<String, SpanSummary>,
    common: BTreeMap<(String, String), Value>,
    latest: Vec<Value>,
    slowest: Vec<Value>,
    logs: Vec<Value>,
    levels: BTreeMap<String, u64>,
}
fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str().filter(|s| !s.trim().is_empty())
}
fn nanos(value: &Value, key: &str) -> Option<u64> {
    text(value, key)?
        .parse::<u128>()
        .ok()
        .and_then(|n| u64::try_from(n / 1_000_000).ok())
}
fn some(value: Value) -> Value {
    json!({"_tag":"Some","value":value})
}
fn option(value: Option<Value>) -> Value {
    value.map(some).unwrap_or_else(|| json!({"_tag":"None"}))
}
fn bounded(items: &mut Vec<Value>, item: Value, limit: usize, key: &str) {
    items.push(item);
    items.sort_by(|a, b| {
        match (
            a.get(key).and_then(Value::as_f64),
            b.get(key).and_then(Value::as_f64),
        ) {
            (Some(a), Some(b)) => b.total_cmp(&a),
            _ => b[key].as_str().cmp(&a[key].as_str()),
        }
    });
    items.truncate(limit);
}
impl TraceAggregator {
    pub fn new(threshold: u64) -> Self {
        Self {
            threshold,
            records: 0,
            parse_errors: 0,
            failures: 0,
            interruptions: 0,
            slow: 0,
            first: None,
            last: None,
            spans: BTreeMap::new(),
            common: BTreeMap::new(),
            latest: vec![],
            slowest: vec![],
            logs: vec![],
            levels: BTreeMap::new(),
        }
    }
    pub fn add_line(&mut self, line: &str) {
        if line.trim().is_empty() {
            return;
        }
        let Ok(mut v) = serde_json::from_str::<Value>(line) else {
            self.parse_errors += 1;
            return;
        };
        privacy::sanitize(&mut v);
        let (Some(name), Some(trace), Some(span), Some(duration), Some(end)) = (
            text(&v, "name"),
            text(&v, "traceId"),
            text(&v, "spanId"),
            v.get("durationMs").and_then(Value::as_f64),
            nanos(&v, "endTimeUnixNano"),
        ) else {
            self.parse_errors += 1;
            return;
        };
        self.records += 1;
        if let Some(start) = nanos(&v, "startTimeUnixNano") {
            self.first = Some(self.first.map_or(start, |s| s.min(start)));
        }
        self.last = Some(self.last.map_or(end, |s| s.max(end)));
        let failure = v["exit"]["_tag"] == "Failure" || v["status"]["code"] == "2";
        let interrupted = v["exit"]["_tag"] == "Interrupted"
            || v["attributes"]["effect.fiber.interrupted"] == true
            || v["status"]["message"] == "Interrupted";
        self.failures += u64::from(failure);
        self.interruptions += u64::from(interrupted);
        self.slow += u64::from(duration >= self.threshold as f64);
        let summary = self.spans.entry(name.to_owned()).or_default();
        summary.count += 1;
        summary.failures += u64::from(failure);
        summary.total += duration;
        summary.max = summary.max.max(duration);
        let occurrence = json!({"name":name,"durationMs":duration,"endedAt":datetime(end),"traceId":trace,"spanId":span});
        bounded(&mut self.slowest, occurrence.clone(), 10, "durationMs");
        if failure {
            let cause = if v["exit"]["_tag"] == "Failure" {
                text(&v["exit"], "cause")
            } else {
                text(&v["status"], "message")
            }
            .unwrap_or("Failure")
            .trim();
            let mut recent = occurrence;
            recent["cause"] = json!(cause);
            bounded(&mut self.latest, recent, 20, "endedAt");
            let key = (name.to_owned(), cause.to_owned());
            let existing = self.common.get(&key);
            let count = existing.and_then(|e| e["count"].as_u64()).unwrap_or(0) + 1;
            let newer =
                existing.is_none_or(|e| e["lastSeenAt"].as_str() < Some(datetime(end).0.as_str()));
            let common = if newer {
                json!({"name":name,"cause":cause,"count":count,"lastSeenAt":datetime(end),"traceId":trace,"spanId":span})
            } else {
                let mut existing = existing.cloned().unwrap_or(Value::Null);
                existing["count"] = json!(count);
                existing
            };
            self.common.insert(key, common);
        }
        for event in v["events"].as_array().into_iter().flatten() {
            let Some(level) = text(&event["attributes"], "effect.logLevel") else {
                continue;
            };
            *self.levels.entry(level.to_owned()).or_default() += 1;
            if !["warning", "warn", "error", "fatal"].contains(&level.to_lowercase().as_str()) {
                continue;
            }
            bounded(
                &mut self.logs,
                json!({"spanName":name,"level":level,"message":text(event,"name").unwrap_or("Log event").trim(),"seenAt":datetime(nanos(event,"timeUnixNano").unwrap_or(end)),"traceId":trace,"spanId":span}),
                20,
                "seenAt",
            );
        }
    }
    pub fn finish(
        self,
        path: &Path,
        paths: &[PathBuf],
        now_ms: u64,
        error: Option<Value>,
        partial: bool,
    ) -> Result<ServerTraceDiagnosticsResult> {
        let mut top: Vec<_> = self.spans.into_iter().collect();
        top.sort_by(|(an, a), (bn, b)| {
            b.count
                .cmp(&a.count)
                .then(b.max.total_cmp(&a.max))
                .then(an.cmp(bn))
        });
        top.truncate(10);
        let mut common: Vec<_> = self.common.into_values().collect();
        common.sort_by(|a, b| {
            b["count"]
                .as_u64()
                .cmp(&a["count"].as_u64())
                .then(b["lastSeenAt"].as_str().cmp(&a["lastSeenAt"].as_str()))
        });
        common.truncate(10);
        Ok(serde_json::from_value(
            json!({"traceFilePath":path.to_string_lossy(),"scannedFilePaths":paths.iter().map(|p|p.to_string_lossy()).collect::<Vec<_>>(),"readAt":datetime(now_ms),
            "recordCount":self.records,"parseErrorCount":self.parse_errors,"firstSpanAt":option(self.first.map(|s|json!(datetime(s)))),"lastSpanAt":option(self.last.map(|s|json!(datetime(s)))),
            "failureCount":self.failures,"interruptionCount":self.interruptions,"slowSpanThresholdMs":self.threshold,"slowSpanCount":self.slow,"logLevelCounts":self.levels,
            "topSpansByCount":top.into_iter().map(|(name,s)|json!({"name":name,"count":s.count,"failureCount":s.failures,"totalDurationMs":s.total,"averageDurationMs":s.total/s.count as f64,"maxDurationMs":s.max})).collect::<Vec<_>>(),
            "slowestSpans":self.slowest,"commonFailures":common,"latestFailures":self.latest,"latestWarningAndErrorLogs":self.logs,
            "partialFailure":option(partial.then_some(json!(true))),"error":option(error)}),
        )?)
    }
}
