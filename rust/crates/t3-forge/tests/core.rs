use std::collections::BTreeMap;
use t3_forge::{FailureReason, model::*, rate_limit::*, remote::*};
fn key(scope: &str) -> RateLimitKey {
    RateLimitKey {
        provider: "github".into(),
        host: " GitHub.COM ".into(),
        credential_scope: scope.into(),
    }
}
#[test]
fn rate_limit_leases_scopes_and_headers() {
    let limits = RateLimits::default();
    let k = key("first");
    let lease = limits.check(&k, 0, false).unwrap();
    limits.record_limit(&k, lease, None, 0);
    assert!(limits.check(&key("second"), 0, false).is_ok());
    assert_eq!(
        limits.check(&k, 0, false).unwrap_err().reason(),
        Some(FailureReason::RateLimited)
    );
    limits.record_success(&k, lease, 0);
    assert!(limits.check(&k, 0, false).is_err());
    let current = limits.check(&k, 1000, true).unwrap();
    assert_eq!(current, 1);
    limits.record_success(&k, current, 1000);
    assert!(limits.check(&k, 1000, false).is_err());
    limits.record_limit(&k, lease, Some(61_000), 31_000);
    assert!(limits.check(&k, 60_000, false).is_err());
    assert!(limits.check(&k, 61_000, false).is_ok());
    limits.record_success(&k, current, 61_000);
    limits.record_limit(&k, current, None, 61_000);
    assert!(limits.check(&k, 90_999, false).is_err());
    assert!(limits.check(&k, 91_000, false).is_ok());
    assert_eq!(retry_at_from_header("120", 1000), Some(121_000));
    assert_eq!(
        retry_at_from_header("Thu, 01 Jan 1970 00:02:01 GMT", 1000),
        Some(121_000)
    );
    assert_eq!(retry_at_from_header("later", 1000), None);
    assert_eq!(retry_at_from_header("-1", 1000), None);
}
#[test]
fn concurrent_limits_only_extend_the_pause() {
    let l = RateLimits::default();
    let k = key("");
    l.record_limit(&k, 0, Some(121_000), 1000);
    l.record_limit(&k, 0, Some(61_000), 1000);
    let error = l.check(&k, 1000, false).unwrap_err();
    match error {
        t3_forge::ForgeError::Provider { retry_at, .. } => assert_eq!(retry_at, Some(121_000)),
        _ => panic!(),
    };
    l.record_limit(&k, 0, Some(151_000), 1000);
    assert!(l.check(&k, 150_000, false).is_err());
}
#[test]
fn remote_recognition_priority_and_url_shapes() {
    for (remote, kind, host, repository) in [
        (
            "git@github.com:Acme/Web.git",
            "github",
            "github.com",
            "Acme/Web",
        ),
        (
            "https://gitlab.example.com/group/sub/repo.git?token=secret",
            "gitlab",
            "gitlab.example.com",
            "group/sub/repo",
        ),
        (
            "ssh://git@codeberg.org:2222/a/b.git",
            "forgejo",
            "codeberg.org",
            "a/b",
        ),
        (
            "http://gitea.local:3000/a/b.git",
            "forgejo",
            "gitea.local:3000",
            "a/b",
        ),
        (
            "git@staging.git.cafe:a/b.git",
            "gitcafe",
            "staging.git.cafe",
            "a/b",
        ),
        (
            "git@origin.cursor.com:a/b.git",
            "cursor-origin",
            "origin.cursor.com",
            "a/b",
        ),
        (
            "git@ssh.dev.azure.com:v3/org/project/repo",
            "azure-devops",
            "ssh.dev.azure.com",
            "v3/org/project/repo",
        ),
        (
            "git@bitbucket.org:a/b.git",
            "bitbucket",
            "bitbucket.org",
            "a/b",
        ),
    ] {
        let parsed = parse_remote(remote).unwrap();
        assert_eq!(parsed.host, host);
        assert_eq!(parsed.repository.as_deref(), Some(repository));
        assert_eq!(recognize_remote(remote).unwrap().kind.0, kind);
    }
    for local in ["C:\\work\\repo", "C:/work/repo", "/tmp/repo", "../repo", ""] {
        assert!(parse_remote(local).is_none(), "{local}");
    }
}
#[test]
fn registry_prefers_origin_even_when_unknown() {
    let remotes = vec![
        ("upstream".into(), "git@github.com:a/b.git".into()),
        ("origin".into(), "https://custom.example/a/b.git".into()),
    ];
    assert_eq!(select_context(&remotes).unwrap().provider.kind.0, "unknown");
    assert_eq!(
        select_context(&remotes[..1]).unwrap().provider.kind.0,
        "github"
    );
}
#[test]
fn links_keep_mounts_ports_and_cursor_identity() {
    for (url, kind, repo) in [
        ("https://git.cafe/a/b/pulls/7", "gitcafe", "a/b"),
        (
            "https://cursor.com/codebase/a/b/pull/7",
            "cursor-origin",
            "a/b",
        ),
        (
            "http://gitlab.local/group/sub/repo/-/merge_requests/7",
            "gitlab",
            "group/sub/repo",
        ),
        (
            "http://forgejo.local:3000/mount/a/b/pulls/7/files",
            "forgejo",
            "mount/a/b",
        ),
        (
            "https://dev.azure.com/org/proj/_git/repo/pullrequest/7",
            "azure-devops",
            "org/proj/_git/repo",
        ),
    ] {
        let link = parse_pull_request_url(url).unwrap();
        assert_eq!(link.provider, kind);
        assert_eq!(link.repository, repo);
        assert_eq!(link.number, 7);
    }
    assert!(parse_pull_request_url("https://user:pass@github.com/a/b/pull/7").is_none());
    assert!(parse_pull_request_url("https://github.com/a/b/pull/0").is_none());
    assert!(parse_pull_request_url("https://github.com/a/b/pull/9007199254740992").is_none());
}
#[test]
fn source_refs_and_transport_error_sanitization() {
    assert_eq!(
        parse_owner_ref(" octocat:feature ")
            .unwrap()
            .owner
            .as_deref(),
        Some("octocat")
    );
    assert!(parse_owner_ref("a/b:feature").is_none());
    assert_eq!(source_branch("octocat:feature", None), "feature");
    assert_eq!(
        transport_safe_error_value("https://user:password@example.test/a?token=secret#frag"),
        "https://example.test/a"
    );
    assert_eq!(
        transport_safe_error_value(" one\n\0two\tthree "),
        "one two three"
    );
}
#[test]
fn provider_models_preserve_missing_and_null() {
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/core.json")).unwrap();
    let raw = &value["data"]["repository"]["pullRequest"];
    let mut summary = t3_forge::github::decode::summary(raw, "github.com").unwrap();
    summary.stack = None;
    let absent = serde_json::to_value(&summary).unwrap();
    assert!(absent.get("stack").is_none());
    summary.stack = Some(None);
    let null = serde_json::to_value(&summary).unwrap();
    assert!(null["stack"].is_null());
    let round: ProviderChangeRequestSummary = serde_json::from_value(null).unwrap();
    assert_eq!(round.stack, Some(None));
    value["data"]["repository"]["pullRequest"]["stack"] =
        serde_json::json!({"number":2,"size":3,"baseRefName":"main"});
    value["data"]["repository"]["pullRequest"]["stackEntry"] = serde_json::json!({"position":1});
    value["data"]["repository"]["pullRequest"]["commits"] =
        serde_json::json!({"nodes":[{"commit":{"statusCheckRollup":{"state":"FAILURE"}}}]});
    let summary = t3_forge::github::decode::summary(
        &value["data"]["repository"]["pullRequest"],
        "github.com",
    )
    .unwrap();
    assert_eq!(summary.stack.unwrap().unwrap().base.0, "main");
    assert_eq!(
        summary.checks_state,
        Some(Some(PullRequestChecksState::Failing))
    );
}
#[test]
fn github_rate_limit_classification_matches_http_and_graphql() {
    use serde_json::json;
    use t3_forge::github::transport::is_rate_limited;
    let empty = BTreeMap::new();
    assert!(!is_rate_limited(403, &empty, &json!({}), "Forbidden"));
    assert!(!is_rate_limited(
        403,
        &BTreeMap::from([("retry-after".into(), "60".into())]),
        &json!({}),
        "Forbidden"
    ));
    assert!(is_rate_limited(429, &empty, &json!({}), ""));
    assert!(is_rate_limited(
        403,
        &empty,
        &json!({}),
        "You have triggered an abuse detection mechanism"
    ));
    assert!(is_rate_limited(
        200,
        &empty,
        &json!({"errors":[{"type":"RATE_LIMITED"}]}),
        ""
    ));
    assert!(!is_rate_limited(
        200,
        &empty,
        &json!({"errors":[{"type":"FORBIDDEN"}]}),
        ""
    ));
}
#[test]
fn check_reruns_keep_pending_runs_and_separate_workflows() {
    use serde_json::json;
    let rows = vec![
        json!({"name":"test","status":"COMPLETED","conclusion":"FAILURE","completedAt":"2026-07-01T00:00:00Z","workflowName":"CI"}),
        json!({"name":"test","status":"IN_PROGRESS","workflowName":"CI"}),
        json!({"name":"test","status":"COMPLETED","conclusion":"SUCCESS","completedAt":"2026-07-01T00:00:01Z","workflowName":"Release"}),
    ];
    let checks = t3_forge::github::decode::checks(&rows).unwrap();
    assert_eq!(checks.len(), 2);
    assert_eq!(checks[0].name.0, "CI / test");
    assert_eq!(checks[0].status, PullRequestCheckStatus::Pending);
    assert_eq!(checks[1].name.0, "Release / test");
    assert_eq!(
        t3_forge::github::decode::checks_state(&checks),
        Some(PullRequestChecksState::Pending)
    );
}
#[test]
fn agent_credits_preserve_people_and_code_fences() {
    use t3_forge::github::merge_message::remove_agent_credits;
    let text = "Change\n\nCo-authored-by: Codex <codex@openai.com>\nCo-authored-by: Human <human@example.com>\nGenerated with [Claude Code](https://claude.ai/code)\n";
    assert_eq!(
        remove_agent_credits(text),
        "Change\n\nCo-authored-by: Human <human@example.com>"
    );
    let fenced = "```\nGenerated with Codex\n```\n\nGenerated with Codex";
    assert_eq!(
        remove_agent_credits(fenced),
        "```\nGenerated with Codex\n```"
    );
    assert_eq!(
        remove_agent_credits("Co-authored-by: Human <noreply@github.com>"),
        "Co-authored-by: Human <noreply@github.com>"
    );
}

