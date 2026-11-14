//! `ForgeReviewHost::targets_remote` for every forge kind the registry knows, against TypeScript's
//! `targetsRemote` (`apps/server/src/git/GitManager.ts`): a provider without
//! `createsChangeRequestsIn` creates change requests in the remote, so the answer is `true`. Only
//! GitHub has one; it compares the checkout's repository with the one asked about.
#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use std::{path::Path, sync::Arc};
use t3_forge::{
    Forge,
    azure_devops::AzureDevOps,
    bitbucket::Bitbucket,
    cursor_origin::{CursorOrigin, CursorOriginConfig},
    forgejo::{Forgejo, transport::ForgejoConfig},
    gitcafe::{
        GitCafe,
        credentials::{GitCafeCredentials, GitCafeSettings},
    },
    github::{
        GitHub,
        transport::{CredentialSettings, GitHubCredentials, GitHubHttp},
    },
    gitlab::GitLab,
    http::{ApiCredential, ApiHttp},
    rate_limit::RateLimits,
    registry::ForgeRegistry,
    review_host::{ForgeReviewHost, ReviewSetup},
};
use t3_git::ReviewHost;

struct NoSetup;
#[async_trait]
impl ReviewSetup for NoSetup {
    async fn setup(&self, _: &Path, _: &str, _: &Path) -> Result<(), t3_git::GitError> {
        panic!("targets_remote never runs project setup")
    }
}

fn git(cwd: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", cwd.join(".no-global-config"))
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[tokio::test]
async fn targets_remote_follows_typescript_for_every_forge_kind() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();
    // GitHub's answer reads the checkout's remotes (Git only, no gh).
    git(cwd, &["init", "-q", "-b", "main"]);
    git(
        cwd,
        &["remote", "add", "origin", "https://github.com/acme/web.git"],
    );
    let limits = Arc::new(RateLimits::default());
    let driver = || t3_git::Git::new(cwd);
    let forges: Vec<Arc<dyn Forge>> = vec![
        Arc::new(
            GitHub::new(
                GitHubHttp::new(
                    Arc::new(GitHubCredentials::new(CredentialSettings::default())),
                    limits.clone(),
                )
                .unwrap(),
            )
            .unwrap(),
        ),
        Arc::new(GitLab::new(driver()).unwrap()),
        Arc::new(AzureDevOps::new().unwrap()),
        Arc::new(
            Bitbucket::new(
                ApiHttp::new(
                    "bitbucket",
                    "https://api.bitbucket.org/2.0".parse().unwrap(),
                    ApiCredential::Anonymous,
                    limits.clone(),
                )
                .unwrap(),
                driver(),
            )
            .unwrap(),
        ),
        Arc::new(
            GitCafe::new(
                driver(),
                Arc::new(GitCafeCredentials::new(
                    cwd.into(),
                    GitCafeSettings {
                        token: None,
                        environment: Default::default(),
                    },
                )),
                limits.clone(),
            )
            .unwrap(),
        ),
        Arc::new(
            Forgejo::new(
                driver(),
                ForgejoConfig {
                    authentication_cwd: cwd.into(),
                    keys_paths: Vec::new(),
                    environment: Default::default(),
                },
            )
            .unwrap(),
        ),
        Arc::new(
            CursorOrigin::new(
                driver(),
                CursorOriginConfig {
                    authentication_cwd: cwd.into(),
                    environment: Default::default(),
                },
            )
            .unwrap(),
        ),
    ];
    let registry = ForgeRegistry::new(forges);
    // (kind, origin, repository asked about, TypeScript's answer)
    let rows = [
        ("github", "https://github.com/acme/web.git", "acme/web", true),
        ("github", "https://github.com/acme/web.git", "other/repo", false),
        ("gitlab", "https://gitlab.com/acme/web.git", "other/repo", true),
        (
            "bitbucket",
            "https://bitbucket.org/acme/web.git",
            "other/repo",
            true,
        ),
        (
            "azure-devops",
            "https://dev.azure.com/acme/project/_git/web",
            "other/repo",
            true,
        ),
        ("gitcafe", "https://git.cafe/acme/web.git", "other/repo", true),
        (
            "forgejo",
            "https://codeberg.org/acme/web.git",
            "other/repo",
            true,
        ),
        (
            "cursor-origin",
            "https://origin.cursor.com/acme/web.git",
            "other/repo",
            true,
        ),
        (
            "unknown",
            "https://git.example.invalid/acme/web.git",
            "other/repo",
            true,
        ),
    ];
    for (kind, origin, repository, expected) in rows {
        let handle = registry
            .resolve_remotes(&[("origin".into(), origin.into())], None)
            .unwrap();
        assert_eq!(handle.forge.kind().0, kind, "{origin}");
        let host = ForgeReviewHost::new(handle, Arc::new(NoSetup));
        let answer = host.targets_remote(cwd, "origin", Some(repository)).await;
        assert_eq!(
            answer.as_ref().ok(),
            Some(&expected),
            "{kind} asked about {repository}: {answer:?}"
        );
    }
}
