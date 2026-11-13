use crate::GitError;
use notify::Watcher;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitProgress {
    Output {
        stream: &'static str,
        text: String,
    },
    HookStarted {
        hook_name: String,
    },
    HookFinished {
        hook_name: String,
        exit_code: Option<i32>,
        duration_ms: Option<u64>,
    },
}
pub type CommitReporter = Arc<dyn Fn(CommitProgress) + Send + Sync>;
struct TraceReader {
    path: PathBuf,
    processed: usize,
    hooks: BTreeMap<String, (String, Instant)>,
    reporter: CommitReporter,
}
impl TraceReader {
    fn flush(&mut self, final_line: bool) {
        let Ok(contents) = std::fs::read(&self.path) else {
            return;
        };
        if contents.len() <= self.processed {
            return;
        }
        let delta = &contents[self.processed..];
        let complete = if final_line {
            delta.len()
        } else {
            delta
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |index| index + 1)
        };
        for line in delta[..complete].split(|byte| *byte == b'\n') {
            let Ok(record) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            let child = record["child_id"]
                .as_i64()
                .map(|id| id.to_string())
                .or_else(|| record["child_id"].as_str().map(str::to_owned))
                .or_else(|| record["hook_name"].as_str().map(str::to_owned));
            let Some(child) = child else {
                continue;
            };
            let key = format!("{}\n{child}", record["sid"].as_str().unwrap_or(""));
            if record["event"] == "child_start" && record["child_class"] == "hook" {
                let hook = record["hook_name"].as_str().unwrap_or("").trim();
                if hook.is_empty() {
                    continue;
                }
                self.hooks.insert(key, (hook.into(), Instant::now()));
                (self.reporter)(CommitProgress::HookStarted {
                    hook_name: hook.into(),
                });
            } else if record["event"] == "child_exit"
                && let Some((hook_name, started)) = self.hooks.remove(&key)
            {
                (self.reporter)(CommitProgress::HookFinished {
                    hook_name,
                    exit_code: record["code"]
                        .as_i64()
                        .and_then(|code| i32::try_from(code).ok()),
                    duration_ms: Some(started.elapsed().as_millis().min(u64::MAX as u128) as u64),
                });
            }
        }
        self.processed += complete;
    }
}
pub(crate) struct CommitMonitor {
    pub path: PathBuf,
    trace: Arc<Mutex<TraceReader>>,
    _watcher: notify::RecommendedWatcher,
    _directory: tempfile::TempDir,
}
impl CommitMonitor {
    pub fn new(reporter: CommitReporter) -> Result<Self, GitError> {
        let directory = tempfile::Builder::new()
            .prefix("t3code-git-trace2-")
            .tempdir()?;
        let path = directory.path().join("events.json");
        std::fs::write(&path, "")?;
        let trace = Arc::new(Mutex::new(TraceReader {
            path: path.clone(),
            processed: 0,
            hooks: BTreeMap::new(),
            reporter,
        }));
        let callback = trace.clone();
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                if result.is_ok()
                    && let Ok(mut trace) = callback.lock()
                {
                    trace.flush(false);
                }
            })
            .map_err(|_| GitError::InvalidOutput("Failed to create Git trace monitor.".into()))?;
        watcher
            .watch(&path, notify::RecursiveMode::NonRecursive)
            .map_err(|_| GitError::InvalidOutput("Failed to create Git trace monitor.".into()))?;
        Ok(Self {
            path,
            trace,
            _watcher: watcher,
            _directory: directory,
        })
    }
}
impl Drop for CommitMonitor {
    fn drop(&mut self) {
        if let Ok(mut trace) = self.trace.lock() {
            trace.flush(true);
        }
    }
}

pub(crate) struct OutputLines {
    pending: Arc<Mutex<Vec<u8>>>,
    reporter: CommitReporter,
    stream: &'static str,
}
impl OutputLines {
    pub fn callback(&self) -> t3_process::OutputCallback {
        let pending = self.pending.clone();
        let reporter = self.reporter.clone();
        let stream = self.stream;
        Arc::new(move |bytes| {
            let Ok(mut pending) = pending.lock() else {
                return;
            };
            pending.extend_from_slice(bytes);
            while let Some(index) = pending
                .iter()
                .position(|byte| *byte == b'\n' || *byte == b'\r')
            {
                let bytes: Vec<_> = pending.drain(..=index).collect();
                let text = String::from_utf8_lossy(&bytes[..bytes.len() - 1]);
                if !text.is_empty() {
                    reporter(CommitProgress::Output {
                        stream,
                        text: crate::diagnostics::sanitize_diagnostic(&text),
                    });
                }
            }
            if pending.len() > 64 * 1024 {
                pending.clear();
            }
        })
    }
}
impl Drop for OutputLines {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.pending.lock()
            && !pending.is_empty()
        {
            let text = String::from_utf8_lossy(&pending).into_owned();
            pending.clear();
            (self.reporter)(CommitProgress::Output {
                stream: self.stream,
                text: crate::diagnostics::sanitize_diagnostic(&text),
            });
        }
    }
}
pub(crate) fn output_lines(stream: &'static str, reporter: CommitReporter) -> OutputLines {
    OutputLines {
        pending: Arc::new(Mutex::new(Vec::new())),
        reporter,
        stream,
    }
}