#[tokio::test]
async fn credentials_preserve_precedence_and_reject_malformed_hosts_without_cli() {
    use t3_forge::github::transport::{CredentialSettings, Credentials, GitHubCredentials};
    let settings = CredentialSettings {
        tokens: BTreeMap::from([("github.com".into(), "saved-token".into())]),
        environment: BTreeMap::from([("GH_TOKEN".into(), "environment-token".into())]),
        ..Default::default()
    };
    let saved = GitHubCredentials::new(settings)
        .get("github.com", std::path::Path::new("."))
        .await
        .unwrap();
    assert_eq!(saved.source, "settings");
    assert!(!format!("{saved:?}").contains("saved-token"));
    let settings = CredentialSettings {
        environment: BTreeMap::from([
            ("GH_TOKEN".into(), "environment-token".into()),
            ("GITHUB_TOKEN".into(), "other-token".into()),
        ]),
        ..Default::default()
    };
    let environment = GitHubCredentials::new(settings)
        .get("github.com", std::path::Path::new("."))
        .await
        .unwrap();
    assert_eq!(environment.source, "env");
    assert_ne!(saved.fingerprint, environment.fingerprint);
    let empty = GitHubCredentials::new(CredentialSettings::default());
    assert_eq!(
        empty
            .get("https://github.com/evil", std::path::Path::new("."))
            .await
            .unwrap_err()
            .reason(),
        Some(FailureReason::Unauthenticated)
    );
}
