mod common;
use common::*;
use serde_json::{Value, json};
use std::sync::Arc;
use t3_forge::{
    FailureReason, Forge,
    forge::*,
    gitcafe::{GitCafe, credentials::*},
    model::*,
    rate_limit::RateLimits,
};
fn pr() -> PullRequestInput {
    PullRequestInput {
        repo: ProviderRepositoryRef {
            cwd: "fixture".into(),
            host: "git.cafe".into(),
            repository: "acme/web".into(),
        },
        number: 7,
    }
}
fn pull() -> Value {
    json!({"id":"pr-seven","number":7,"title":"Add page","state":"open","draft":false,"sourceBranch":"feature","targetBranch":"main","headOid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","observedBaseOid":"cccccccccccccccccccccccccccccccccccccccc","author":{"kind":"local","actorId":"author-id","handle":"author","displayName":"Author","avatarUrl":"/avatar/author"},"createdAt":"2026-07-01T00:00:00Z","updatedAt":"2026-07-02T00:00:00Z","version":3,"description":"Body","closedAt":null,"mergedAt":null,"sourceRepo":null,"mergeRoute":"native","viewerActorId":"author-id","capabilities":{"comment":true,"review":true,"merge":true,"edit":true,"moderate":false}})
}
fn forge(mut base: url::Url) -> GitCafe {
    base.set_path("/api");
    let credentials = Arc::new(GitCafeCredentials::new(
        "fixture".into(),
        GitCafeSettings {
            token: Some("fixture-token".into()),
            ..Default::default()
        },
    ));
    GitCafe::new(
        t3_git::Git::new("."),
        credentials,
        Arc::new(RateLimits::default()),
    )
    .unwrap()
    .with_loopback_endpoint("git.cafe", base)
    .unwrap()
}
fn action() -> ActionInput {
    ActionInput {
        pr: pr(),
        action: PullRequestAction::Merge,
        stack_number: None,
        expected_stack_heads: None,
        remove_agent_credits_on_merge: false,
        merge_method: None,
        update_method: None,
    }
}
fn snapshot(items: Value) -> Value {
    json!({"version":3,"headOid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","comparisonBaseOid":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","items":items,"next":null})
}
#[tokio::test]
async fn checkout_without_state_includes_merged_requests() {
    let (base, task) = server(2, |r| {
        let mut row = pull();
        row["state"] = json!("merged");
        if r.path.contains('?') {
            let url = url::Url::parse(&format!("http://fixture{}", r.path)).unwrap();
            assert!(!url.query_pairs().any(|(k, _)| k == "state"));
            answer(json!({"items":[row],"next":null}))
        } else {
            answer(row)
        }
    })
    .await;
    let rows = forge(base)
        .checkout_list_change_requests(CheckoutListInput {
            checkout: CheckoutInput {
                cwd: "fixture".into(),
                context: Some(CheckoutContext {
                    provider: serde_json::from_value(
                        json!({"kind":"gitcafe","name":"GitCafe","baseUrl":"https://git.cafe"}),
                    )
                    .unwrap(),
                    remote_name: "origin".into(),
                    remote_url: "https://git.cafe/acme/web.git".into(),
                    requested_host: None,
                }),
            },
            source: None,
            head_selector: "feature".into(),
            state: None,
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(rows[0].state, ChangeRequestState::Merged);
    task.await.unwrap();
}
#[tokio::test]
async fn detail_uses_newest_check_runs_and_gates_permissions() {
    let(base,task)=server(6,|r|{if r.path.contains("/status"){answer(json!({"merge":{"conflicts":"none","fastForward":false,"strategies":["merge","squash","rebase"],"permitted":true,"blockers":[{"blockedStrategies":["rebase"]}],"reasons":["provider-merge-pending"]},"checks":{"total":2,"pending":0,"failing":1}}))}else if r.path.contains("/checks"){answer(json!({"items":[{"id":"900719925474099300000","provider":"github","githubApp":"ci","name":"test","status":"completed","conclusion":"failure","summary":null,"detailsUrl":null},{"id":"900719925474099300001","provider":"github","githubApp":"ci","name":"test","status":"queued","conclusion":null,"summary":null,"detailsUrl":null}],"next":null}))}else if r.path.contains("/changes"){answer(snapshot(json!([{"path":"a.rs","additions":4,"deletions":2}])))}else if r.path.contains("/reviewers"){answer(json!({"items":[{"id":"r-id","actor":{"kind":"github","actorId":"r-id","login":"github-reviewer","avatarUrl":null,"linkedProfile":{"handle":"local-reviewer"}}}],"next":null}))}else if r.path.contains("/labels"){answer(json!({"items":[{"id":"label-id","name":"backend","color":"ff0000"}],"next":null}))}else{answer(pull())}}).await;
    let detail = serde_json::to_value(forge(base).get_change_request(pr()).await.unwrap()).unwrap();
    assert_eq!(detail["checks"].as_array().unwrap().len(), 1);
    assert_eq!(detail["checksState"], "pending");
    assert_eq!(detail["additions"], 4);
    assert_eq!(detail["deletions"], 2);
    assert_eq!(detail["changedFiles"], 1);
    assert_eq!(detail["baseComparison"], "behind");
    assert_eq!(detail["autoMergeEnabled"], true);
    assert_eq!(detail["mergeCapabilities"]["rebase"], false);
    assert!(
        !detail["viewerPermissions"]["actions"]
            .as_array()
            .unwrap()
            .contains(&json!("merge"))
    );
    assert_eq!(detail["viewerPermissions"]["verdicts"], json!(["comment"]));
    assert_eq!(
        detail["reviewRequestLogins"],
        json!(["github-reviewer", "local-reviewer"])
    );
    assert_eq!(
        detail["author"]["avatarUrl"],
        "https://git.cafe/avatar/author"
    );
    task.await.unwrap();
}
#[tokio::test]
async fn creation_cursor_does_not_skip_rows_when_newer_pulls_disappear() {
    let (base, task) = server(1, |_| {
        let mut newer = pull();
        newer["number"] = json!(9);
        let mut boundary = pull();
        boundary["number"] = json!(8);
        answer(json!({"items":[newer,boundary,pull()],"next":null}))
    })
    .await;
    let page = forge(base)
        .list_change_requests(ListInput {
            repo: pr().repo,
            state: PullRequestListState::Open,
            involvement: PullRequestInvolvement::All,
            viewer: "author".into(),
            limit: 1,
            query: None,
            cursor: Some(ProviderListCursor {
                updated_before: "2026-07-02T00:00:00Z".into(),
                delivered: 100,
                seen_at: None,
                resume_after: Some("2026-07-01T00:00:00Z~8".into()),
            }),
            filters: None,
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].number, 7);
    assert_eq!(page.resume_after.as_deref(), Some("2026-07-01T00:00:00Z~7"));
    let requests = task.await.unwrap();
    assert!(!requests[0].path.contains("skip"));
    assert!(requests[0].headers.contains("Bearer fixture-token"));
}
#[tokio::test]
async fn fork_contents_use_comparison_base_and_verify_blob_identity() {
    let(base,task)=server(4,|r|{if r.path.contains("/changes"){answer(snapshot(json!([])))}else if r.path.contains("/blob?"){let url=url::Url::parse(&format!("http://fixture{}",r.path)).unwrap();let params:std::collections::BTreeMap<_,_>=url.query_pairs().into_owned().collect();answer(json!({"commit":params["oid"],"path":params["path"],"size":4,"binary":false,"content":if r.path.contains("/fork/web/"){"new\n"}else{"old\n"}}))}else{let mut raw=pull();raw["sourceRepo"]=json!({"owner":"fork","name":"web"});answer(raw)}}).await;
    let contents = forge(base)
        .get_diff_file_contents(FileContentsInput {
            pr: pr(),
            commit: None,
            change_type: PullRequestDiffFileContentsInputChangeType::Change,
            old_path: "old&path".into(),
            new_path: "new#path".into(),
        })
        .await
        .unwrap();
    assert_eq!(contents.old_contents, "old\n");
    assert_eq!(contents.new_contents, "new\n");
    let requests = task.await.unwrap();
    assert!(requests.iter().any(|r| {
        r.path
            .contains("oid=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
    }));
    assert!(
        requests
            .iter()
            .any(|r| r.path.contains("/repos/fork/web/blob?"))
    );
}
#[tokio::test]
async fn diff_pages_are_fenced_and_keep_omitted_file_stats() {
    let(base,task)=server(3,|r|{if r.path.contains("/changes"){let mut raw=snapshot(json!([{"path":"new.txt","oldPath":"old.txt","status":"renamed"},{"path":"large","tooLarge":true,"additions":15,"deletions":2}]));raw["next"]=json!("opaque-next");answer(raw)}else if r.path.ends_with("/diff-files"){assert_eq!(r.body["expectedVersion"],3);assert_eq!(r.body["paths"],json!(["new.txt"]));answer(snapshot(json!([{"path":"new.txt","oldPath":"old.txt","status":"renamed","hunks":[{"oldStart":1,"oldLines":1,"newStart":1,"newLines":1,"lines":[{"origin":"-","content":"old\n"},{"origin":"+","content":"new"}]}]}])))}else{answer(pull())}}).await;
    let diff = forge(base)
        .get_diff(DiffInput {
            pr: pr(),
            cursor: None,
            commit: None,
        })
        .await
        .unwrap();
    assert!(diff.truncated);
    assert!(
        diff.patch
            .contains("rename from old.txt\nrename to new.txt")
    );
    assert!(diff.patch.contains("+new\n\\ No newline at end of file\n"));
    assert_eq!(
        serde_json::to_value(diff.omitted_file_stats).unwrap()[0]["additions"],
        15
    );
    let cursor: Value = serde_json::from_str(diff.next_cursor.as_ref().unwrap()).unwrap();
    assert_eq!(cursor["after"], "opaque-next");
    assert_eq!(cursor["version"], 3);
    task.await.unwrap();
}
#[tokio::test]
async fn changed_diff_revision_is_refused_before_hunks_are_requested() {
    let (base, task) = server(1, |_| {
        let mut raw = snapshot(json!([]));
        raw["version"] = json!(4);
        answer(raw)
    })
    .await;
    let error = forge(base)
        .get_diff(DiffInput {
            pr: pr(),
            cursor: Some(
                json!({"version":3,"headOid":"a","baseOid":"b","after":"opaque"}).to_string(),
            ),
            commit: None,
        })
        .await
        .unwrap_err();
    assert_eq!(error.reason(), Some(FailureReason::Failed));
    task.await.unwrap();
}
#[tokio::test]
async fn reviewer_toggle_and_comment_edit_use_their_own_version_fences() {
    let (base, task) = server(5, |r| {
        if r.method == "PUT" {
            answer(json!({"version":4}))
        } else if r.method == "PATCH" {
            answer(json!({"id":"comment-one"}))
        } else if r.path.contains("/detail") {
            answer(json!({"id":"comment-one","version":10}))
        } else if r.path.contains("/reviewers") {
            answer(json!({"items":[{"id":"old-reviewer"}],"next":null}))
        } else {
            answer(pull())
        }
    })
    .await;
    let forge = forge(base);
    forge
        .set_reviewer_request(ReviewerRequestInput {
            pr: pr(),
            reviewers: vec![
                serde_json::from_value(
                    json!({"id":"new-reviewer","login":"reviewer","kind":"user"}),
                )
                .unwrap(),
            ],
            requested: true,
        })
        .await
        .unwrap();
    forge
        .update_comment(UpdateCommentInput {
            pr: pr(),
            comment_id: "comment-one".into(),
            kind: PullRequestCommentUpdateInputKind::IssueComment,
            body: "Edited".into(),
        })
        .await
        .unwrap();
    let requests = task.await.unwrap();
    assert_eq!(
        requests[2].body,
        json!({"expectedVersion":3,"actorIds":["old-reviewer","new-reviewer"]})
    );
    assert_eq!(
        requests[4].body,
        json!({"expectedVersion":10,"body":"Edited"})
    );
}
#[tokio::test]
async fn stack_merge_checks_reviewed_heads_before_landing() {
    let(base,task)=server(2,|r|if r.path.ends_with("/stack"){answer(json!({"stack":{"id":"stack-one","number":1,"revision":7,"landingBase":"main","members":[{"pullRequestNumber":7,"title":"Add page","state":"open","draft":false,"sourceBranch":"feature","position":1}]}}))}else{answer(pull())}).await;
    let mut input = action();
    input.stack_number = Some(1);
    input.expected_stack_heads = Some(vec![
        serde_json::from_value(json!({"number":7,"headSha":"outdated-head"})).unwrap(),
    ]);
    assert!(forge(base).run_action(input).await.is_err());
    task.await.unwrap();
}
#[tokio::test]
async fn merge_pins_current_target_head_and_review_pins_current_pull_head() {
    let (base, task) = server(5, |r| {
        if r.path.contains("/commit?") {
            answer(json!({"oid":"current-target"}))
        } else if r.path.ends_with("/merge") {
            answer(json!({"id":"operation-one","state":"accepted"}))
        } else if r.path.ends_with("/reviews") {
            answer(json!({"id":"review-one"}))
        } else {
            answer(pull())
        }
    })
    .await;
    let forge = forge(base);
    forge.run_action(action()).await.unwrap();
    forge
        .submit_review(ReviewInput {
            pr: pr(),
            verdict: PullRequestReviewVerdict::RequestChanges,
            body: "Please fix".into(),
            comments: vec![],
        })
        .await
        .unwrap();
    let requests = task.await.unwrap();
    assert_eq!(requests[2].body["baseOid"], "current-target");
    assert_eq!(
        requests[2].body["headOid"],
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert_eq!(requests[2].body["expectedVersion"], 3);
    uuid::Uuid::parse_str(requests[2].body["requestId"].as_str().unwrap()).unwrap();
    assert_eq!(requests[4].body["verdict"], "request_changes");
    assert_eq!(requests[4].body["expectedVersion"], 3);
}
#[derive(Default)]
struct CredentialCli {
    requests: std::sync::Mutex<Vec<t3_forge::cli::CliRequest>>,
}
#[async_trait::async_trait]
impl t3_forge::cli::ForgeCli for CredentialCli {
    async fn run(
        &self,
        request: t3_forge::cli::CliRequest,
    ) -> t3_forge::Result<t3_forge::cli::CliResponse> {
        let stdout = if request.operation == "credential" {
            "password=cli-token\n\n".into()
        } else if request.args == vec!["--version"] {
            "cafe 1.0\n".into()
        } else {
            json!({"schemaVersion":1,"data":{"host":"git.cafe","username":"fixture-user"}})
                .to_string()
        };
        self.requests.lock().unwrap().push(request);
        Ok(t3_forge::cli::CliResponse {
            stdout: stdout.into_bytes(),
            stderr: vec![],
            code: 0,
            truncated: false,
        })
    }
}
#[tokio::test]
async fn credentials_scope_saved_and_environment_tokens_and_cache_the_cli_fallback() {
    use t3_forge::http::{ApiCredential, ApiCredentials};
    let cli = Arc::new(CredentialCli::default());
    let environment = t3_process::Environment::from([
        (
            "CAFE_HOST".into(),
            Some("https://staging.git.cafe/api".into()),
        ),
        ("CAFE_TOKEN".into(), Some("stage-token".into())),
    ]);
    let credentials = GitCafeCredentials::new(
        "fixture".into(),
        GitCafeSettings {
            token: Some("saved-token".into()),
            environment,
        },
    )
    .with_cli(cli.clone());
    assert!(
        matches!(credentials.get("git.cafe").await.unwrap(),ApiCredential::Bearer(token) if token=="saved-token")
    );
    assert!(
        matches!(credentials.get("staging.git.cafe").await.unwrap(),ApiCredential::Bearer(token) if token=="stage-token")
    );
    assert!(credentials.get("attacker.invalid").await.is_err());
    assert!(cli.requests.lock().unwrap().is_empty());
    credentials
        .update_settings(GitCafeSettings::default())
        .await;
    for _ in 0..2 {
        assert!(
            matches!(credentials.get("git.cafe").await.unwrap(),ApiCredential::Bearer(token) if token=="cli-token")
        );
    }
    assert_eq!(cli.requests.lock().unwrap().len(), 1);
    credentials.invalidate("git.cafe").await;
    credentials.get("git.cafe").await.unwrap();
    let requests = cli.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].stdin.as_deref(),
        Some(b"protocol=https\nhost=git.cafe\n\n".as_slice())
    );
    assert!(
        requests[0]
            .args
            .contains(&"--host=https://git.cafe/api".into())
    );
    assert!(!requests[0].args.iter().any(|arg| arg.contains("token")));
}
#[tokio::test]
async fn discovery_injects_saved_token_into_cafe_without_reading_a_provider_home() {
    let cli = Arc::new(CredentialCli::default());
    let credentials = Arc::new(
        GitCafeCredentials::new(
            "fixture".into(),
            GitCafeSettings {
                token: Some("saved-token".into()),
                ..Default::default()
            },
        )
        .with_cli(cli.clone()),
    );
    let forge = GitCafe::new(
        t3_git::Git::new("."),
        credentials,
        Arc::new(RateLimits::default()),
    )
    .unwrap()
    .with_cli(cli.clone());
    let discovery = serde_json::to_value(forge.discover("fixture".into()).await.unwrap()).unwrap();
    assert_eq!(discovery["auth"]["status"], "authenticated");
    assert_eq!(
        discovery["auth"]["account"],
        json!({"_tag":"Some","value":"fixture-user"})
    );
    let requests = cli.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1]
            .environment
            .get(std::ffi::OsStr::new("CAFE_TOKEN"))
            .unwrap()
            .as_deref(),
        Some(std::ffi::OsStr::new("saved-token"))
    );
}
#[tokio::test]
async fn write_redirects_require_method_preservation_and_remain_under_api() {
    let (base, task) = server(1, |_| Answer {
        status: 302,
        body: String::new(),
        headers: vec![("Location".into(), "/api/moved".into())],
    })
    .await;
    assert!(
        forge(base)
            .comment(CommentInput {
                pr: pr(),
                body: "Body".into()
            })
            .await
            .is_err()
    );
    task.await.unwrap();
    let (base, task) = server(1, |_| Answer {
        status: 307,
        body: String::new(),
        headers: vec![("Location".into(), "/outside-api".into())],
    })
    .await;
    assert!(
        forge(base)
            .comment(CommentInput {
                pr: pr(),
                body: "Body".into()
            })
            .await
            .is_err()
    );
    task.await.unwrap();
}
#[tokio::test]
async fn repository_admission_and_thread_resolution_preserve_server_versions() {
    let(base,task)=server(3,|r|if r.path.ends_with("/repos"){answer(json!({"repoId":"repo-one","state":"complete","owner":"acme","name":"web"}))}else if r.path.contains("/comments?"){answer(json!({"items":[{"id":"root-one","threadId":"root-one","version":8,"resolvedAt":null}],"next":null}))}else{answer(json!({"id":"root-one"}))}).await;
    let forge = forge(base);
    let urls = forge
        .create_repository(CreateRepositoryInput {
            cwd: "fixture".into(),
            repository: "acme/web".into(),
            visibility: SourceControlRepositoryVisibility::Private,
        })
        .await
        .unwrap();
    assert_eq!(urls.ssh_url.0, "ssh@git.cafe:acme/web.git");
    forge
        .set_thread_resolution(ThreadResolutionInput {
            pr: pr(),
            thread_id: "root-one".into(),
            resolved: true,
        })
        .await
        .unwrap();
    let requests = task.await.unwrap();
    assert_eq!(requests[0].body["visibility"], "private");
    uuid::Uuid::parse_str(requests[0].body["requestKey"].as_str().unwrap()).unwrap();
    assert_eq!(requests[2].body, json!({"expectedVersion":8}));
    assert!(requests[2].path.ends_with("/comments/root-one/resolve"));
}
