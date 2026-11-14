use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeProgress {
    CheckoutStarted,
    CheckoutFiles {
        percent: i64,
        completed: u64,
        total: u64,
    },
    CheckoutFinished,
    SubmodulesDisabled {
        source: &'static str,
    },
    SubmodulesStarted,
    SubmoduleLine(String),
    SubmodulesFinished {
        ok: bool,
    },
}
pub type WorktreeReporter = Arc<dyn Fn(WorktreeProgress) + Send + Sync>;
#[derive(Clone, Default)]
pub struct WorktreeCreationOptions {
    pub submodules: Option<crate::WorktreeSubmodules>,
    pub progress: Option<WorktreeReporter>,
}

pub(crate) fn lines(reporter: WorktreeReporter, checkout: bool) -> t3_process::OutputCallback {
    let pending = Mutex::new(Vec::new());
    Arc::new(move |bytes| {
        let mut pending = pending.lock().unwrap_or_else(|e| e.into_inner());
        for byte in bytes {
            if *byte == b'\r' || *byte == b'\n' {
                let line = String::from_utf8_lossy(&pending).trim().to_owned();
                pending.clear();
                if checkout {
                    if let Some((percent, counts)) = line
                        .strip_prefix("Updating files:")
                        .and_then(|line| line.trim().split_once('%'))
                        && let Ok(percent) = percent.trim().parse::<i64>()
                        && let Some(counts) = counts
                            .trim()
                            .strip_prefix('(')
                            .and_then(|counts| counts.split(')').next())
                        && let Some((completed, total)) = counts.split_once('/')
                        && let (Ok(completed), Ok(total)) =
                            (completed.parse::<u64>(), total.parse::<u64>())
                    {
                        reporter(WorktreeProgress::CheckoutFiles {
                            percent: percent.clamp(0, 100),
                            completed,
                            total,
                        });
                    }
                } else if !line.is_empty() {
                    reporter(WorktreeProgress::SubmoduleLine(
                        crate::diagnostics::sanitize_diagnostic(&line),
                    ));
                }
            } else {
                // Hooks can print arbitrarily long lines without separators.
                if pending.len() >= 8192 {
                    pending.clear();
                }
                pending.push(*byte);
            }
        }
    })
}
