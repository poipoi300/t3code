use super::transport::error;
use crate::{FailureReason, Result};
use async_trait::async_trait;
use std::{path::Path, sync::LazyLock, time::Duration};
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct CliOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}
impl std::fmt::Debug for CliOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CliOutput")
            .field("code", &self.code)
            .finish_non_exhaustive()
    }
}
#[async_trait]
pub trait GitHubCli: Send + Sync {
    async fn run(&self, cwd: &Path, args: &[String]) -> Result<CliOutput>;
}
#[derive(Default)]
pub struct ProcessGitHubCli;
#[async_trait]
impl GitHubCli for ProcessGitHubCli {
    async fn run(&self, cwd: &Path, args: &[String]) -> Result<CliOutput> {
        static SLOTS: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(4));
        let _permit = SLOTS.acquire().await.map_err(|_| {
            error(
                "gh",
                FailureReason::Failed,
                "GitHub CLI request was cancelled.",
            )
        })?;
        let mut options = t3_process::ProcessOptions::new("gh").args(args);
        options.cwd = Some(cwd.into());
        options.timeout = Some(Duration::from_secs(10));
        options.max_output_bytes = 16_000;
        options.check_exit = false;
        options.env.insert("GH_DEBUG".into(), Some("".into()));
        options
            .env
            .insert("GH_PROMPT_DISABLED".into(), Some("1".into()));
        for variable in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_ENTERPRISE_TOKEN",
        ] {
            options.env.insert(variable.into(), None);
        }
        let output = t3_process::run(options)
            .await
            .map_err(|cause| match cause {
                t3_process::ProcessError::NotFound { .. } => error(
                    "gh",
                    FailureReason::MissingTool,
                    "Install GitHub CLI from https://cli.github.com/.",
                ),
                _ => error("gh", FailureReason::Failed, "GitHub CLI failed."),
            })?;
        if output.stdout_truncated || output.stderr_truncated {
            return Err(error(
                "gh",
                FailureReason::Failed,
                "GitHub CLI returned too much output.",
            ));
        }
        Ok(CliOutput {
            code: output.code.unwrap_or(-1),
            stdout: output.stdout_text(),
            stderr: output.stderr_text(),
        })
    }
}
