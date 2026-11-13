use crate::{
    error::{Error, Result},
    runtime::threads::date_ms,
};
use serde::{Deserialize, Serialize};
use t3_contracts::{
    base_schemas::{IsoDateTime, TrimmedNonEmptyString},
    orchestration_v2::OrchestrationV2Notification,
    pull_request::{
        PullRequestCheck, PullRequestCheckStatus as Status, PullRequestComment, PullRequestDetail,
        PullRequestMergeability,
    },
    thread_pull_request::{
        ThreadPullRequestWatch, ThreadPullRequestWatchAwaitingBaseline as Baseline,
    },
};

pub const PULL_REQUEST_WATCH_WAKE_LIMIT: u64 = 10;
pub const PULL_REQUEST_WATCH_NO_CHECKS_MS: i64 = 15 * 60000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PullRequestWatchChange {
    ChecksFailed { failed: Vec<PullRequestCheck> },
    ChecksPassed { count: usize, required: bool },
    ChecksAwaitingApproval { checks: Vec<PullRequestCheck> },
    Remarks { remarks: Vec<PullRequestComment> },
    NoChecks { minutes: i64 },
    Conflicting,
    MergeQueueRemoved { reason: Option<String> },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullRequestWatchReport {
    pub changes: Vec<PullRequestWatchChange>,
    pub next: ThreadPullRequestWatch,
    pub exhausted: bool,
}
fn failed(check: &PullRequestCheck) -> bool {
    matches!(check.status, Status::Failure | Status::Cancelled)
}
fn unfinished(check: &PullRequestCheck) -> bool {
    matches!(check.status, Status::Pending | Status::ActionRequired)
}
fn workflow_run(url: &str) -> Option<&str> {
    let index = url.rfind("/actions/runs/")? + "/actions/runs/".len();
    let digits = url[index..].bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0).then_some(&url[..index + digits])
}
fn approval_key(check: &PullRequestCheck) -> TrimmedNonEmptyString {
    TrimmedNonEmptyString(
        check
            .url
            .as_deref()
            .and_then(workflow_run)
            .unwrap_or(&check.name.0)
            .to_owned(),
    )
}
fn active_at(remark: &PullRequestComment) -> &IsoDateTime {
    remark
        .edited_at
        .as_ref()
        .and_then(Option::as_ref)
        .unwrap_or(&remark.created_at)
}

