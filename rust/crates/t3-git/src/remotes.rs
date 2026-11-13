use crate::{ExecuteOptions, Git, GitError, core::parse_remote_ref};
use std::time::Duration;

pub fn remote_repository_key(value: &str) -> String {
    let value = value.trim().trim_end_matches('/');
    let lower = value.to_lowercase();
    if lower.starts_with("ssh://")
        || lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("git://")
    {
        if let Ok(url) = url::Url::parse(&lower)
            && let Some(host) = url.host_str()
        {
            let path = url.path().trim_matches('/').trim_end_matches(".git");
            if path.split('/').count() >= 2 {
                return format!("{host}/{path}");
            }
        }
    } else if !value.contains('\\')
        && !lower.as_bytes().get(1).is_some_and(|byte| *byte == b':')
        && let Some((prefix, path)) = lower.split_once(':')
    {
        let host = prefix.rsplit('@').next().unwrap_or("");
        let path = path.trim_start_matches('/').trim_end_matches(".git");
        if !host.is_empty() && !host.contains(['/', ' ']) && path.split('/').count() >= 2 {
            return format!("{host}/{path}");
        }
    }
    lower.trim_end_matches(".git").into()
}

impl Git {
    pub async fn ensure_remote(&self, preferred: &str, url: &str) -> Result<String, GitError> {
        let result = self.ensure_remote_inner(preferred, url).await;
        self.invalidate_caches().await;
        result
    }
    async fn remote_fetch_urls(&self) -> Result<Vec<(String, String)>, GitError> {
        let output = self
            .core_text(
                "GitVcsDriver.ensureRemote.listRemoteUrls",
                &["remote", "-v"],
                false,
            )
            .await?;
        Ok(output
            .lines()
            .filter_map(|line| {
                let (name, rest) = line.split_once('\t')?;
                let (url, suffix) = rest.rsplit_once(" (fetch)")?;
                (suffix.is_empty() || suffix.starts_with(" [") && suffix.ends_with(']'))
                    .then(|| (name.into(), url.into()))
            })
            .collect())
    }
    async fn ensure_remote_inner(&self, preferred: &str, url: &str) -> Result<String, GitError> {
        let urls = self.remote_fetch_urls().await?;
        let key = remote_repository_key(url);
        if let Some((name, _)) = urls
            .iter()
            .find(|(_, url)| remote_repository_key(url) == key)
        {
            return Ok(name.clone());
        }
        let preferred = regex::Regex::new("[^A-Za-z0-9._-]+")
            .map_err(|_| {
                self.command_error("GitVcsDriver.ensureRemote", "Invalid remote-name pattern.")
            })?
            .replace_all(preferred.trim(), "-")
            .trim_matches('-')
            .to_owned();
        let preferred = if preferred.is_empty() {
            "fork"
        } else {
            &preferred
        };
        let mut name = preferred.to_owned();
        for suffix in 1.. {
            if !urls.iter().any(|(existing, _)| existing == &name) {
                break;
            }
            name = format!("{preferred}-{suffix}");
        }
        let result = {
            let _lock = self.shared_config_lock().await;
            self.retry_config_command(
                "GitVcsDriver.ensureRemote.add",
                &["remote", "add", "--", &name, url],
            )
            .await
        };
        if let Err(error) = result {
            if let Some((name, _)) = self
                .remote_fetch_urls()
                .await?
                .into_iter()
                .find(|(_, url)| remote_repository_key(url) == key)
            {
                return Ok(name);
            }
            return Err(error);
        }
        Ok(name)
    }
    pub async fn fetch_remote_branch(
        &self,
        remote: &str,
        remote_branch: &str,
        local: &str,
    ) -> Result<(), GitError> {
        crate::checkpoint::validate_ref_name(&format!("refs/remotes/{remote}/{remote_branch}"))?;
        crate::checkpoint::validate_ref_name(&format!("refs/heads/{remote_branch}"))?;
        crate::checkpoint::validate_ref_name(&format!("refs/heads/{local}"))?;
        let result = async {
            self.execute_command(
                "GitVcsDriver.fetchRemoteBranch.fetch",
                &[
                    "fetch",
                    "--quiet",
                    "--no-tags",
                    "--",
                    remote,
                    &format!("+refs/heads/{remote_branch}:refs/remotes/{remote}/{remote_branch}"),
                ],
                ExecuteOptions::default(),
            )
            .await?;
            if !self.ref_exists(&format!("refs/heads/{local}")).await? {
                return self
                    .create_branch(local, &format!("{remote}/{remote_branch}"))
                    .await;
            }
            let _lock = self.shared_config_lock().await;
            self.execute_command(
                "GitVcsDriver.fetchRemoteBranch.materialize",
                &[
                    "branch",
                    "--force",
                    "--",
                    local,
                    &format!("{remote}/{remote_branch}"),
                ],
                ExecuteOptions::default(),
            )
            .await?;
            Ok(())
        }
        .await;
        self.invalidate_caches().await;
        result
    }
    pub async fn resolve_remote_tracking_commit(
        &self,
        name: &str,
        fallback_remote: &str,
    ) -> Result<(String, String), GitError> {
        let names = self.list_remote_names().await?;
        let reference = parse_remote_ref(name, &names)
            .map(|(remote, branch)| format!("{remote}/{branch}"))
            .unwrap_or_else(|| format!("{fallback_remote}/{name}"));
        let commit = self
            .core_text(
                "GitVcsDriver.resolveRemoteTrackingCommit",
                &[
                    "rev-parse",
                    "--verify",
                    &format!("refs/remotes/{reference}^{{commit}}"),
                ],
                false,
            )
            .await?;
        Ok((commit.trim().into(), reference))
    }
    pub async fn resolve_worktree_base_remote(
        &self,
        name: &str,
    ) -> Result<Option<(String, String)>, GitError> {
        let names = self.list_remote_names().await?;
        let local = self
            .core_text(
                "GitVcsDriver.resolveWorktreeBaseRemote",
                &[
                    "for-each-ref",
                    "--format=%(refname)\t%(upstream:remotename)\t%(upstream:remoteref)",
                    &format!("refs/heads/{name}"),
                ],
                false,
            )
            .await?;
        let local = local
            .lines()
            .find(|line| line.split('\t').next() == Some(&format!("refs/heads/{name}")));
        let target = if let Some(local) = local {
            let fields: Vec<_> = local.split('\t').collect();
            fields
                .get(1)
                .zip(fields.get(2))
                .and_then(|(remote, branch)| {
                    branch
                        .strip_prefix("refs/heads/")
                        .filter(|_| names.iter().any(|name| name == remote))
                        .map(|branch| ((*remote).into(), branch.into()))
                })
        } else {
            parse_remote_ref(name, &names)
        };
        let (remote, branch) = target.unwrap_or_else(|| ("origin".into(), name.into()));
        Ok(names.contains(&remote).then_some((remote, branch)))
    }
    pub async fn fetch_worktree_base_commit(
        &self,
        name: &str,
        remote: Option<(&str, &str)>,
    ) -> Result<Option<(String, String)>, GitError> {
        let result = async {
            let target = match remote {
                Some((remote, branch)) => Some((remote.to_owned(), branch.to_owned())),
                None => self.resolve_worktree_base_remote(name).await?,
            };
            let Some((remote, branch)) = target else {
                return Ok(None);
            };
            let reference = format!("{remote}/{branch}");
            if !self.fetch_remote_scoped(&remote, Some(&reference)).await? {
                return Ok(None);
            }
            let output = self
                .execute_command(
                    "GitVcsDriver.fetchWorktreeBaseCommit",
                    &[
                        "rev-parse",
                        "--verify",
                        "--quiet",
                        &format!("refs/remotes/{reference}^{{commit}}"),
                    ],
                    ExecuteOptions {
                        allow_nonzero: true,
                        ..Default::default()
                    },
                )
                .await?;
            Ok((output.code == Some(0)).then(|| (output.stdout_text().trim().into(), reference)))
        }
        .await;
        self.invalidate_caches().await;
        result
    }
    pub async fn resolve_branch_name(&self, name: &str) -> Result<Option<String>, GitError> {
        let output = self
            .execute_command(
                "GitVcsDriver.resolveBranchName.resolveRef",
                &[
                    "rev-parse",
                    "--symbolic-full-name",
                    "--verify",
                    "--end-of-options",
                    name,
                ],
                ExecuteOptions {
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await?;
        if output.code != Some(0) {
            return Ok(
                if let Some(remote) = self.primary_remote_name().await.ok().flatten()
                    && self.remote_branch_exists(&remote, name).await?
                {
                    Some(name.into())
                } else {
                    None
                },
            );
        }
        let text = output.stdout_text();
        let reference = text.trim();
        if let Some(branch) = reference.strip_prefix("refs/heads/") {
            return Ok(Some(branch.into()));
        }
        if let Some(remote_ref) = reference.strip_prefix("refs/remotes/") {
            return Ok(
                parse_remote_ref(remote_ref, &self.list_remote_names().await?)
                    .map(|(_, branch)| branch),
            );
        }
        Ok(None)
    }
    pub async fn has_commit(&self, name: &str) -> Result<bool, GitError> {
        Ok(self
            .execute_command(
                "GitWorkflowService.hasCommit",
                &[
                    "rev-parse",
                    "--verify",
                    "--end-of-options",
                    &format!("{name}^{{commit}}"),
                ],
                ExecuteOptions {
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await?
            .code
            == Some(0))
    }
    pub async fn has_ref_named(&self, name: &str) -> Result<bool, GitError> {
        if name.contains(['*', '?', '[', '\\']) {
            return Ok(false);
        }
        Ok(!self
            .core_text(
                "GitWorkflowService.hasRefNamed",
                &[
                    "for-each-ref",
                    "--count=1",
                    "--format=%(refname)",
                    &format!("refs/**/{name}"),
                ],
                false,
            )
            .await?
            .trim()
            .is_empty())
    }
    pub async fn head_path(&self) -> Option<PathBuf> {
        self.execute_command(
            "GitWorkflowService.headPath",
            &["rev-parse", "--path-format=absolute", "--git-path", "HEAD"],
            ExecuteOptions {
                allow_nonzero: true,
                ..Default::default()
            },
        )
        .await
        .ok()
        .filter(|output| output.code == Some(0))
        .and_then(|output| {
            let text = output.stdout_text();
            (!text.trim().is_empty()).then(|| PathBuf::from(text.trim()))
        })
    }
    pub async fn init_repository(&self) -> Result<(), GitError> {
        let result = self
            .execute_command(
                "GitVcsDriver.initRepo",
                &["init"],
                ExecuteOptions {
                    timeout: Some(Duration::from_secs(10)),
                    ..Default::default()
                },
            )
            .await;
        self.invalidate_caches().await;
        result.map(|_| ())
    }
}
use std::path::PathBuf;
