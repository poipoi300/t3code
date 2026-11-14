use serde_json::json;
use std::{path::Path, sync::Arc};
use t3_contracts::source_control::*;
use t3_forge::{registry::ForgeRegistry, services::SourceControlRepositoryService};

fn git(cwd: &Path) -> t3_git::Git {
    t3_git::Git::new(cwd).with_environment(
        [
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
        ]
        .into(),
    )
}
#[tokio::test]
async fn local_clone_names_are_operands_even_when_they_start_with_a_dash() {
    let dir = tempfile::tempdir().unwrap();
    let service = SourceControlRepositoryService::new(
        Arc::new(ForgeRegistry::default()),
        git(dir.path()),
        dir.path().into(),
    );
    for remote in ["--bare", "source.git"] {
        let input = serde_json::from_value(
            json!({"remoteUrl":remote,"destinationPath":dir.path().join("clone")}),
        )
        .unwrap();
        let prepared = service.prepare_clone(input).await.unwrap();
        assert_eq!(prepared.clone_url, remote);
    }
}
#[tokio::test]
async fn lookup_exposes_only_fixed_repository_hints_and_never_raw_provider_text() {
    struct Failure {
        detail: &'static str,
        caps: t3_forge::PullRequestCapabilities,
    }
    #[async_trait::async_trait]
    impl t3_forge::Forge for Failure {
        fn kind(&self) -> SourceControlProviderKind {
            "bitbucket".into()
        }
        fn capabilities(&self) -> &t3_forge::PullRequestCapabilities {
            &self.caps
        }
        fn supports(&self, _: &str) -> bool {
            true
        }
        async fn get_repository_clone_urls(
            &self,
            _: t3_forge::forge::CloneUrlsInput,
        ) -> t3_forge::Result<SourceControlRepositoryCloneUrls> {
            Err(t3_forge::ForgeError::provider(
                "bitbucket",
                "repository",
                t3_forge::FailureReason::Failed,
                self.detail,
            ))
        }
    }
    let dir = tempfile::tempdir().unwrap();
    for (detail, expected) in [
        (
            "<html>private-token</html>",
            "The source control operation could not be completed.",
        ),
        (
            "Repositories are named workspace/repository.",
            "Repositories are named workspace/repository.",
        ),
    ] {
        let caps=serde_json::from_value(json!({"diff":false,"comment":false,"actions":[],"mergeMethods":[],"search":false,"review":{"inlineComment":false,"reply":false,"resolve":false,"verdicts":[]},"reviewers":{"request":false,"listCandidates":false}})).unwrap();
        let registry = Arc::new(ForgeRegistry::new([
            Arc::new(Failure { detail, caps }) as Arc<dyn t3_forge::Forge>
        ]));
        let service =
            SourceControlRepositoryService::new(registry, git(dir.path()), dir.path().into());
        let error = service
            .lookup_repository(
                serde_json::from_value(json!({"provider":"bitbucket","repository":"invalid"}))
                    .unwrap(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.wire.detail, expected);
        assert!(!format!("{error:?}").contains("private-token"));
    }
}

#[tokio::test]
async fn clone_copies_a_real_repository_and_refuses_nonempty_destinations() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    tokio::fs::create_dir(&source).await.unwrap();
    let driver = git(&source);
    driver
        .execute(&["init", "--initial-branch=main"], Default::default())
        .await
        .unwrap();
    tokio::fs::write(source.join("tracked.txt"), "recorded\n")
        .await
        .unwrap();
    driver
        .execute(&["add", "--", "tracked.txt"], Default::default())
        .await
        .unwrap();
    driver
        .execute(&["commit", "-m", "fixture"], Default::default())
        .await
        .unwrap();
    let service = SourceControlRepositoryService::new(
        Arc::new(ForgeRegistry::default()),
        driver,
        root.path().into(),
    );
    let destination = root.path().join("-clone");
    let input: SourceControlCloneRepositoryInput =
        serde_json::from_value(json!({"remoteUrl":source,"destinationPath":destination})).unwrap();
    let cloned = service
        .clone_repository(input.clone(), Default::default())
        .await
        .unwrap();
    assert_eq!(cloned.cwd.0, destination.to_string_lossy());
    assert_eq!(
        tokio::fs::read_to_string(destination.join("tracked.txt"))
            .await
            .unwrap(),
        "recorded\n"
    );
    assert!(cloned.repository.is_none());
    let error = service
        .clone_repository(input, Default::default())
        .await
        .unwrap_err();
    assert_eq!(
        error.wire.detail,
        "Destination path already exists and is not empty."
    );
    assert_eq!(
        tokio::fs::read_to_string(destination.join("tracked.txt"))
            .await
            .unwrap(),
        "recorded\n"
    );

    let reject = root.path().join("reject.sh");
    tokio::fs::write(
        &reject,
        "printf '%s\\n' 'Permission denied (publickey).' >&2\nexit 255\n",
    )
    .await
    .unwrap();
    let file_url = url::Url::from_directory_path(&source).unwrap().to_string();
    let fallback_driver = git(root.path()).with_environment(
        [
            ("GIT_SSH_VARIANT".into(), Some("ssh".into())),
            (
                "GIT_SSH_COMMAND".into(),
                Some(format!("sh '{}'", reject.to_string_lossy().replace('\\', "/")).into()),
            ),
            ("GIT_CONFIG_COUNT".into(), Some("1".into())),
            (
                "GIT_CONFIG_KEY_0".into(),
                Some(format!("url.{file_url}.insteadOf").into()),
            ),
            (
                "GIT_CONFIG_VALUE_0".into(),
                Some("https://github.com/acme/web.git".into()),
            ),
        ]
        .into(),
    );
    let service = SourceControlRepositoryService::new(
        Arc::new(ForgeRegistry::default()),
        fallback_driver,
        root.path().into(),
    );
    let fallback = root.path().join("fallback");
    let input: SourceControlCloneRepositoryInput = serde_json::from_value(
        json!({"remoteUrl":"git@github.com:acme/web.git","destinationPath":fallback}),
    )
    .unwrap();
    let cloned = service
        .clone_repository(input, Default::default())
        .await
        .unwrap();
    assert_eq!(cloned.remote_url.0, "https://github.com/acme/web.git");
    assert_eq!(
        tokio::fs::read_to_string(fallback.join("tracked.txt"))
            .await
            .unwrap(),
        "recorded\n"
    );
    let explicit = root.path().join("explicit-ssh");
    let input = serde_json::from_value(json!({"remoteUrl":"git@github.com:acme/web.git","destinationPath":explicit,"protocol":"ssh"})).unwrap();
    let failure = service
        .clone_repository(input, Default::default())
        .await
        .unwrap_err();
    assert_eq!(
        failure.git_reason,
        Some(t3_contracts::git::GitCommandFailureReason::AuthenticationFailed)
    );
    assert!(!explicit.join("tracked.txt").exists());
}

