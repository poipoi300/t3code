use std::{collections::BTreeMap, path::Path, sync::Arc};
use t3_forge::{
    Forge,
    forge::*,
    github::{GitHub, transport::*},
    rate_limit::RateLimits,
    registry::ForgeRegistry,
};
fn environment() -> t3_process::Environment {
    BTreeMap::from([
        ("GIT_CONFIG_NOSYSTEM".into(), Some("1".into())),
        (
            "GIT_CONFIG_GLOBAL".into(),
            Some(if cfg!(windows) { "NUL" } else { "/dev/null" }.into()),
        ),
        ("GIT_AUTHOR_NAME".into(), Some("Fixture".into())),
        (
            "GIT_AUTHOR_EMAIL".into(),
            Some("fixture@example.invalid".into()),
        ),
        ("GIT_COMMITTER_NAME".into(), Some("Fixture".into())),
        (
            "GIT_COMMITTER_EMAIL".into(),
            Some("fixture@example.invalid".into()),
        ),
    ])
}
async fn git(cwd: &Path, args: &[&str]) {
    let mut options = t3_process::ProcessOptions::new("git").args(args);
    options.cwd = Some(cwd.into());
    options.env = environment();
    t3_process::run(options).await.unwrap();
}
async fn git_output(cwd: &Path, args: &[&str]) -> String {
    let mut options = t3_process::ProcessOptions::new("git").args(args);
    options.cwd = Some(cwd.into());
    options.env = environment();
    t3_process::run(options)
        .await
        .unwrap()
        .stdout_text()
        .trim()
        .into()
}
#[tokio::test]
async fn stack_rebase_replays_each_layers_own_commits_with_leased_pushes() {
    use t3_forge::github::stack::{CascadeInput, CascadeLayer};
    let root = tempfile::tempdir().unwrap();
    let remote = root.path().join("remote.git");
    let work = root.path().join("work");
    git(
        root.path(),
        &[
            "init",
            "--bare",
            "--initial-branch=main",
            remote.to_str().unwrap(),
        ],
    )
    .await;
    git(
        root.path(),
        &["clone", remote.to_str().unwrap(), work.to_str().unwrap()],
    )
    .await;
    std::fs::write(work.join("index.ts"), "export const answer = 41;\n").unwrap();
    git(&work, &["add", "--all"]).await;
    git(
        &work,
        &[
            "-c",
            "core.hooksPath=",
            "commit",
            "--no-gpg-sign",
            "-m",
            "Initial",
        ],
    )
    .await;
    git(&work, &["switch", "-c", "feat/answer-42"]).await;
    std::fs::write(work.join("index.ts"), "export const answer = 42;\n").unwrap();
    git(
        &work,
        &[
            "-c",
            "core.hooksPath=",
            "commit",
            "--no-gpg-sign",
            "-am",
            "A: set the answer to 42",
        ],
    )
    .await;
    let bottom = git_output(&work, &["rev-parse", "HEAD"]).await;
    git(&work, &["switch", "-c", "feat/answer-doc"]).await;
    std::fs::write(
        work.join("index.ts"),
        "/** The answer. */\nexport const answer = 42;\n",
    )
    .unwrap();
    git(
        &work,
        &[
            "-c",
            "core.hooksPath=",
            "commit",
            "--no-gpg-sign",
            "-am",
            "B: document the answer",
        ],
    )
    .await;
    let top = git_output(&work, &["rev-parse", "HEAD"]).await;
    git(&work, &["switch", "main"]).await;
    std::fs::write(work.join("README.md"), "main moved ahead\n").unwrap();
    git(&work, &["add", "--all"]).await;
    git(
        &work,
        &[
            "-c",
            "core.hooksPath=",
            "commit",
            "--no-gpg-sign",
            "-m",
            "Docs",
        ],
    )
    .await;
    git(
        &work,
        &[
            "push",
            "origin",
            "main",
            "feat/answer-42",
            "feat/answer-doc",
        ],
    )
    .await;
    let credentials = CredentialSettings {
        tokens: BTreeMap::from([("github.com".into(), "fixture-token".into())]),
        ..Default::default()
    };
    let github = GitHub::new(
        GitHubHttp::new(
            Arc::new(GitHubCredentials::new(credentials)),
            Arc::new(RateLimits::default()),
        )
        .unwrap(),
    )
    .unwrap()
    .with_git_environment(environment());
    let completed = github
        .cascade_rebase_stack(CascadeInput {
            host: "github.com".into(),
            repository: "acme/web".into(),
            base: "main".into(),
            merged_head_sha: None,
            remote: Some(remote.to_string_lossy().into()),
            layers: vec![
                CascadeLayer {
                    number: 1,
                    head_branch: "feat/answer-42".into(),
                    head_sha: bottom,
                },
                CascadeLayer {
                    number: 2,
                    head_branch: "feat/answer-doc".into(),
                    head_sha: top,
                },
            ],
        })
        .await
        .unwrap();
    assert_eq!(completed, 2);
    let main = git_output(&remote, &["rev-parse", "main"]).await;
    let bottom = git_output(&remote, &["rev-parse", "feat/answer-42"]).await;
    assert_eq!(
        git_output(&remote, &["rev-parse", "feat/answer-42~1"]).await,
        main
    );
    assert_eq!(
        git_output(&remote, &["rev-parse", "feat/answer-doc~1"]).await,
        bottom
    );
    assert_eq!(
        git_output(
            &remote,
            &["log", "--format=%s", "feat/answer-42..feat/answer-doc"]
        )
        .await,
        "B: document the answer"
    );
    assert_eq!(
        git_output(&remote, &["show", "feat/answer-doc:index.ts"]).await,
        "/** The answer. */\nexport const answer = 42;"
    );
}
fn github() -> GitHub {
    let http = GitHubHttp::new(
        Arc::new(GitHubCredentials::new(CredentialSettings::default())),
        Arc::new(RateLimits::default()),
    )
    .unwrap();
    GitHub::new(http)
        .unwrap()
        .with_git_environment(environment())
}
#[tokio::test]
async fn templates_read_committed_blobs_and_honor_directory_ambiguity() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "--initial-branch=main"]).await;
    std::fs::create_dir(dir.path().join(".github")).unwrap();
    std::fs::write(
        dir.path().join(".github/PULL_REQUEST_TEMPLATE.md"),
        "Committed template\n",
    )
    .unwrap();
    git(dir.path(), &["add", "--all"]).await;
    git(
        dir.path(),
        &[
            "-c",
            "core.hooksPath=",
            "commit",
            "--no-gpg-sign",
            "-m",
            "fixture",
        ],
    )
    .await;
    std::fs::write(
        dir.path().join(".github/PULL_REQUEST_TEMPLATE.md"),
        "Dirty template",
    )
    .unwrap();
    let github = github();
    assert_eq!(
        github
            .read_change_request_template(TemplateInput {
                cwd: dir.path().into(),
                treeish: "HEAD".into()
            })
            .await
            .unwrap()
            .as_deref(),
        Some("Committed template")
    );
    github
        .record_branch_base(RecordBranchBaseInput {
            cwd: dir.path().into(),
            branch: "feature".into(),
            base_branch: "main".into(),
        })
        .await
        .unwrap();
    let value = t3_git::Git::new(dir.path())
        .with_environment(environment())
        .read_config_value("branch.feature.gh-merge-base")
        .await
        .unwrap();
    assert_eq!(value.as_deref(), Some("main"));
    std::fs::remove_file(dir.path().join(".github/PULL_REQUEST_TEMPLATE.md")).unwrap();
    std::fs::create_dir(dir.path().join(".github/PULL_REQUEST_TEMPLATE")).unwrap();
    for name in ["one.md", "two.md"] {
        std::fs::write(
            dir.path().join(".github/PULL_REQUEST_TEMPLATE").join(name),
            name,
        )
        .unwrap();
    }
    git(dir.path(), &["add", "--all"]).await;
    git(
        dir.path(),
        &[
            "-c",
            "core.hooksPath=",
            "commit",
            "--no-gpg-sign",
            "-m",
            "ambiguous",
        ],
    )
    .await;
    assert!(
        github
            .read_change_request_template(TemplateInput {
                cwd: dir.path().into(),
                treeish: "HEAD".into()
            })
            .await
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
async fn registry_resolves_real_git_remotes_and_unknown_listing_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init"]).await;
    git(
        dir.path(),
        &[
            "remote",
            "add",
            "upstream",
            "https://github.com/acme/web.git",
        ],
    )
    .await;
    git(
        dir.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://custom.example/acme/web.git",
        ],
    )
    .await;
    let registry = ForgeRegistry::default().with_git_environment(environment());
    let handle = registry.resolve(dir.path(), None).await.unwrap();
    assert_eq!(handle.forge.kind().0, "unknown");
    assert_eq!(handle.context.as_ref().unwrap().remote_name, "origin");
    let rows = handle
        .forge
        .checkout_list_change_requests(CheckoutListInput {
            checkout: CheckoutInput {
                cwd: dir.path().into(),
                context: handle.context,
            },
            source: None,
            head_selector: "feature".into(),
            state: None,
            limit: None,
        })
        .await
        .unwrap();
    assert!(rows.is_empty());
}
