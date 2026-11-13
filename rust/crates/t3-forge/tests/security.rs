use serde_json::{Value, json};

#[test]
fn decoder_diagnostics_do_not_echo_credentials_from_remote_data() {
    let mut recording: Value = serde_json::from_str(include_str!("fixtures/core.json")).unwrap();
    recording["data"]["repository"]["pullRequest"]["additions"] =
        json!("https://oauth:remote-secret@github.com/acme/web?token=capability");
    let error = t3_forge::github::decode::summary(
        &recording["data"]["repository"]["pullRequest"],
        "github.com",
    )
    .unwrap_err();
    assert!(!error.to_string().contains("remote-secret"));
    assert!(!format!("{error:?}").contains("capability"));
}
#[test]
fn diagnostic_url_redaction_covers_urls_embedded_in_process_messages() {
    let diagnostic = t3_forge::remote::transport_safe_error_value(
        "fatal: unable to access https://oauth:remote-secret@github.com/acme/web?token=capability: denied",
    );
    assert!(!diagnostic.contains("remote-secret"));
    assert!(!diagnostic.contains("capability"));
}

#[test]
fn recorded_detail_has_expected_typed_content() {
    use t3_forge::model::*;
    let recording: Value =
        serde_json::from_str(include_str!("fixtures/decoder-core.json")).unwrap();
    let result =
        t3_forge::github::decode::detail(&recording["data"], "github.com", &[], false).unwrap();
    assert_eq!(result.number, 7);
    assert_eq!(result.title, "Detail");
    assert_eq!(result.url, "https://github.com/pingdotgg/t3code/pull/7");
    assert_eq!(result.body, "Body");
    assert_eq!(result.head_branch, "feat/detail");
    assert_eq!(result.base_branch, "main");
    assert_eq!(result.head_sha, Some(Some("abc123".into())));
    assert_eq!(result.state, PullRequestState::Open);
    assert_eq!(result.mergeability, PullRequestMergeability::Mergeable);
    assert_eq!(
        result.base_comparison,
        Some(PullRequestBaseComparison::Unknown)
    );
    assert!(result.checks.is_empty());
    assert!(result.merge_capabilities.merge);
    assert!(result.merge_capabilities.squash);
    assert!(result.merge_capabilities.rebase);
}