#[tokio::test]
async fn clone_failure_never_exposes_url_credentials_and_discard_preserves_unrelated_files() {
    let root = tempfile::tempdir().unwrap();
    let service = SourceControlRepositoryService::new(
        Arc::new(ForgeRegistry::default()),
        git(root.path()),
        root.path().into(),
    );
    let destination = root.path().join("failed");
    let input = serde_json::from_value(
        json!({"remoteUrl":"--upload-pack=remote-secret","destinationPath":destination}),
    )
    .unwrap();
    let error = service
        .clone_repository(input, Default::default())
        .await
        .unwrap_err();
    assert!(!format!("{error:?}").contains("remote-secret"));
    tokio::fs::create_dir(&destination).await.unwrap();
    tokio::fs::write(destination.join("keep.txt"), "keep")
        .await
        .unwrap();
    assert!(service.discard_clone(&destination).await.is_err());
    assert_eq!(
        tokio::fs::read_to_string(destination.join("keep.txt"))
            .await
            .unwrap(),
        "keep"
    );
    let missing = root.path().join("missing");
    service.discard_clone(&missing).await.unwrap();
    assert!(missing.is_dir());
}

#[test]
fn clone_progress_preserves_stages_and_counters_without_exposing_diagnostic_lines() {
    use t3_contracts::project_clone::ProjectCloneStage::*;
    use t3_forge::clone_progress::parse_git_clone_progress_line as parse;
    let cases = [
        (
            "remote: Enumerating objects: 123, done.",
            Counting,
            None,
            None,
        ),
        (
            "remote: Counting objects: 100% (4/4), done.",
            Counting,
            Some(100),
            None,
        ),
        (
            "Receiving objects: 42% (42/100), 2.00 MiB | 1.00 MiB/s",
            Receiving,
            Some(42),
            Some("2.00 MiB | 1.00 MiB/s"),
        ),
        (
            "Resolving deltas: 200% (2/1), done.",
            Resolving,
            Some(100),
            None,
        ),
        (
            "Checking out files: 50% (1/2), done.",
            Checkout,
            Some(50),
            None,
        ),
        ("Updating files: 50% (1/2) broken", Checkout, None, None),
    ];
    for (line, stage, percent, detail) in cases {
        let value = parse(line).unwrap();
        assert_eq!(value.stage, stage);
        assert_eq!(value.percent, percent);
        assert_eq!(value.detail.as_deref(), detail);
    }
    assert!(parse("fatal: https://oauth:secret@github.com/acme/web?token=hidden").is_none());
    assert_eq!(
        parse(
            "Receiving objects: 42% (42/100), https://oauth:secret@github.com/acme/web?token=hidden"
        )
        .unwrap()
        .detail
        .as_deref(),
        Some("https://github.com/acme/web")
    );
}

