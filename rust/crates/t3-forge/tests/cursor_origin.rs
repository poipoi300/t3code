mod common;
use async_trait::async_trait;
use common::{Answer, answer, server};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use t3_forge::{FailureReason, Forge, Result, cli::*, cursor_origin::*, forge::*, model::*};
struct Origin {
    calls: Mutex<Vec<CliRequest>>,
    tokens: AtomicUsize,
}
#[async_trait]
impl ForgeCli for Origin {
    async fn run(&self, request: CliRequest) -> Result<CliResponse> {
        let response=match request.operation.as_str(){"authToken"=>format!("fixture-{}",self.tokens.fetch_add(1,Ordering::SeqCst)),"getViewer"=>json!({"authenticated":true,"tokenValid":true,"account":{"email":"fixture@example.test"}}).to_string(),"discover"=>"origin 0.1-fixture".into(),"readAvatars"=>json!({"author":{"id":"actor","avatarUrl":"https://images.example/avatar"}}).to_string(),_=>"{}".into()};
        self.calls.lock().unwrap().push(request);
        Ok(CliResponse {
            stdout: response.into_bytes(),
            stderr: vec![],
            code: 0,
            truncated: false,
        })
    }
}
fn environment() -> t3_process::Environment {
    t3_process::Environment::from([
        ("GIT_CONFIG_NOSYSTEM".into(), Some("1".into())),
        (
            "GIT_CONFIG_GLOBAL".into(),
            Some(if cfg!(windows) { "NUL" } else { "/dev/null" }.into()),
        ),
    ])
}
fn setup(mut base: url::Url) -> (tempfile::TempDir, CursorOrigin, Arc<Origin>) {
    base.set_path("/");
    let dir = tempfile::tempdir().unwrap();
    let cli = Arc::new(Origin {
        calls: Mutex::default(),
        tokens: AtomicUsize::new(0),
    });
    let forge = CursorOrigin::with_api_base(
        t3_git::Git::new(dir.path()).with_environment(environment()),
        CursorOriginConfig {
            authentication_cwd: dir.path().into(),
            environment: environment(),
        },
        base,
    )
    .unwrap()
    .with_cli(cli.clone());
    (dir, forge, cli)
}
fn pr(cwd: &Path) -> PullRequestInput {
    PullRequestInput {
        repo: ProviderRepositoryRef {
            cwd: cwd.to_string_lossy().into(),
            host: "origin.cursor.com".into(),
            repository: "fixture/web".into(),
        },
        number: 7,
    }
}
fn raw() -> Value {
    json!({"number":"7","title":"Fixture PR","webUrl":"https://cursor.com/codebase/fixture/web/pull/7","head":{"ref":"feature","sha":"abcdef0"},"base":{"ref":"main","sha":"1234567"},"author":{"user":{"id":"actor","handle":"fixture"}},"additions":"4","deletions":2,"changedFiles":"3","version":{"headSha":"abcdef0","baseSha":"1234567","potentialMergeCommit":{"state":"prepared","baseSha":"1234567"}}})
}
fn list(repo: ProviderRepositoryRef) -> ListInput {
    ListInput {
        repo,
        state: PullRequestListState::All,
        involvement: PullRequestInvolvement::Authored,
        viewer: "fixture@example.test".into(),
        limit: 10,
        query: None,
        cursor: None,
        filters: None,
    }
}
fn action(pr: PullRequestInput) -> ActionInput {
    ActionInput {
        pr,
        action: PullRequestAction::Merge,
        stack_number: None,
        expected_stack_heads: None,
        remove_agent_credits_on_merge: false,
        merge_method: Some(PullRequestMergeMethod::Squash),
        update_method: None,
    }
}
#[tokio::test]
async fn refused_session_reloads_once_and_does_not_leak_response_body() {
    let (base, task) = server(3, |r| {
        if r.headers.contains("Bearer fixture-0") {
            Answer {
                status: 401,
                body: "private refusal body".into(),
                headers: vec![],
            }
        } else {
            answer(raw())
        }
    })
    .await;
    let (dir, forge, cli) = setup(base);
    let input = pr(dir.path());
    assert_eq!(
        forge
            .get_change_request_summary(input.clone())
            .await
            .unwrap()
            .title,
        "Fixture PR"
    );
    forge.forget_change_request_reads(None).await;
    assert_eq!(
        forge.get_change_request_summary(input).await.unwrap().title,
        "Fixture PR"
    );
    let requests = task.await.unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(cli.tokens.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn opaque_long_token_resumes_and_expired_cursor_requests_refresh() {
    let token = "opaque".repeat(500);
    let expected = token.clone();
    let (base, task) = server(2, move |r| {
        let url = url::Url::parse(&format!("http://fixture{}", r.path)).unwrap();
        let pairs = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(pairs.get("author").unwrap(), "fixture@example.test");
        if let Some(token) = pairs.get("pageToken") {
            assert_eq!(token, &expected);
            answer(json!({"pullRequests":[]}))
        } else {
            answer(json!({"pullRequests":[raw()],"nextPageToken":token}))
        }
    })
    .await;
    let (dir, forge, _) = setup(base);
    let mut input = list(pr(dir.path()).repo);
    let page = forge.list_change_requests(input.clone()).await.unwrap();
    assert_eq!(
        page.items[0].author.as_ref().unwrap().avatar_url.as_deref(),
        Some("https://images.example/avatar")
    );
    assert_eq!(page.items[0].created_at, "1970-01-01T00:00:00.000Z");
    let resume = page.resume_after.unwrap();
    assert!(resume.len() < 100);
    input.cursor = Some(ProviderListCursor {
        updated_before: "".into(),
        delivered: 1,
        seen_at: None,
        resume_after: Some(resume),
    });
    assert!(
        !forge
            .list_change_requests(input.clone())
            .await
            .unwrap()
            .continues
    );
    input.cursor.as_mut().unwrap().resume_after = Some("expired".into());
    assert!(
        forge
            .list_change_requests(input)
            .await
            .unwrap_err()
            .to_string()
            .contains("Refresh")
    );
    task.await.unwrap();
}
#[tokio::test]
async fn prepared_merge_and_truncated_green_checks_are_not_reported_as_pass() {
    let (base,task)=server(5,|r|{
        if r.path.contains("check-runs"){answer(json!({"checkRuns":[{"name":"build","status":"completed","conclusion":"success"}],"nextPageToken":"same"}))}
        else if r.path.contains("git/ref"){answer(json!({"object":{"sha":"1234567"}}))}
        else if r.path.ends_with("/pulls/7"){answer(raw())}else{answer(json!({"allowMergeCommit":false,"allowSquashMerge":true}))}
    }).await;
    let (dir, forge, _) = setup(base);
    let detail = forge.get_change_request(pr(dir.path())).await.unwrap();
    let detail = serde_json::to_value(detail).unwrap();
    assert_eq!(detail["mergeability"], "mergeable");
    assert_eq!(detail["checksState"], "pending");
    assert_eq!(detail["mergeCapabilities"]["merge"], false);
    assert_eq!(detail["changedFiles"], 3);
    task.await.unwrap();
}
#[tokio::test]
async fn moved_base_uses_mergeability_and_rejects_a_stale_head_answer() {
    let (base, task) = server(5, |r| {
        if r.path.contains("git/ref") {
            answer(json!({"object":{"sha":"2222222"}}))
        } else if r.path.contains("/mergeability") {
            answer(json!({"headSha":"oldhead","verdict":"mergeable"}))
        } else if r.path.contains("check-runs") {
            answer(json!({"checkRuns":[]}))
        } else if r.path.ends_with("/pulls/7") {
            answer(raw())
        } else {
            answer(json!({}))
        }
    })
    .await;
    let (dir, forge, _) = setup(base);
    let detail =
        serde_json::to_value(forge.get_change_request(pr(dir.path())).await.unwrap()).unwrap();
    assert_eq!(detail["mergeability"], "unknown");
    task.await.unwrap();
}
#[tokio::test]
async fn an_open_stack_parent_blocks_merge_without_sending_a_write() {
    let (base, task) = server(2, |r| {
        let mut value = raw();
        if r.path.ends_with("/7") {
            value["stack"] = json!({"parentPullRequest":{"number":"6"}});
        } else {
            value["number"] = json!(6);
        }
        answer(value)
    })
    .await;
    let (dir, forge, _) = setup(base);
    assert!(
        forge
            .run_action(action(pr(dir.path())))
            .await
            .unwrap_err()
            .to_string()
            .contains("stacked on #6")
    );
    assert!(task.await.unwrap().iter().all(|r| r.method == "GET"));
}
#[tokio::test]
async fn closed_parent_allows_merge_and_mutation_invalidates_the_pull() {
    let (base, task) = server(4, |r| {
        if r.method == "POST" {
            assert_eq!(r.body, json!({"mergeMethod":"squash"}));
            answer(json!({"futureResponseField":true}))
        } else {
            let mut value = raw();
            if r.path.ends_with("/7") {
                value["stack"] = json!({"parentPullRequest":{"number":6}});
            } else {
                value["number"] = json!(6);
                value["state"] = json!("closed");
            }
            answer(value)
        }
    })
    .await;
    let (dir, forge, _) = setup(base);
    let input = pr(dir.path());
    forge.run_action(action(input.clone())).await.unwrap();
    forge.get_change_request_summary(input).await.unwrap();
    let requests = task.await.unwrap();
    assert_eq!(
        requests.iter().filter(|r| r.path.ends_with("/7")).count(),
        2
    );
}
#[tokio::test]
async fn a_push_during_patch_read_prevents_caching_under_old_version() {
    let files = Arc::new(AtomicUsize::new(0));
    let seen = files.clone();
    let (base,task)=server(5,move|r|if r.path.contains("/files?"){seen.fetch_add(1,Ordering::SeqCst);answer(json!({"files":[{"filename":"new\tfile","previousFilename":"old\tfile","status":"renamed","patch":"@@ -1 +1 @@\n-old\n+new"}]}))}else{let mut value=raw();if seen.load(Ordering::SeqCst)>0{value["version"]["headSha"]=json!("bbbbbbb");}answer(value)}).await;
    let (dir, forge, _) = setup(base);
    let input = DiffInput {
        pr: pr(dir.path()),
        cursor: None,
        commit: None,
    };
    let first = forge.get_diff(input.clone()).await.unwrap();
    assert!(first.patch.contains("rename from \"old\\011file\""));
    forge.get_diff(input).await.unwrap();
    task.await.unwrap();
    assert_eq!(files.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn inline_threads_keep_version_resolution_and_review_verdicts() {
    let (base,task)=server(4,|r|if r.path.contains("/comments?"){answer(json!({"comments":[{"id":"comment","body":"inline","createdAt":"2026-01-01T00:00:00Z","thread":{"id":"thread","path":"x.rs","side":"left","startLine":"2","endLine":"3","resolvedAt":"2026-01-02T00:00:00Z","version":{"headSha":"old"}}}]}))}else if r.path.contains("/reviews?"){answer(json!({"reviews":[{"id":"review","verdict":"approve","submittedAt":"2026-01-01T00:00:00Z","dismissal":{"dismissedAt":"2026-01-02T00:00:00Z"}}]}))}else if r.path.contains("/commits?"){answer(json!({"commits":[{"sha":"abcdef0","commit":{"message":"Subject\nBody"},"stats":{"additions":"4"}}]}))}else{answer(raw())}).await;
    let (dir, forge, _) = setup(base);
    let activity = serde_json::to_value(
        forge
            .get_change_request_activity(pr(dir.path()))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(activity["reviewThreads"][0]["line"], 3);
    assert_eq!(activity["reviewThreads"][0]["isOutdated"], true);
    assert_eq!(activity["reviewThreads"][0]["isResolved"], true);
    assert_eq!(activity["comments"][0]["reviewState"], "DISMISSED");
    assert_eq!(activity["commits"][0]["messageHeadline"], "Subject");
    task.await.unwrap();
}
#[tokio::test]
async fn checkout_none_state_finds_merged_requests_and_foreign_url_is_rejected() {
    let (base, task) = server(1, |r| {
        assert!(r.path.contains("state=all"));
        let mut row = raw();
        row["merged"] = json!(true);
        answer(json!({"pullRequests":[row]}))
    })
    .await;
    let (dir, forge, cli) = setup(base);
    let checkout=CheckoutInput{cwd:dir.path().into(),context:Some(CheckoutContext{provider:serde_json::from_value(json!({"kind":"cursor-origin","name":"Cursor Origin","baseUrl":"https://cursor.com/codebase"})).unwrap(),remote_name:"origin".into(),remote_url:"https://origin.cursor.com/fixture/web.git".into(),requested_host:None})};
    let rows = forge
        .checkout_list_change_requests(CheckoutListInput {
            checkout: checkout.clone(),
            source: None,
            head_selector: "feature".into(),
            state: None,
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(rows[0].state, ChangeRequestState::Merged);
    assert!(
        forge
            .checkout_change_request(CheckoutGetInput {
                checkout,
                reference: "https://github.com/fixture/web/pull/7".into(),
                force: false
            })
            .await
            .is_err()
    );
    assert_eq!(cli.calls.lock().unwrap().len(), 1);
    task.await.unwrap();
}
#[tokio::test]
async fn repository_is_made_private_after_creation_and_discovery_uses_namespace() {
    let (base, task) = server(4, |r| {
        if r.path.ends_with("/namespaces") {
            answer(json!({"namespaces":[{"namespace":{"type":"user","slug":"fixture"}}]}))
        } else if r.method == "POST" {
            assert_eq!(r.body, json!({"name":"web"}));
            answer(json!({"name":"web"}))
        } else {
            assert_eq!(r.method, "PATCH");
            assert_eq!(r.body, json!({"visibility":"private"}));
            answer(json!({"name":"web","fullName":"fixture/web"}))
        }
    })
    .await;
    let (dir, forge, _) = setup(base);
    let urls = forge
        .create_repository(CreateRepositoryInput {
            cwd: dir.path().into(),
            repository: "web".into(),
            visibility: SourceControlRepositoryVisibility::Private,
        })
        .await
        .unwrap();
    assert_eq!(urls.url.0, "https://origin.cursor.com/fixture/web.git");
    assert_eq!(urls.url, urls.ssh_url);
    let discovery = serde_json::to_value(forge.discover(dir.path().into()).await.unwrap()).unwrap();
    assert_eq!(discovery["auth"]["account"]["value"], "fixture");
    task.await.unwrap();
}

#[tokio::test]
async fn twice_refused_tokens_stop_without_exposing_remote_secrets() {
    let (base, task) = server(2, |_| Answer {
        status: 401,
        body: "private-refusal-token".into(),
        headers: vec![],
    })
    .await;
    let (dir, forge, cli) = setup(base);
    let error = forge
        .get_change_request_summary(pr(dir.path()))
        .await
        .unwrap_err();
    assert_eq!(error.reason(), Some(FailureReason::Unauthenticated));
    assert!(!format!("{error:?}").contains("private-refusal-token"));
    assert_eq!(cli.tokens.load(Ordering::SeqCst), 2);
    task.await.unwrap();
    struct SignedOut;
    #[async_trait]
    impl ForgeCli for SignedOut {
        async fn run(&self, _request: CliRequest) -> Result<CliResponse> {
            Ok(CliResponse {
                stdout: br#"{"authenticated":false,"tokenValid":false}"#.to_vec(),
                stderr: vec![],
                code: 1,
                truncated: false,
            })
        }
    }
    let forge = forge.with_cli(Arc::new(SignedOut));
    assert_eq!(
        forge
            .get_viewer(ViewerInput {
                cwd: dir.path().into(),
                host: None
            })
            .await
            .unwrap_err()
            .reason(),
        Some(FailureReason::Unauthenticated)
    );
}

#[tokio::test]
async fn review_positions_and_thread_writes_use_origin_wire_shapes() {
    let (base, task) = server(3, |r| {
        if r.path.ends_with("/reviews") {
            assert_eq!(r.body["verdict"], "request_changes");
            assert_eq!(
                r.body["comments"][0]["inline"],
                json!({"path":"old.rs","side":"left","startLine":2,"endLine":2})
            );
        } else if r.path.ends_with("/comments") {
            assert_eq!(r.body, json!({"body":"reply","threadId":"thread/one"}));
        } else {
            assert!(r.path.ends_with("threads/thread%2Fone"));
            assert_eq!(r.body, json!({"resolved":false}));
        }
        answer(json!({"newApiShape":true}))
    })
    .await;
    let (dir, forge, _) = setup(base);
    let input = pr(dir.path());
    forge.submit_review(ReviewInput{pr:input.clone(),verdict:PullRequestReviewVerdict::RequestChanges,body:"review".into(),comments:vec![serde_json::from_value(json!({"id":"draft","path":"old.rs","body":"fix","position":{"kind":"deleted","oldLine":2},"oldPath":"old.rs"})).unwrap()]}).await.unwrap();
    forge
        .reply_to_thread(ThreadReplyInput {
            pr: input.clone(),
            thread_id: "thread/one".into(),
            body: "reply".into(),
        })
        .await
        .unwrap();
    forge
        .set_thread_resolution(ThreadResolutionInput {
            pr: input,
            thread_id: "thread/one".into(),
            resolved: false,
        })
        .await
        .unwrap();
    task.await.unwrap();
}
