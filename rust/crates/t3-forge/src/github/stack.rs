use super::{GitHub, adapter::repo_parts, decode::*, repository::encode, transport::Credential};
use crate::{Forge, ForgeError, Result, forge::*, model::*};
use base64::Engine;
use reqwest::Method;
use serde_json::{Value, json};
use std::{collections::HashSet, time::Duration};

#[derive(Debug, Clone)]
pub struct CascadeLayer {
    pub number: u64,
    pub head_branch: String,
    pub head_sha: String,
}
#[derive(Debug, Clone)]
pub struct CascadeInput {
    pub host: String,
    pub repository: String,
    pub base: String,
    pub layers: Vec<CascadeLayer>,
    pub merged_head_sha: Option<String>,
    pub remote: Option<String>,
}
fn stack_error(number: u64, completed: usize, detail: &str) -> ForgeError {
    ForgeError::Stack {
        number,
        completed,
        detail: detail.into(),
    }
}
fn valid_ref(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.contains("..")
        && !value.contains("@{")
        && !value.ends_with('.')
        && !value.ends_with('/')
        && !value
            .bytes()
            .any(|b| b <= 32 || b == 127 || b"~^:?*[\\".contains(&b))
        && value
            .split('/')
            .all(|s| !s.is_empty() && !s.starts_with('.') && !s.ends_with(".lock"))
}
fn valid_sha(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}
struct CancelOnDrop(t3_process::CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
impl GitHub {
    pub(super) async fn repository_credential(
        &self,
        host: &str,
        repository: &str,
        cwd: &str,
    ) -> Result<Credential> {
        let reached = self.app_reach(host, repository).await?;
        let confirmed = if let Some(app) = &self.app_sign_in {
            match app.token(host).await {
                Ok(Some(credential)) => self
                    .coverage
                    .lock()
                    .await
                    .get(&credential.fingerprint)
                    .is_some_and(|entry| !entry.failed),
                _ => false,
            }
        } else {
            false
        };
        if (reached != t3_contracts::source_control::SourceControlAppAccessStatus::Covered
            || !confirmed)
            && let Ok(personal) = self
                .http
                .personal_credential(host, std::path::Path::new(cwd))
                .await
        {
            return Ok(personal);
        }
        self.http.credential(host, std::path::Path::new(cwd)).await
    }
    pub async fn cascade_rebase_stack(&self, input: CascadeInput) -> Result<usize> {
        if input.layers.is_empty() {
            return Ok(0);
        }
        if !super::transport::valid_host(&input.host)
            || repo_parts(&input.repository).is_err()
            || !valid_ref(&input.base)
            || input
                .layers
                .iter()
                .any(|l| !valid_ref(&l.head_branch) || !valid_sha(&l.head_sha))
            || input
                .merged_head_sha
                .as_ref()
                .is_some_and(|sha| !valid_sha(sha))
        {
            return Err(stack_error(
                input.layers[0].number,
                0,
                "Invalid stack branch or revision.",
            ));
        }
        let credential = self
            .repository_credential(&input.host, &input.repository, ".")
            .await?;
        let token = t3_process::CancellationToken::new();
        let _cancel = CancelOnDrop(token.clone());
        let driver = self.git_driver.clone();
        let environment = self.git_environment.clone();
        tokio::spawn(async move {
            let first = input.layers[0].number;
            let directory = tempfile::Builder::new()
                .prefix("t3-stack-rebase-")
                .tempdir()
                .map_err(|_| stack_error(first, 0, "Could not prepare the stack rebase."))?;
            let remote = input
                .remote
                .unwrap_or_else(|| format!("https://{}/{}.git", input.host, input.repository));
            if remote.starts_with('-')
                || remote.contains(['\0', '\r', '\n'])
                || url::Url::parse(&remote).is_ok_and(|u| {
                    !u.username().is_empty()
                        || u.password().is_some()
                        || matches!(u.scheme(), "http" | "https")
                            && (u.query().is_some() || u.fragment().is_some())
                })
            {
                return Err(stack_error(first, 0, "Invalid stack remote."));
            }
            let authorization = base64::engine::general_purpose::STANDARD
                .encode(format!("x-access-token:{}", credential.token));
            let mut env = environment;
            env.extend([
                ("GIT_CONFIG_NOSYSTEM".into(), Some("1".into())),
                (
                    "GIT_CONFIG_GLOBAL".into(),
                    Some(if cfg!(windows) { "NUL" } else { "/dev/null" }.into()),
                ),
                ("GIT_CONFIG_COUNT".into(), Some("4".into())),
                (
                    "GIT_CONFIG_KEY_0".into(),
                    Some(format!("http.{remote}.extraheader").into()),
                ),
                (
                    "GIT_CONFIG_VALUE_0".into(),
                    Some(format!("AUTHORIZATION: basic {authorization}").into()),
                ),
                ("GIT_CONFIG_KEY_1".into(), Some("user.name".into())),
                ("GIT_CONFIG_VALUE_1".into(), Some("T3 Code".into())),
                ("GIT_CONFIG_KEY_2".into(), Some("user.email".into())),
                ("GIT_CONFIG_VALUE_2".into(), Some("noreply@t3.codes".into())),
                ("GIT_CONFIG_KEY_3".into(), Some("core.hooksPath".into())),
                ("GIT_CONFIG_VALUE_3".into(), Some("".into())),
            ]);
            let git = driver
                .at(directory.path())
                .with_environment(env)
                .with_cancellation(token);
            let run = |args: Vec<String>, allow_nonzero: bool| {
                let git = git.clone();
                async move {
                    git.execute(
                        &args,
                        t3_git::ExecuteOptions {
                            allow_nonzero,
                            timeout: Some(Duration::from_secs(120)),
                            ..Default::default()
                        },
                    )
                    .await
                }
            };
            run(vec!["init".into(), "--quiet".into()], false)
                .await
                .map_err(|_| stack_error(first, 0, "Could not prepare the stack rebase."))?;
            run(
                vec![
                    "remote".into(),
                    "add".into(),
                    "--".into(),
                    "origin".into(),
                    remote,
                ],
                false,
            )
            .await
            .map_err(|_| stack_error(first, 0, "Could not configure the stack remote."))?;
            let mut fetch = vec![
                "fetch".into(),
                "--quiet".into(),
                "--no-tags".into(),
                "--".into(),
                "origin".into(),
                format!(
                    "+refs/heads/{}:refs/remotes/origin/{}",
                    input.base, input.base
                ),
            ];
            fetch.extend(input.layers.iter().map(|l| {
                format!(
                    "+refs/heads/{}:refs/remotes/origin/{}",
                    l.head_branch, l.head_branch
                )
            }));
            run(fetch, false)
                .await
                .map_err(|_| stack_error(first, 0, "Could not fetch the stack."))?;
            let mut parent_old = format!("refs/remotes/origin/{}", input.base);
            let mut parent_new = parent_old.clone();
            let merged = if let Some(sha) = &input.merged_head_sha {
                run(
                    vec![
                        "merge-base".into(),
                        "--is-ancestor".into(),
                        "--".into(),
                        sha.clone(),
                        input.layers[0].head_sha.clone(),
                    ],
                    true,
                )
                .await
                .ok()
                .filter(|o| o.code == Some(0))
                .map(|_| sha.clone())
            } else {
                None
            };
            for (index, layer) in input.layers.iter().enumerate() {
                let fail = |step| stack_error(layer.number, index, step);
                let upstream = if index == 0 {
                    if let Some(merged) = &merged {
                        merged.clone()
                    } else {
                        run(
                            vec![
                                "merge-base".into(),
                                "--".into(),
                                parent_old.clone(),
                                layer.head_sha.clone(),
                            ],
                            false,
                        )
                        .await
                        .map_err(|_| fail("Could not read the stack fork point."))?
                        .stdout_text()
                        .trim()
                        .into()
                    }
                } else {
                    parent_old.clone()
                };
                if !valid_sha(&upstream) {
                    return Err(fail("Invalid stack fork point."));
                }
                run(
                    vec![
                        "checkout".into(),
                        "--quiet".into(),
                        "--detach".into(),
                        layer.head_sha.clone(),
                        "--".into(),
                    ],
                    false,
                )
                .await
                .map_err(|_| fail("Could not check out the stack layer."))?;
                let rebase = run(
                    vec![
                        "rebase".into(),
                        "--quiet".into(),
                        "--onto".into(),
                        parent_new.clone(),
                        "--".into(),
                        upstream,
                    ],
                    true,
                )
                .await
                .map_err(|_| fail("Could not rebase the stack layer."))?;
                if rebase.code != Some(0) {
                    let _ = run(vec!["rebase".into(), "--abort".into()], true).await;
                    return Err(fail("The stack layer conflicts with the layer below it."));
                }
                let rebased = run(
                    vec!["rev-parse".into(), "--verify".into(), "HEAD".into()],
                    false,
                )
                .await
                .map_err(|_| fail("Could not read the rebased stack layer."))?
                .stdout_text()
                .trim()
                .to_string();
                if !valid_sha(&rebased) {
                    return Err(fail("Invalid rebased revision."));
                }
                if rebased != layer.head_sha {
                    run(
                        vec![
                            "push".into(),
                            "--quiet".into(),
                            format!(
                                "--force-with-lease=refs/heads/{}:{}",
                                layer.head_branch, layer.head_sha
                            ),
                            "--".into(),
                            "origin".into(),
                            format!("{rebased}:refs/heads/{}", layer.head_branch),
                        ],
                        false,
                    )
                    .await
                    .map_err(|_| fail("Could not push the rebased stack layer."))?;
                }
                parent_old = layer.head_sha.clone();
                parent_new = rebased;
            }
            Ok(input.layers.len())
        })
        .await
        .map_err(|_| stack_error(0, 0, "The stack rebase worker failed."))?
    }
    pub(super) async fn stack_action(&self, input: ActionInput) -> Result<()> {
        let stack_number = input
            .stack_number
            .ok_or_else(|| stack_error(input.pr.number, 0, "No stack was selected."))?;
        if !matches!(
            input.action,
            PullRequestAction::Merge | PullRequestAction::UpdateBranch
        ) {
            return Err(stack_error(
                input.pr.number,
                0,
                "This operation is not supported for this stack.",
            ));
        }
        let stack = self
            .get_change_request_stack(StackInput {
                pr: input.pr.clone(),
                include_details: false,
            })
            .await?
            .ok_or_else(|| {
                stack_error(
                    input.pr.number,
                    0,
                    "The stack changed. Refresh it before trying again.",
                )
            })?;
        let target_index = stack
            .layers
            .iter()
            .position(|l| l.number == input.pr.number)
            .ok_or_else(|| {
                stack_error(
                    input.pr.number,
                    0,
                    "The stack changed. Refresh it before trying again.",
                )
            })?;
        if stack.number != stack_number
            || input.action == PullRequestAction::UpdateBranch
                && target_index != stack.layers.len() - 1
        {
            return Err(stack_error(
                input.pr.number,
                0,
                "The stack changed. Refresh it before trying again.",
            ));
        }
        let affected = if input.action == PullRequestAction::Merge {
            &stack.layers[..=target_index]
        } else {
            &stack.layers[..]
        };
        let open: Vec<_> = affected
            .iter()
            .filter(|l| l.state != PullRequestState::Merged)
            .collect();
        let expected = input.expected_stack_heads.as_deref().unwrap_or(&[]);
        if expected.len() != open.len()
            || expected
                .iter()
                .map(|l| l.number)
                .collect::<HashSet<_>>()
                .len()
                != open.len()
            || open.iter().any(|layer| {
                layer.head_sha.as_ref().is_none_or(|sha| {
                    !expected
                        .iter()
                        .any(|e| e.number == layer.number && e.head_sha.0 == *sha)
                })
            })
        {
            return Err(stack_error(
                input.pr.number,
                0,
                "The stack changed. Refresh it before trying again.",
            ));
        }
        if open.is_empty()
            || open.iter().any(|l| l.state != PullRequestState::Open)
            || input.action == PullRequestAction::Merge
                && (stack.layers[target_index].state != PullRequestState::Open
                    || open.iter().any(|l| l.is_draft == Some(true)))
        {
            return Err(stack_error(
                input.pr.number,
                0,
                "This operation is not supported for this stack.",
            ));
        }
        if input.action == PullRequestAction::UpdateBranch {
            let targets: Vec<_> = open
                .iter()
                .map(|layer| PullRequestInput {
                    number: layer.number,
                    ..input.pr.clone()
                })
                .collect();
            let access = futures_util::future::try_join_all(
                targets
                    .iter()
                    .map(|target| self.query(target, "PullRequestBranchAccess", Value::Null)),
            )
            .await?;
            for (layer, access) in open.iter().zip(&access) {
                let access = pr(access)?;
                if access["headRepository"].is_null()
                    || access["maintainerCanModify"] != true
                        && !matches!(
                            access["headRepository"]["viewerPermission"].as_str(),
                            Some("ADMIN" | "MAINTAIN" | "WRITE")
                        )
                {
                    return Err(stack_error(
                        layer.number,
                        0,
                        "You cannot push to every stack layer's source branch.",
                    ));
                }
            }
            let first = stack
                .layers
                .iter()
                .position(|l| l.number == open[0].number)
                .unwrap_or(0);
            self.cascade_rebase_stack(CascadeInput {
                host: input.pr.repo.host,
                repository: input.pr.repo.repository,
                base: stack.base,
                layers: open
                    .iter()
                    .map(|l| CascadeLayer {
                        number: l.number,
                        head_branch: l.head_branch.clone(),
                        head_sha: l.head_sha.clone().unwrap_or_default(),
                    })
                    .collect(),
                merged_head_sha: first
                    .checked_sub(1)
                    .and_then(|i| stack.layers[i].head_sha.clone()),
                remote: None,
            })
            .await?;
            return Ok(());
        }
        let (owner, name) = repo_parts(&input.pr.repo.repository)?;
        let root = format!("repos/{owner}/{name}/pulls/{}/merge-async", input.pr.number);
        let method = match input.merge_method {
            Some(PullRequestMergeMethod::Squash) => "squash",
            Some(PullRequestMergeMethod::Rebase) => "rebase",
            _ => "merge",
        };
        let mut result = self.rest_json(&input.pr.repo.host,&input.pr.repo.cwd,"runGitHubStackAction",Method::PUT,&root,Some(json!({"merge_method":method,"merge_action":"default","sha":stack.layers[target_index].head_sha}))).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
        let mut attempt = 0u32;
        while result["status"] == "pending" && tokio::time::Instant::now() < deadline {
            let uuid = text(&result["details"], "uuid")
                .ok_or_else(|| {
                    stack_error(
                        input.pr.number,
                        0,
                        "GitHub returned an unreadable stack operation response.",
                    )
                })?
                .to_string();
            tokio::time::sleep(Duration::from_millis(
                (1000u64.saturating_mul(1 << attempt.min(16))).min(10_000),
            ))
            .await;
            result = self
                .rest_json(
                    &input.pr.repo.host,
                    &input.pr.repo.cwd,
                    "runGitHubStackAction",
                    Method::GET,
                    &format!("{root}/{}", encode(&uuid)),
                    None,
                )
                .await?;
            attempt += 1;
        }
        match result["status"].as_str() {
            Some("merged" | "enqueued") => Ok(()),
            Some("pending") => Err(stack_error(
                input.pr.number,
                0,
                "GitHub is still merging this stack. Refresh before trying again.",
            )),
            Some("failed") => Err(stack_error(
                input.pr.number,
                0,
                "GitHub rejected the stack merge.",
            )),
            _ => Err(stack_error(
                input.pr.number,
                0,
                "GitHub returned an unreadable stack operation response.",
            )),
        }
    }
}