#[tokio::test]
async fn sign_in_dispatch_preserves_unsupported_and_optional_operation_results() {
    use t3_forge::services::SourceControlSignIn;
    let service = SourceControlSignIn::new(Arc::new(ForgeRegistry::default()));
    let input: SourceControlSignInInput =
        serde_json::from_value(json!({"provider":"gitlab","host":"gitlab.com"})).unwrap();
    for error in [
        service.start(input.clone()).await.unwrap_err(),
        service.wait(input.clone()).await.unwrap_err(),
        service.cancel(input).await.unwrap_err(),
    ] {
        assert_eq!(error.wire.reason, SourceControlSignInFailure::Unsupported);
        assert_eq!(error.wire.provider.0, "gitlab");
        assert_eq!(error.wire.host, "gitlab.com");
    }
    let input: SourceControlAppAccessInput = serde_json::from_value(
        json!({"provider":"gitlab","host":"gitlab.com","repository":"acme/web"}),
    )
    .unwrap();
    assert_eq!(
        service.access(input).await.status,
        SourceControlAppAccessStatus::SignedOut
    );
    let input: SourceControlStarredInput = serde_json::from_value(
        json!({"provider":"gitlab","host":"gitlab.com","repository":"acme/web"}),
    )
    .unwrap();
    assert!(!service.starred(input.clone()).await);
    assert!(!service.star(input).await);
}

#[tokio::test]
async fn real_git_clone_failure_redacts_userinfo_and_query_from_snapshots_and_errors() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buffer = [0; 8192];
        assert!(stream.read(&mut buffer).await.unwrap() > 0);
        stream
            .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let service = SourceControlRepositoryService::new(
        Arc::new(ForgeRegistry::default()),
        git(root.path()),
        root.path().into(),
    );
    let remote = format!("http://oauth:remote-secret@{endpoint}/repo?token=capability");
    let input: SourceControlCloneRepositoryInput = serde_json::from_value(
        json!({"remoteUrl":remote,"destinationPath":root.path().join("clone")}),
    )
    .unwrap();
    let prepared = service.prepare_clone(input.clone()).await.unwrap();
    assert_eq!(prepared.remote_url, format!("http://{endpoint}/repo"));
    assert!(!format!("{prepared:?}").contains("remote-secret"));
    assert!(!format!("{prepared:?}").contains("capability"));
    let failure = service
        .clone_repository(input, Default::default())
        .await
        .unwrap_err();
    assert_eq!(failure.wire.operation, "cloneRepository");
    assert!(!format!("{failure:?}").contains("remote-secret"));
    assert!(!format!("{failure:?}").contains("capability"));
    server.await.unwrap();
}