pub fn evaluate_pull_request_watch(
    watch: &ThreadPullRequestWatch,
    detail: &PullRequestDetail,
    remarks: Option<&[PullRequestComment]>,
    now: &IsoDateTime,
) -> Result<PullRequestWatchReport> {
    let now_ms = date_ms(&now.0).ok_or_else(|| Error::Decode {
        entity: "watch clock",
        detail: now.0.clone(),
    })?;
    let mut changes = Vec::new();
    let head_sha = detail
        .head_sha
        .clone()
        .flatten()
        .or_else(|| watch.head_sha.clone());
    let head_moved = watch.head_sha.is_some() && head_sha != watch.head_sha;
    let baseline = watch.awaiting_baseline.as_ref().and_then(Option::as_ref);
    let checks_baseline = baseline.is_some() && !head_moved;
    let mut failed_checks = if head_moved {
        vec![]
    } else {
        watch.failed_checks.clone()
    };
    let mut passed = !head_moved && watch.passed;
    let mut passed_checks = if head_moved {
        vec![]
    } else {
        watch.passed_checks.clone()
    };
    let mut owed_checks = if head_moved {
        vec![]
    } else {
        watch.owed_checks.clone().flatten().unwrap_or_default()
    };
    let mut awaiting_approval = if head_moved {
        vec![]
    } else {
        watch
            .awaiting_approval
            .clone()
            .flatten()
            .unwrap_or_default()
    };
    let mut approval_announced = !head_moved && watch.approval_announced.flatten().unwrap_or(false);
    let old_awaited = watch.checks_awaited_since.clone().flatten();
    let mut checks_pending = !head_moved
        && watch
            .checks_pending
            .flatten()
            .unwrap_or(old_awaited.is_some());
    let unread = detail
        .checks
        .iter()
        .any(|check| check.placeholder.flatten() == Some(true));
    let checks: Vec<_> = detail
        .checks
        .iter()
        .filter(|check| check.placeholder.flatten() != Some(true))
        .collect();
    let failures: Vec<_> = checks
        .iter()
        .copied()
        .filter(|check| failed(check))
        .collect();
    let awaiting: Vec<_> = checks
        .iter()
        .copied()
        .filter(|check| check.status == Status::ActionRequired)
        .collect();
    if !checks.is_empty() {
        checks_pending = checks.iter().any(|check| check.status == Status::Pending);
        let newly_failed: Vec<_> = failures
            .iter()
            .copied()
            .filter(|check| !failed_checks.contains(&check.name))
            .cloned()
            .collect();
        if !checks_baseline && !newly_failed.is_empty() {
            changes.push(PullRequestWatchChange::ChecksFailed {
                failed: newly_failed,
            });
        }
        if !approval_announced && !awaiting.is_empty() {
            if !checks_baseline {
                changes.push(PullRequestWatchChange::ChecksAwaitingApproval {
                    checks: awaiting.iter().copied().cloned().collect(),
                });
            }
            approval_announced = true;
        }
        failed_checks = failures.iter().map(|check| check.name.clone()).collect();
        let mut awaiting_now = Vec::new();
        for check in &awaiting {
            let key = approval_key(check);
            if !awaiting_now.contains(&key) {
                awaiting_now.push(key);
            }
        }
        let jobs_pending = checks.iter().any(|check| {
            check.status == Status::Pending
                && (check.url.is_none() || check.url.as_deref().and_then(workflow_run).is_some())
        });
        let finished: Vec<_> = if jobs_pending {
            vec![]
        } else {
            checks
                .iter()
                .filter(|check| !unfinished(check))
                .filter_map(|check| check.url.as_deref().and_then(workflow_run))
                .collect()
        };
        let previous: Vec<_> = awaiting_approval
            .into_iter()
            .filter(|key| {
                !awaiting_now.contains(key)
                    && if workflow_run(&key.0) == Some(key.0.as_str()) {
                        !finished.contains(&key.0.as_str())
                    } else {
                        !checks.iter().any(|check| check.name == *key)
                            || checks
                                .iter()
                                .any(|check| check.name == *key && unfinished(check))
                    }
            })
            .collect();
        awaiting_now.extend(previous);
        awaiting_approval = awaiting_now;
    }
    let required: Vec<_> = checks
        .iter()
        .copied()
        .filter(|check| check.required.flatten() == Some(true))
        .collect();
    let gate = if required.is_empty() {
        &checks
    } else {
        &required
    };
    let passed_now = awaiting_approval.is_empty()
        && gate
            .iter()
            .all(|check| check.status != Status::Pending && !failed(check));
    if !checks.is_empty() && !(unread && passed_now) {
        let gate_names: Vec<_> = gate.iter().map(|check| check.name.clone()).collect();
        let told = if passed && passed_checks.is_empty() {
            &gate_names
        } else {
            &passed_checks
        };
        let gate_grew = !required.is_empty() && gate_names.iter().any(|name| !told.contains(name));
        let owed = if passed_now && passed {
            checks
                .iter()
                .filter(|check| {
                    check.required.flatten() != Some(true) && owed_checks.contains(&check.name)
                })
                .map(|check| check.name.clone())
                .collect()
        } else {
            vec![]
        };
        let settled = failures.is_empty()
            && awaiting_approval.is_empty()
            && checks.iter().all(|check| check.status != Status::Pending);
        owed_checks = owed;
        if passed_now && (!passed || gate_grew) {
            if !checks_baseline {
                changes.push(PullRequestWatchChange::ChecksPassed {
                    count: gate.len(),
                    required: !required.is_empty(),
                });
            }
            owed_checks = if required.is_empty() {
                vec![]
            } else {
                checks
                    .iter()
                    .filter(|check| {
                        check.required.flatten() != Some(true) && check.status == Status::Pending
                    })
                    .map(|check| check.name.clone())
                    .collect()
            };
        } else if settled && !owed_checks.is_empty() {
            changes.push(PullRequestWatchChange::ChecksPassed {
                count: checks.len(),
                required: false,
            });
            owed_checks.clear();
        }
        passed = passed_now;
        passed_checks = if passed_now { gate_names } else { vec![] };
    }
    let mut checks_awaited_since = if !checks.is_empty() {
        None
    } else if head_moved || baseline == Some(&Baseline::All) {
        Some(now.clone())
    } else {
        old_awaited
    };
    let awaited_ms = checks_awaited_since
        .as_ref()
        .and_then(|date| date_ms(&date.0))
        .map(|at| now_ms - at)
        .unwrap_or(0);
    if !unread && awaited_ms >= PULL_REQUEST_WATCH_NO_CHECKS_MS {
        if detail.capabilities.checks_always_report.flatten() == Some(true) {
            changes.push(PullRequestWatchChange::NoChecks {
                minutes: (awaited_ms + 30000) / 60000,
            });
        }
        checks_awaited_since = None;
        checks_pending = false;
    } else if checks_awaited_since.is_some() {
        checks_pending = true;
    }
    let own = detail
        .viewer
        .as_ref()
        .and_then(Option::as_ref)
        .map(|value| value.0.as_str())
        .or_else(|| detail.author.as_ref().map(|author| author.login.0.as_str()))
        .map(str::to_lowercase);
    let through = date_ms(&watch.remarks_through.0);
    let fresh: Vec<_> = remarks
        .unwrap_or_default()
        .iter()
        .filter(|remark| {
            let at = date_ms(&active_at(remark).0);
            at.is_some()
                && through.is_some()
                && (at > through || (at == through && !watch.remark_ids.contains(&remark.id)))
                && remark
                    .author
                    .as_ref()
                    .map(|author| author.login.0.to_lowercase())
                    != own
        })
        .cloned()
        .collect();
    if !fresh.is_empty() {
        changes.push(PullRequestWatchChange::Remarks {
            remarks: fresh.clone(),
        });
    }
    let latest = fresh
        .iter()
        .filter_map(|remark| date_ms(&active_at(remark).0))
        .chain(through)
        .max();
    let at_latest: Vec<_> = fresh
        .iter()
        .filter(|remark| date_ms(&active_at(remark).0) == latest)
        .collect();
    let remarks_through = if latest == through {
        watch.remarks_through.clone()
    } else {
        at_latest
            .first()
            .map(|remark| active_at(remark).clone())
            .unwrap_or_else(|| watch.remarks_through.clone())
    };
    let mut remark_ids = if latest == through {
        watch.remark_ids.clone()
    } else {
        vec![]
    };
    remark_ids.extend(at_latest.iter().map(|remark| remark.id.clone()));
    let told_conflict = !head_moved && watch.conflicting;
    if baseline != Some(&Baseline::All)
        && detail.mergeability == PullRequestMergeability::Conflicting
        && !told_conflict
    {
        changes.push(PullRequestWatchChange::Conflicting);
    }
    let conflicting = if detail.mergeability == PullRequestMergeability::Unknown {
        told_conflict
    } else {
        detail.mergeability == PullRequestMergeability::Conflicting
    };
    let removal = detail.merge_queue_removal.as_ref().and_then(Option::as_ref);
    let previous_removal = watch
        .merge_queue_removal_id
        .as_ref()
        .and_then(Option::as_ref);
    if let Some(removal) = removal
        && removal.reason.as_deref() != Some("merged")
        && Some(&removal.id) != previous_removal
        && (watch.merge_queue_removal_id.is_some()
            || date_ms(&removal.removed_at.0)
                .zip(date_ms(&watch.started_at.0))
                .is_some_and(|(removed, started)| removed >= started.div_euclid(1000) * 1000))
    {
        changes.push(PullRequestWatchChange::MergeQueueRemoved {
            reason: removal.reason.clone(),
        });
    }
    let merge_queue_removal_id = removal
        .map(|removal| removal.id.clone())
        .or_else(|| previous_removal.cloned());
    let comments_only = !changes.is_empty()
        && changes
            .iter()
            .all(|change| matches!(change, PullRequestWatchChange::Remarks { .. }));
    let progress = head_moved || (!changes.is_empty() && !comments_only);
    let wakes = (if progress { 0 } else { watch.wakes }) + u64::from(comments_only);
    Ok(PullRequestWatchReport {
        changes,
        exhausted: comments_only && wakes >= PULL_REQUEST_WATCH_WAKE_LIMIT,
        next: ThreadPullRequestWatch {
            started_at: watch.started_at.clone(),
            head_sha,
            failed_checks,
            passed,
            passed_checks,
            owed_checks: Some(Some(owed_checks)),
            awaiting_approval: Some(Some(awaiting_approval)),
            approval_announced: Some(Some(approval_announced)),
            remarks_through,
            remark_ids,
            conflicting,
            merge_queue_removal_id: Some(merge_queue_removal_id),
            wakes,
            awaiting_baseline: if checks_baseline
                && (unread || (detail.head_sha.is_none() && checks.is_empty()))
            {
                Some(Some(Baseline::Checks))
            } else {
                None
            },
            checks_awaited_since: Some(checks_awaited_since),
            checks_pending: Some(Some(checks_pending)),
        },
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestWatchWake {
    pub text: String,
    pub notification: OrchestrationV2Notification,
}

const WATCHING_CLOSING: &str = "Look into each item and act on it as your task requires. This PR stays linked and watched, so do not link, watch, or list it again. End your turn when you are done, and T3 Code wakes you on the next change. When you hand the work back to the user, call unwatch_pull_request first so the thread returns to their inbox.";
const WATCHING_CLOSING_PLURAL: &str = "Look into each item and act on it as your task requires. Each PR T3 Code still watches stays linked and watched, so do not link, watch, or list them again. End your turn when you are done, and T3 Code wakes you on the next change. When you hand the work back to the user, call unwatch_pull_request for each first so the thread returns to their inbox.";

fn snippet(body: &str) -> String {
    let mut stripped = String::new();
    let mut remaining = body;
    while let Some(start) = remaining.find("<!--") {
        let Some(end) = remaining[start + 4..].find("-->") else {
            break;
        };
        stripped.push_str(&remaining[..start]);
        stripped.push(' ');
        remaining = &remaining[start + 4 + end + 3..];
    }
    stripped.push_str(remaining);
    let text = stripped
        .split(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if text.encode_utf16().count() <= 200 {
        return text;
    }
    let mut units = 0;
    let mut result = String::new();
    for c in text.chars() {
        if units + c.len_utf16() > 197 {
            break;
        }
        units += c.len_utf16();
        result.push(c);
    }
    result.push_str("...");
    result
}
fn listed<T>(items: &[T], line: impl Fn(&T) -> String) -> Vec<String> {
    let mut lines: Vec<_> = items.iter().take(10).map(line).collect();
    if items.len() > 10 {
        lines.push(format!("  - and {} more", items.len() - 10));
    }
    lines
}
pub fn pull_request_watch_message(
    number: u64,
    url: &str,
    base_branch: &str,
    head_sha: Option<&str>,
    report: &PullRequestWatchReport,
) -> Result<PullRequestWatchWake> {
    let commit = head_sha
        .map(|sha| format!(" on {}", sha.chars().take(7).collect::<String>()))
        .unwrap_or_default();
    let mut lines = vec![format!(
        "Update on pull request #{number} ({url}), which T3 Code is watching for you:"
    )];
    let mut summaries = Vec::new();
    let mut failed = false;
    let mut completed = true;
    for change in &report.changes {
        completed &= matches!(change, PullRequestWatchChange::ChecksPassed { .. });
        let summary = match change {
            PullRequestWatchChange::ChecksFailed { failed: checks } => {
                failed = true;
                lines.push(format!("- Checks failed{commit}:"));
                lines.extend(listed(checks, |check| {
                    let status = if check.status == Status::Cancelled {
                        " (cancelled)"
                    } else {
                        ""
                    };
                    format!(
                        "  - {}{status}{}",
                        check.name.0,
                        check
                            .url
                            .as_ref()
                            .filter(|url| !url.is_empty())
                            .map(|url| format!(" {url}"))
                            .unwrap_or_default()
                    )
                }));
                "checks failed"
            }
            PullRequestWatchChange::ChecksPassed { count, required } => {
                lines.push(format!(
                    "- All {count} {}{} passed{commit}.",
                    if *required { "required " } else { "" },
                    if *count == 1 { "check" } else { "checks" }
                ));
                "checks passed"
            }
            PullRequestWatchChange::ChecksAwaitingApproval { checks } => {
                lines.push(format!(
                    "- Checks are waiting for approval or action from a maintainer{commit}:"
                ));
                lines.extend(listed(checks, |check| {
                    format!(
                        "  - {}{}",
                        check.name.0,
                        check
                            .url
                            .as_ref()
                            .filter(|url| !url.is_empty())
                            .map(|url| format!(" {url}"))
                            .unwrap_or_default()
                    )
                }));
                "checks await approval"
            }
            PullRequestWatchChange::Remarks { remarks } => {
                lines.push(format!(
                    "- {} new {}:",
                    remarks.len(),
                    if remarks.len() == 1 {
                        "comment"
                    } else {
                        "comments"
                    }
                ));
                lines.extend(listed(remarks, |remark| {
                    let body = snippet(&remark.body);
                    let said = if body.is_empty() {
                        remark.review_state.clone().unwrap_or("reviewed".into())
                    } else {
                        format!("\"{body}\"")
                    };
                    format!(
                        "  - {}{}: {said}{}",
                        remark
                            .author
                            .as_ref()
                            .map(|author| author.login.0.as_str())
                            .unwrap_or("someone"),
                        remark
                            .path
                            .as_ref()
                            .map(|path| format!(" on {path}"))
                            .unwrap_or_default(),
                        remark
                            .url
                            .as_ref()
                            .filter(|url| !url.is_empty())
                            .map(|url| format!(" {url}"))
                            .unwrap_or_default()
                    )
                }));
                "new comments"
            }
            PullRequestWatchChange::NoChecks { minutes } => {
                lines.push(format!("- No checks have reported{commit} after {minutes} minutes. None may be configured to run on this change."));
                "no checks reported"
            }
            PullRequestWatchChange::Conflicting => {
                failed = true;
                lines.push(format!("- The branch now conflicts with {base_branch}."));
                "merge conflict"
            }
            PullRequestWatchChange::MergeQueueRemoved { reason } => {
                failed = true;
                lines.push(format!(
                    "- It was removed from the merge queue without merging{}.",
                    reason
                        .as_ref()
                        .map(|reason| format!(": {reason}"))
                        .unwrap_or_default()
                ));
                "removed from merge queue"
            }
        };
        summaries.push(summary);
    }
    lines.push(String::new());
    lines.push(if report.exhausted { summaries.push("stopped watching"); format!("T3 Code stopped watching after {PULL_REQUEST_WATCH_WAKE_LIMIT} comment-only updates in a row. Call watch_pull_request to watch it again.") } else { WATCHING_CLOSING.into() });
    Ok(PullRequestWatchWake {
        text: lines.join("\n"),
        notification: serde_json::from_value(
            serde_json::json!({"source":{"kind":"monitor"},"outcome":if failed { "failed" } else if completed { "completed" } else { "updated" },"summary":format!("#{number}: {}",summaries.join(", "))}),
        )?,
    })
}

pub fn combine_pull_request_watch_wakes(
    first: PullRequestWatchWake,
    rest: &[PullRequestWatchWake],
) -> Result<PullRequestWatchWake> {
    if rest.is_empty() {
        return Ok(first);
    }
    let closing = format!("\n\n{WATCHING_CLOSING}");
    let wakes: Vec<_> = std::iter::once(&first).chain(rest).collect();
    let watching = wakes.iter().any(|wake| wake.text.ends_with(&closing));
    let mut texts: Vec<_> = wakes
        .iter()
        .map(|wake| {
            wake.text
                .strip_suffix(&closing)
                .unwrap_or(&wake.text)
                .to_owned()
        })
        .collect();
    if watching {
        texts.push(WATCHING_CLOSING_PLURAL.into());
    }
    let notifications: Vec<_> = wakes
        .iter()
        .map(|wake| serde_json::to_value(&wake.notification))
        .collect::<std::result::Result<_, _>>()?;
    let outcome = if notifications
        .iter()
        .any(|value| value["outcome"] == "failed")
    {
        "failed"
    } else if notifications
        .iter()
        .all(|value| value["outcome"] == "completed")
    {
        "completed"
    } else {
        "updated"
    };
    let summary = notifications
        .iter()
        .filter_map(|value| value["summary"].as_str())
        .collect::<Vec<_>>()
        .join("; ");
    Ok(PullRequestWatchWake {
        text: texts.join("\n\n"),
        notification: serde_json::from_value(
            serde_json::json!({"source":{"kind":"monitor"},"outcome":outcome,"summary":summary}),
        )?,
    })
}
