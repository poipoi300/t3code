mod common;
use common::{Repository, TestResult};
use t3_contracts::vcs::{VcsError, VcsProcessExitFailureKind as Kind};
use t3_git::{ExecuteOptions, GitError};

#[tokio::test]
async fn process_errors_discard_arguments_credentials_and_provider_output() -> TestResult {
    let repo = Repository::new().await?;
    for (message, expected) in [
        (
            "fatal: Authentication failed for https://user:secret@example.invalid/repo?token=private",
            Kind::Authentication,
        ),
        (
            "HTTP 429: too many requests: token=private",
            Kind::RateLimited,
        ),
        ("secret opaque provider diagnostic", Kind::CommandFailed),
    ] {
        repo.command(&[
            "config",
            "alias.reject",
            &format!("!printf '%s\\n' '{message}' >&2; exit 7"),
        ])
        .await?;
        let error = repo
            .git
            .execute_vcs(
                "test.diagnostic",
                &["reject", "private-argument"],
                ExecuteOptions::default(),
                None,
                None,
            )
            .await
            .expect_err("failure");
        let GitError::Vcs(error) = error else {
            panic!("typed process boundary");
        };
        let VcsError::Exit(exit) = &error.error else {
            panic!("exit error");
        };
        assert_eq!(exit.failure_kind, Some(Some(expected)));
        assert_eq!(exit.argument_count, Some(Some(2)));
        let serialized = serde_json::to_string(&error.error)?;
        for value in ["secret", "private", "example.invalid", "user:"] {
            assert!(!serialized.contains(value));
            assert!(!format!("{error:?}").contains(value));
            assert!(!error.to_string().contains(value));
        }
        let actual = repo
            .git
            .execute(
                &["reject", "private-argument"],
                ExecuteOptions {
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await?;
        assert_eq!(
            exit.stderr_length,
            Some(Some(actual.stderr_text().encode_utf16().count() as u64))
        );
    }
    Ok(())
}

#[tokio::test]
async fn command_errors_are_sanitized_and_lock_failures_have_fixed_detail() -> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    repo.command(&["config", "alias.reject", "!printf '%s\\n' 'fatal: Authentication failed https://user:secret@example.invalid/token' >&2; exit 1"]).await?;
    let error = repo
        .git
        .execute_command(
            "test.command",
            &["reject", "private"],
            ExecuteOptions::default(),
        )
        .await
        .expect_err("exit");
    let GitError::Command(error) = error else {
        panic!("command error");
    };
    assert_eq!(
        error.reason,
        Some(t3_contracts::git::GitCommandFailureReason::AuthenticationFailed)
    );
    let contract = serde_json::to_string(&error.to_contract()?)?;
    assert!(
        !contract.contains("secret")
            && !contract.contains("private")
            && !contract.contains("example.invalid")
    );
    std::fs::write(repo.root().join(".git/index.lock"), "held")?;
    let error = repo
        .git
        .execute_vcs(
            "test.lock",
            &["add", "-A"],
            ExecuteOptions::default(),
            None,
            None,
        )
        .await
        .expect_err("locked");
    let GitError::Vcs(error) = error else {
        panic!("process boundary");
    };
    let VcsError::Exit(exit) = error.error else {
        panic!("exit error");
    };
    assert_eq!(exit.detail, t3_git::INDEX_LOCKED_DETAIL);
    assert_eq!(exit.retryable, Some(Some(true)));
    Ok(())
}

#[test]
fn authentication_precedes_rate_limit_and_command_classifier_has_same_precedence() {
    use t3_git::classify_process_failure;
    assert_eq!(
        classify_process_failure("unauthorized, too many requests", None),
        Kind::Authentication
    );
    assert_eq!(
        classify_process_failure("unauthorized", Some(Kind::RateLimited)),
        Kind::Authentication
    );
    assert_eq!(
        classify_process_failure("too many requests", Some(Kind::Authentication)),
        Kind::Authentication
    );
    assert_eq!(
        classify_process_failure("opaque", Some(Kind::NotFound)),
        Kind::NotFound
    );
}

#[tokio::test]
async fn custom_not_found_detail_and_output_limits_are_typed() -> TestResult {
    let repo = Repository::new().await?;
    repo.command(&[
        "config",
        "alias.optional-locks",
        "!printf '%s' \"$GIT_OPTIONAL_LOCKS\"",
    ])
    .await?;
    assert_eq!(
        repo.git
            .execute_command(
                "test.environment",
                &["optional-locks"],
                ExecuteOptions::default()
            )
            .await?
            .stdout_text(),
        "0"
    );
    let mut override_options = ExecuteOptions::default();
    override_options
        .env
        .insert("GIT_OPTIONAL_LOCKS".into(), Some("1".into()));
    assert_eq!(
        repo.git
            .execute_command("test.environment", &["optional-locks"], override_options)
            .await?
            .stdout_text(),
        "1"
    );
    repo.command(&["config", "alias.missing", "!exit 1"])
        .await?;
    let error = repo
        .git
        .execute_vcs(
            "test.not-found",
            &["missing"],
            ExecuteOptions::default(),
            Some(|_| Some(Kind::NotFound)),
            Some("Change request not found."),
        )
        .await
        .expect_err("missing");
    let GitError::Vcs(error) = error else {
        panic!("process boundary");
    };
    let VcsError::Exit(exit) = error.error else {
        panic!("exit error");
    };
    assert_eq!(exit.detail, "Change request not found.");
    repo.command(&[
        "config",
        "alias.output",
        "!printf abcdefghijklmnopqrstuvwxyz",
    ])
    .await?;
    let error = repo
        .git
        .execute_vcs(
            "test.limit",
            &["output"],
            ExecuteOptions {
                max_output_bytes: 3,
                output_mode: t3_git::OutputMode::Error,
                ..Default::default()
            },
            None,
            None,
        )
        .await
        .expect_err("overflow");
    assert!(
        matches!(error, GitError::Vcs(ref error) if matches!(error.error, VcsError::OutputLimit(_)))
    );
    Ok(())
}
