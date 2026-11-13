use super::{GitHub, adapter::repo_parts, decode::*, repository::encode, transport::error};
use crate::{FailureReason, Result, forge::PullRequestInput, model::*};
use reqwest::Method;
use serde_json::{Value, json};
use std::collections::HashSet;
impl GitHub {
    pub(super) async fn enrich_workflows(
        &self,
        input: &PullRequestInput,
        value: &Value,
        result: &mut ProviderChangeRequestDetail,
    ) -> Result<()> {
        let raw = pr(value)?;
        if state(raw) != "open" {
            result.workflow_approvals_required = Some(0);
            return Ok(());
        }
        let approval = if raw["isCrossRepository"] == true {
            self.approval_runs(input, raw).await
        } else {
            Ok(Vec::new())
        };
        let mut unavailable = false;
        let approval = match approval {
            Ok(runs) => {
                result.workflow_approvals_required = Some(runs.len() as u64);
                runs
            }
            Err(e) if e.reason() == Some(FailureReason::RateLimited) => return Err(e),
            Err(_) => {
                unavailable = true;
                Vec::new()
            }
        };
        let (owner, name) = repo_parts(&input.repo.repository)?;
        if let Some(sha) = text(raw, "headRefOid") {
            let path = format!(
                "repos/{owner}/{name}/actions/runs?head_sha={}&per_page=100",
                encode(sha)
            );
            match self
                .rest_json(
                    &input.repo.host,
                    &input.repo.cwd,
                    "listUnfinishedWorkflowRuns",
                    Method::GET,
                    &path,
                    None,
                )
                .await
            {
                Ok(value) => {
                    for run in value["workflow_runs"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|r| {
                            matches!(
                                r["status"].as_str(),
                                Some(
                                    "requested" | "queued" | "pending" | "waiting" | "in_progress"
                                )
                            )
                        })
                    {
                        if !represented(&result.checks, run, false) {
                            result.checks.push(workflow_check(run, false)?);
                        }
                    }
                }
                Err(e) if e.reason() == Some(FailureReason::RateLimited) => return Err(e),
                Err(_) => {}
            }
        }
        for run in approval {
            if !represented(&result.checks, &run, true) {
                result.checks.push(workflow_check(&run, true)?);
            }
        }
        if unavailable {
            result.checks.push(parse(json!({"name":"Workflow approval status","status":"action-required","description":"GitHub could not determine whether workflows are awaiting approval.","url":null,"placeholder":true}),"workflowChecks")?);
        }
        Ok(())
    }
    async fn approval_runs(&self, input: &PullRequestInput, raw: &Value) -> Result<Vec<Value>> {
        let sha = text(raw, "headRefOid").ok_or_else(|| {
            error(
                "workflowApproval",
                FailureReason::Failed,
                "The pull request head is unavailable.",
            )
        })?;
        let owner = text(&raw["headRepositoryOwner"], "login").ok_or_else(|| {
            error(
                "workflowApproval",
                FailureReason::Failed,
                "The fork owner is unavailable.",
            )
        })?;
        let branch = text(raw, "headRefName").ok_or_else(|| {
            error(
                "workflowApproval",
                FailureReason::Failed,
                "The source branch is unavailable.",
            )
        })?;
        let (mut after, mut seen, mut heads) = (None::<String>, HashSet::new(), Vec::new());
        for _ in 0..11 {
            let value = self
                .query(
                    input,
                    "PullRequestHeads",
                    json!({"head":branch,"after":after}),
                )
                .await?;
            let conn = &value["repository"]["pullRequests"];
            heads.extend(nodes(conn).into_iter().cloned());
            after = next_cursor(conn);
            if after.is_none() {
                break;
            }
            if heads.len() > 1000 || !seen.insert(after.clone()) {
                return Err(error(
                    "workflowApproval",
                    FailureReason::Failed,
                    "The matching head list is truncated.",
                ));
            }
        }
        if after.is_some() || heads.len() > 1000 {
            return Err(error(
                "workflowApproval",
                FailureReason::Failed,
                "The matching head list is truncated.",
            ));
        }
        let exact: Vec<_> = heads
            .iter()
            .filter(|h| {
                h["headRefOid"] == sha
                    && h["isCrossRepository"] == true
                    && text(&h["headRepositoryOwner"], "login")
                        .is_some_and(|s| s.eq_ignore_ascii_case(owner))
            })
            .collect();
        if exact.len() != 1 || exact[0]["number"] != input.number {
            return Err(error(
                "workflowApproval",
                FailureReason::Failed,
                "The source head does not identify one pull request.",
            ));
        }
        let (repo_owner, name) = repo_parts(&input.repo.repository)?;
        let mut runs = Vec::new();
        for page in 1..=11 {
            let path = format!(
                "repos/{repo_owner}/{name}/actions/runs?head_sha={}&branch={}&event=pull_request&status=action_required&per_page=100&page={page}",
                encode(sha),
                encode(branch)
            );
            let value = self
                .rest_json(
                    &input.repo.host,
                    &input.repo.cwd,
                    "listWorkflowRunsRequiringApproval",
                    Method::GET,
                    &path,
                    None,
                )
                .await?;
            let page = value["workflow_runs"].as_array().ok_or_else(|| {
                error(
                    "workflowApproval",
                    FailureReason::Failed,
                    "GitHub returned invalid workflow runs.",
                )
            })?;
            runs.extend(page.iter().cloned());
            if runs.len() > 1000 {
                return Err(error(
                    "workflowApproval",
                    FailureReason::Failed,
                    "The workflow run list is truncated.",
                ));
            }
            if page.len() < 100 {
                return Ok(runs);
            }
        }
        Err(error(
            "workflowApproval",
            FailureReason::Failed,
            "The workflow run list is truncated.",
        ))
    }
    pub(super) async fn approve_workflows(&self, input: &PullRequestInput) -> Result<()> {
        let value=self.query(input,"PullRequestCore",json!({"headRef":format!("refs/pull/{}/head",input.number),"required":input.repo.host.eq_ignore_ascii_case("github.com"),"requestedReviewers":100,"mergeQueue":input.repo.host.eq_ignore_ascii_case("github.com")})).await?;
        let raw = pr(&value)?;
        if raw["isCrossRepository"] != true {
            return Ok(());
        }
        let runs = self.approval_runs(input, raw).await?;
        let (owner, name) = repo_parts(&input.repo.repository)?;
        for run in runs {
            let current=self.query(input,"PullRequestCore",json!({"headRef":format!("refs/pull/{}/head",input.number),"required":input.repo.host.eq_ignore_ascii_case("github.com"),"requestedReviewers":100,"mergeQueue":input.repo.host.eq_ignore_ascii_case("github.com")})).await?;
            let now = pr(&current)?;
            if now["isCrossRepository"] != true
                || now["headRefOid"] != raw["headRefOid"]
                || now["headRefName"] != raw["headRefName"]
                || text(&now["headRepositoryOwner"], "login").map(str::to_lowercase)
                    != text(&raw["headRepositoryOwner"], "login").map(str::to_lowercase)
            {
                return Err(error(
                    "approve-workflows",
                    FailureReason::Failed,
                    "The pull request head changed before workflow approval.",
                ));
            }
            let current_runs = self.approval_runs(input, now).await?;
            if current_runs.iter().any(|r| r["id"] == run["id"]) {
                let id = run["id"].as_u64().ok_or_else(|| {
                    error(
                        "approve-workflows",
                        FailureReason::Failed,
                        "Invalid workflow run id.",
                    )
                })?;
                self.rest_json(
                    &input.repo.host,
                    &input.repo.cwd,
                    "approveWorkflowRun",
                    Method::POST,
                    &format!("repos/{owner}/{name}/actions/runs/{id}/approve"),
                    None,
                )
                .await?;
            }
        }
        Ok(())
    }
}
fn represented(checks: &[PullRequestCheck], run: &Value, approval: bool) -> bool {
    let id = run["id"].as_u64().map(|id| id.to_string());
    checks.iter().any(|c| {
        let status = if approval {
            c.status == PullRequestCheckStatus::ActionRequired
        } else {
            matches!(
                c.status,
                PullRequestCheckStatus::Pending | PullRequestCheckStatus::ActionRequired
            )
        };
        status
            && id.as_deref().is_some_and(|id| {
                c.url.as_deref().is_some_and(|url| {
                    url.split("/actions/runs/")
                        .nth(1)
                        .and_then(|s| s.split('/').next())
                        == Some(id)
                })
            })
    })
}
fn workflow_check(run: &Value, approval: bool) -> Result<PullRequestCheck> {
    parse(
        json!({"name":text(run,"name").map(str::to_string).unwrap_or_else(||format!("Workflow run {}",run["id"])),"status":if approval{"action-required"}else{"pending"},"description":if approval{"A maintainer must approve this workflow before it can run."}else{"This workflow run has not finished."},"url":text(run,"html_url")}),
        "workflowChecks",
    )
}
